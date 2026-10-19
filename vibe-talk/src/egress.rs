//! Outbound HTTP clients, and the switch that keeps a test's clients on this machine.
//!
//! Every vendor client (Discord protocol, Slack, ElevenLabs, the summariser's Messages API) is
//! built from [`client_builder`]. In the server that is `reqwest::Client::builder()` and nothing
//! more. [`crate::testing`] calls [`confine_to_loopback`] first; every client this process builds
//! after that resolves only `localhost`, uses no proxy, and fails any other host name at name
//! resolution, as a transport error, before a packet leaves. A test whose configuration still
//! names a vendor's real API base (a legacy `[discord]` section with no `api_base`) then reads
//! that vendor as unreachable on every host, instead of reaching it wherever the network allows
//! and getting a different answer there. `#359 provider-diagnostics-ids`.
//!
//! What it does not cover: a client built before the process first used [`crate::testing`]; a URL
//! whose host is an IP literal, which is never resolved (tests use loopback literals only); and
//! websocket clients, which are not built here. The mock voice bridge's own client
//! (`elevenlabs::mock`) and the `pair-link` command's client (`pair_link`, which talks only to
//! this host's own server and refuses any other address) are not vendor clients and are
//! deliberately not built here either.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

static LOOPBACK_ONLY: AtomicBool = AtomicBool::new(false);

/// Confine every HTTP client this process builds from now on to loopback.
///
/// Test support, called by [`crate::testing`]. The server never calls it. It cannot be undone.
pub fn confine_to_loopback() {
    LOOPBACK_ONLY.store(true, Ordering::SeqCst);
}

/// Whether [`confine_to_loopback`] has been called in this process.
#[must_use]
pub fn is_confined() -> bool {
    LOOPBACK_ONLY.load(Ordering::SeqCst)
}

/// The builder every vendor client starts from.
pub(crate) fn client_builder() -> reqwest::ClientBuilder {
    let builder = reqwest::Client::builder();
    if is_confined() {
        // No proxy: a proxy named in the environment would carry the request past the resolver.
        builder.no_proxy().dns_resolver(Arc::new(LoopbackOnly))
    } else {
        builder
    }
}

/// Resolves `localhost` to 127.0.0.1 and refuses every other name.
struct LoopbackOnly;

impl reqwest::dns::Resolve for LoopbackOnly {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            if host.eq_ignore_ascii_case("localhost") {
                // Port 0 is replaced by the URL's port.
                let addrs: reqwest::dns::Addrs =
                    Box::new(std::iter::once(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))));
                Ok(addrs)
            } else {
                Err(format!("a test may reach loopback only, not {host}").into())
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The causes under an error, which reqwest's own `Display` leaves out.
    fn chain(error: &dyn std::error::Error) -> String {
        let mut text = error.to_string();
        let mut next = error.source();
        while let Some(cause) = next {
            text.push_str(": ");
            text.push_str(&cause.to_string());
            next = cause.source();
        }
        text
    }

    #[tokio::test]
    async fn a_confined_client_reaches_loopback_and_refuses_any_other_name() {
        confine_to_loopback();
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind");
        let port = listener.local_addr().expect("address").port();
        let app = axum::Router::new().route("/", axum::routing::get(|| async { "here" }));
        let server = tokio::spawn(async move { axum::serve(listener, app).await });
        let client = client_builder().build().expect("client");

        for base in ["127.0.0.1", "localhost"] {
            let body = client
                .get(format!("http://{base}:{port}/"))
                .send()
                .await
                .expect("loopback answers")
                .text()
                .await
                .expect("body");
            assert_eq!(body, "here", "{base}");
        }
        let refused = client
            .get("https://discord.com/api/v10/users/@me")
            .send()
            .await
            .expect_err("a public name is refused");
        assert!(refused.is_connect(), "{refused:?}");
        assert!(
            chain(&refused).contains("loopback only, not discord.com"),
            "{}",
            chain(&refused)
        );
        server.abort();
    }
}
