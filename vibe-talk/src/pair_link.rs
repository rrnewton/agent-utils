//! `vibe-talk pair-link`: print a link that signs one phone in. `#369 pairing-link`.
//!
//! An agent that set a server up for a person finishes by running this on the server's host and
//! pasting what it prints into the chat. It reads the write token from the server's own
//! configuration (the same file and environment the server reads, see [`Config::discover`]), asks
//! the running server on its bind address for a pairing code, and prints the link and when it
//! expires. It never prints the token, and the code appears only inside the link.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Context as _;

use crate::config::Config;
use crate::http::api::PairResponse;

/// Help for `vibe-talk pair-link --help`.
pub const USAGE: &str = "\
vibe-talk pair-link — print a one-time link that signs a phone in

USAGE:
    vibe-talk pair-link [--config PATH] [--config-overlay PATH]... [--base-url URL] [--ttl SECONDS]

Asks the vibe-talk server running on this host for a single-use pairing code, and prints a link
to the app carrying it, then when the link expires. Opening the link signs that browser in with the
write token, exactly as pasting the token into the sign-in form would. The link works ONCE, and
only until it expires; send it to the person in chat, and they open it on their phone and tap
Sign this device in. Whoever uses it first is signed in, so send it where only the person reads it.
vibe-talk redacts pairing codes from everything it sends its read-scope callers, so a bridged
channel is safe from them, but not from anyone else in it.

The write token is read from the server's own configuration (the file, then VIBE_TALK_WRITE_TOKEN,
which wins) and is never printed. The server is reached on the configuration's [server] bind
address; 0.0.0.0 and [::] are reached on loopback, and an address that is not this host's is
refused. Nothing but the link and its expiry is written to standard output; diagnostics, and a
reminder of where the link is safe to send, go to standard error.

OPTIONS:
    --config PATH          the server's configuration file, found exactly as the server finds it
                           (default: $VIBE_TALK_CONFIG, else ./vibe-talk.toml; with neither, the
                           configuration comes from the environment alone)
    --config-overlay PATH  a TOML file laid over the configuration, as for the server. Repeatable;
                           VIBE_TALK_CONFIG_OVERLAY=PATH[:PATH...] is applied after the flags
    --base-url URL         the address the phone opens the app at, such as https://vibe.example.
                           Default: [server] public_url (also spelled public_base_url) or
                           VIBE_TALK_PUBLIC_BASE_URL. One of them is required, because the bind
                           address is rarely what a phone can reach
    --ttl SECONDS          how long the link works, 1 to 600 (default: 300, five minutes)
    --help                 print this message and exit

EXAMPLES:
    vibe-talk pair-link
    vibe-talk pair-link --config /etc/vibe-talk/vibe-talk.toml --ttl 120
    vibe-talk pair-link --base-url https://vibe.example
";

/// What `pair-link` was asked to do.
#[derive(Debug, Default)]
pub struct Options {
    /// `--config`.
    pub config: Option<PathBuf>,
    /// `--config-overlay`, in order.
    pub overlays: Vec<PathBuf>,
    /// `--base-url`.
    pub base_url: Option<String>,
    /// `--ttl`, in seconds.
    pub ttl_seconds: Option<u64>,
}

/// Parse the arguments after `pair-link`. `Ok(None)` means `--help` was printed.
///
/// # Errors
///
/// An unknown argument, a flag without its value, or a `--ttl` that is not a whole number.
pub fn parse(args: impl IntoIterator<Item = String>) -> anyhow::Result<Option<Options>> {
    let mut options = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print!("{USAGE}");
                return Ok(None);
            }
            "--config" => {
                options.config = Some(args.next().context("--config needs a path")?.into())
            }
            "--config-overlay" => options
                .overlays
                .push(args.next().context("--config-overlay needs a path")?.into()),
            "--base-url" => options.base_url = Some(args.next().context("--base-url needs a URL")?),
            "--ttl" => {
                let value = args.next().context("--ttl needs a number of seconds")?;
                let seconds = value
                    .parse::<u64>()
                    .with_context(|| format!("--ttl takes whole seconds, not {value:?}"))?;
                options.ttl_seconds = Some(seconds);
            }
            other => anyhow::bail!("unrecognized argument {other:?}\n\n{USAGE}"),
        }
    }
    Ok(Some(options))
}

/// The link and its expiry, as printed.
#[derive(Debug)]
pub struct PairLink {
    /// `BASE/#pair=CODE`.
    pub link: String,
    /// RFC 3339, from the server.
    pub expires_at: String,
}

/// Mint a code on the running server and build the link.
///
/// # Errors
///
/// The configuration cannot be loaded, no base URL is known or it is not an `http(s)` address
/// without a query or fragment, the server cannot be reached, or it refuses.
pub async fn run(options: Options) -> anyhow::Result<PairLink> {
    if let Some(ttl) = options.ttl_seconds {
        let max = crate::pairing::MAX_TTL.as_secs();
        if ttl == 0 || ttl > max {
            anyhow::bail!("--ttl must be between 1 and {max} seconds, not {ttl}");
        }
    }
    let config = Config::discover(options.config, options.overlays)?;
    let base = options
        .base_url
        .or_else(|| config.public_base_url.clone())
        .context(
            "no address to build the link on: set [server] public_url in the configuration (or \
             VIBE_TALK_PUBLIC_BASE_URL), or pass --base-url https://...",
        )?;
    let base = link_base(&base)?;
    let server = local_address(config.bind);
    if !is_this_host(server.ip()) {
        anyhow::bail!(
            "the configuration's bind address {} is not an address of this host, and the write \
             token is sent only to this host; run pair-link where the server runs",
            server.ip()
        );
    }
    let client = reqwest::Client::builder()
        // The server is on this host. A proxy from the environment would carry the write token
        // somewhere else, so none is used, and a redirect is an answer, never followed.
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(15))
        .build()
        .context("building the HTTP client")?;
    let url = format!("http://{server}/api/v1/pair");
    let response = client
        .post(&url)
        .bearer_auth(config.auth.write_token.expose())
        .json(&serde_json::json!({ "ttl_seconds": options.ttl_seconds }))
        .send()
        .await
        .with_context(|| {
            format!(
                "could not reach the vibe-talk server at {server} (the configuration's bind \
                 address); is it running?"
            )
        })?;
    let status = response.status();
    let body = response
        .bytes()
        .await
        .with_context(|| format!("reading the answer from {server}"))?;
    if !status.is_success() {
        #[derive(serde::Deserialize)]
        struct Refusal {
            error: String,
            detail: String,
        }
        let detail = serde_json::from_slice::<Refusal>(&body)
            .map(|error| format!("{}: {}", error.error, error.detail))
            .unwrap_or_else(|_| "no detail".to_owned());
        anyhow::bail!("the server at {server} refused to mint a pairing code ({status}): {detail}");
    }
    let minted: PairResponse = serde_json::from_slice(&body)
        .with_context(|| format!("the server at {server} answered with something unexpected"))?;
    Ok(PairLink {
        link: format!("{base}/#pair={}", minted.code),
        expires_at: minted.expires_at,
    })
}

/// The base URL with its trailing slashes taken off, after checking it can carry a fragment.
fn link_base(raw: &str) -> anyhow::Result<String> {
    let trimmed = raw.trim();
    let parsed = url::Url::parse(trimmed)
        .with_context(|| format!("the base URL {trimmed:?} is not a URL"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        anyhow::bail!("the base URL {trimmed:?} must be an http:// or https:// address");
    }
    if parsed.query().is_some() || parsed.fragment().is_some() {
        anyhow::bail!("the base URL {trimmed:?} must not carry a query or a fragment");
    }
    Ok(trimmed.trim_end_matches('/').to_owned())
}

/// Where on this host the server listening on `bind` is reached: an unspecified address is
/// reached on loopback.
#[must_use]
pub fn local_address(bind: SocketAddr) -> SocketAddr {
    let ip = match bind.ip() {
        IpAddr::V4(v4) if v4.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(v6) if v6.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        other => other,
    };
    SocketAddr::new(ip, bind.port())
}

/// Whether `ip` is loopback or assigned to one of this host's interfaces: only such an address can
/// be bound, so a configuration copied from another host cannot send the token there.
#[must_use]
pub fn is_this_host(ip: IpAddr) -> bool {
    ip.is_loopback() || std::net::UdpSocket::bind(SocketAddr::new(ip, 0)).is_ok()
}

/// What `pair-link` says on standard error once the link is printed: where it is safe to send.
pub const SEND_WARNING: &str = "\
Send this link only where the person will read it: whoever uses it first is signed in with the
write token. vibe-talk never shows a pairing code to its read-scope callers, but anyone else who
can read the chat can see it. It works once: if the person is told it was already used when they
had not opened it, someone else did, and VIBE_TALK_WRITE_TOKEN should be rotated.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unspecified_bind_address_is_reached_on_loopback() {
        assert_eq!(
            local_address("0.0.0.0:8080".parse().expect("addr")),
            "127.0.0.1:8080".parse::<SocketAddr>().expect("addr")
        );
        assert_eq!(
            local_address("[::]:8080".parse().expect("addr")),
            "[::1]:8080".parse::<SocketAddr>().expect("addr")
        );
        assert_eq!(
            local_address("10.0.0.5:9".parse().expect("addr")),
            "10.0.0.5:9".parse::<SocketAddr>().expect("addr")
        );
    }

    #[test]
    fn only_an_address_of_this_host_is_sent_the_token() {
        assert!(is_this_host("127.0.0.1".parse().expect("ip")));
        assert!(is_this_host("::1".parse().expect("ip")));
        // TEST-NET-1 and the IPv6 documentation prefix: assigned to no host.
        assert!(!is_this_host("192.0.2.1".parse().expect("ip")));
        assert!(!is_this_host("2001:db8::1".parse().expect("ip")));
    }

    #[test]
    fn the_base_url_must_be_able_to_carry_the_fragment() {
        assert_eq!(
            link_base("https://vibe.example/").expect("base"),
            "https://vibe.example"
        );
        assert_eq!(
            link_base("https://vibe.example/app//").expect("base"),
            "https://vibe.example/app"
        );
        for bad in [
            "vibe.example",
            "ftp://vibe.example",
            "https://vibe.example/?a=1",
            "https://vibe.example/#x",
        ] {
            assert!(link_base(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_arguments_parse() {
        let options = parse(
            [
                "--config",
                "/x.toml",
                "--ttl",
                "120",
                "--base-url",
                "https://vibe.example",
            ]
            .map(String::from),
        )
        .expect("parse")
        .expect("options");
        assert_eq!(options.config, Some(PathBuf::from("/x.toml")));
        assert_eq!(options.ttl_seconds, Some(120));
        assert_eq!(options.base_url.as_deref(), Some("https://vibe.example"));
        assert!(parse(["--ttl", "soon"].map(String::from)).is_err());
        assert!(parse(["--nope"].map(String::from)).is_err());
    }
}
