//! `vibe-talk doctor`: a pre-flight setup check. `#1780 vibe-talk-setup-routes-and-pairing-ux`.
//!
//! The server already refuses a configuration it cannot load. What it does not do is warn about
//! a configuration that loads and starts, then fails the person using it later: no durable
//! storage (the channel views answer 503 while `/healthz` passes), no public address (pairing
//! has nothing to build a link on), or plain HTTP on every interface (no microphone, no
//! installable app, tokens in the clear). `doctor` names each one, with its fix, before the
//! first start. It reads configuration exactly as the server does and makes no network call.

use crate::config::{Config, ProviderKind, ReadAloudBackend};

/// How much a finding matters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// The deployment cannot do what the setup guide promises until this is fixed.
    Fail,
    /// It runs, but a named feature will surprise the user until this is fixed.
    Warn,
    /// It runs as configured; this only records a choice.
    Ok,
}

/// One setup finding: what was checked, what was found, and the fix.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// Severity.
    pub level: Level,
    /// Short check name, stable for tests and scripts.
    pub check: &'static str,
    /// What the configuration says, in one sentence.
    pub detail: String,
    /// The concrete next step, in one sentence.
    pub fix: String,
}

/// Check a loaded configuration. Pure: no I/O, so the same findings serve the CLI and tests.
#[must_use]
pub fn check(config: &Config) -> Vec<Finding> {
    let mut findings = Vec::new();

    // Durable storage: without it the channel views answer 503 while /healthz still passes.
    match &config.storage.path {
        Some(path) if path.is_absolute() => findings.push(Finding {
            level: Level::Ok,
            check: "storage",
            detail: format!("durable storage is {}", path.display()),
            fix: "none needed".to_owned(),
        }),
        Some(path) => findings.push(Finding {
            level: Level::Fail,
            check: "storage",
            detail: format!("storage path {} is not absolute", path.display()),
            fix: "set [storage] path to an absolute path outside the container image, for example /var/lib/vibe-talk/vibe-talk.sqlite3".to_owned(),
        }),
        None => findings.push(Finding {
            level: Level::Fail,
            check: "storage",
            detail: "no durable storage is configured, so the channel views answer 503 while /healthz still passes".to_owned(),
            fix: "set [storage] path to an absolute path outside the container image, for example /var/lib/vibe-talk/vibe-talk.sqlite3".to_owned(),
        }),
    }

    // Placeholder provider credentials load fine but can never read a channel.
    let placeholder = config.providers.iter().any(|provider| {
        let token = match &provider.kind {
            ProviderKind::Discord(discord) => discord.bot_token.expose(),
            ProviderKind::Slack(slack) => slack.token.expose(),
        };
        token.contains("REPLACE-ME") || token == "REPLACE_ME"
    });
    if placeholder {
        findings.push(Finding {
            level: Level::Fail,
            check: "provider-token",
            detail: "a chat provider token is still the placeholder from the example configuration".to_owned(),
            fix: "replace it with the real provider credential; see SETUP.md step 1".to_owned(),
        });
    } else {
        findings.push(Finding {
            level: Level::Ok,
            check: "provider-token",
            detail: "every chat provider has a non-placeholder credential".to_owned(),
            fix: "none needed".to_owned(),
        });
    }

    // The address a phone opens: pairing builds its link on it, and the microphone needs HTTPS.
    match &config.public_base_url {
        None => findings.push(Finding {
            level: Level::Warn,
            check: "public-address",
            detail: "no public address is configured, so pair-link needs --base-url on every run".to_owned(),
            fix: "set [server] public_url to the address a phone opens, for example https://vibe.example".to_owned(),
        }),
        Some(url) if url.starts_with("http://") && !is_local(url) => findings.push(Finding {
            level: Level::Warn,
            check: "public-address",
            detail: format!("the public address {url} is plain HTTP, so the browser gives the page no microphone and no installable app"),
            fix: "put a TLS front end in front of the server and set [server] public_url to its https:// address; see SETUP.md connection routes".to_owned(),
        }),
        Some(url) => findings.push(Finding {
            level: Level::Ok,
            check: "public-address",
            detail: format!("the public address is {url}"),
            fix: "none needed".to_owned(),
        }),
    }

    // Binding every interface serves plain HTTP to the whole network unless a front end owns TLS.
    if config.bind.ip().is_unspecified() {
        findings.push(Finding {
            level: Level::Warn,
            check: "bind-exposure",
            detail: format!("the server binds {} in plain HTTP on every interface", config.bind),
            fix: "bind 127.0.0.1 and let one connection route in SETUP.md reach it, or put a TLS front end in front of it".to_owned(),
        });
    } else {
        findings.push(Finding {
            level: Level::Ok,
            check: "bind-exposure",
            detail: format!("the server binds {}", config.bind),
            fix: "none needed".to_owned(),
        });
    }

    if !config.channels.iter().any(|channel| channel.writable) {
        findings.push(Finding {
            level: Level::Warn,
            check: "channels-writable",
            detail: "no channel is writable, so nothing can reply from the phone".to_owned(),
            fix: "mark one channel writable = true (or :rw in VIBE_TALK_CHANNELS) where replies should post".to_owned(),
        });
    }

    if config.live_poll_seconds() == 0 {
        findings.push(Finding {
            level: Level::Warn,
            check: "live-updates",
            detail: "live polling is off, so new messages arrive only when the page is reopened or refreshed".to_owned(),
            fix: "set live_poll_seconds = 30 on the provider (or VIBE_TALK_LIVE_POLL_SECONDS=30) if the page should update by itself".to_owned(),
        });
    }

    // The default read-aloud backend is ElevenLabs; with no agent or key it refuses by name.
    if config.read_aloud.backend == ReadAloudBackend::ElevenLabs
        && config.read_aloud.fallback.is_empty()
        && (config.elevenlabs.agent_id.is_none() || config.elevenlabs.api_key.is_none())
    {
        findings.push(Finding {
            level: Level::Warn,
            check: "voice",
            detail: "read-aloud uses ElevenLabs but no agent id and API key are both configured, so summaries and reading aloud will refuse".to_owned(),
            fix: "set [read_aloud] backend = \"browser\" to read with the phone's own voice, or configure the ElevenLabs agent id and API key".to_owned(),
        });
    }

    findings
}

fn is_local(url: &str) -> bool {
    let host = url
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .split(['/', ':', '?', '#'])
        .next()
        .unwrap_or("");
    matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1")
}

/// Whether any finding blocks a usable deployment.
#[must_use]
pub fn failed(findings: &[Finding]) -> bool {
    findings.iter().any(|finding| finding.level == Level::Fail)
}

/// Render findings as plain lines for a terminal: `FAIL name: detail Fix: ...`.
#[must_use]
pub fn render(findings: &[Finding]) -> String {
    let mut out = String::new();
    for finding in findings {
        let word = match finding.level {
            Level::Fail => "FAIL",
            Level::Warn => "WARN",
            Level::Ok => "OK",
        };
        out.push_str(&format!(
            "{word} {}: {} Fix: {}\n",
            finding.check, finding.detail, finding.fix
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn config(text: &str) -> Config {
        Config::from_toml_and_env(text, &BTreeMap::new()).expect("valid config")
    }

    const BASE: &str = r#"
[server]
bind = "127.0.0.1:8080"
public_url = "https://vibe.example"

[discord]
bot_token = "a-real-discord-bot-token-value"

[auth]
read_token = "read-token-that-is-long-enough"
write_token = "write-token-that-is-long-enough"

[storage]
path = "/var/lib/vibe-talk/vibe-talk.sqlite3"

[[channels]]
id = "111"
label = "lead"
writable = true
"#;

    #[test]
    fn a_complete_configuration_has_no_failure() {
        let findings = check(&config(BASE));
        assert!(!failed(&findings), "findings: {}", render(&findings));
        assert!(
            findings.iter().any(|f| f.check == "storage" && f.level == Level::Ok),
            "findings: {}",
            render(&findings)
        );
    }

    #[test]
    fn missing_storage_is_a_failure_with_a_fix() {
        let text = BASE.replace("[storage]\npath = \"/var/lib/vibe-talk/vibe-talk.sqlite3\"\n", "");
        let findings = check(&config(&text));
        let finding = findings
            .iter()
            .find(|f| f.check == "storage")
            .expect("a storage finding");
        assert_eq!(finding.level, Level::Fail);
        assert!(finding.fix.contains("[storage]"), "fix: {}", finding.fix);
    }

    #[test]
    fn a_placeholder_provider_token_is_a_failure() {
        let text = BASE.replace("a-real-discord-bot-token-value", "REPLACE-ME");
        let findings = check(&config(&text));
        assert!(
            findings.iter().any(|f| f.check == "provider-token" && f.level == Level::Fail),
            "findings: {}",
            render(&findings)
        );
    }

    #[test]
    fn no_public_address_and_plain_http_bind_are_warnings() {
        let text = BASE
            .replace("public_url = \"https://vibe.example\"\n", "")
            .replace("127.0.0.1:8080", "0.0.0.0:8080");
        let findings = check(&config(&text));
        assert!(
            findings.iter().any(|f| f.check == "public-address" && f.level == Level::Warn),
            "findings: {}",
            render(&findings)
        );
        assert!(
            findings.iter().any(|f| f.check == "bind-exposure" && f.level == Level::Warn),
            "findings: {}",
            render(&findings)
        );
    }

    #[test]
    fn read_only_channels_and_silent_voice_are_warnings() {
        let text = BASE.replace("writable = true", "writable = false");
        let findings = check(&config(&text));
        assert!(
            findings.iter().any(|f| f.check == "channels-writable" && f.level == Level::Warn),
            "findings: {}",
            render(&findings)
        );
        // BASE configures no ElevenLabs credentials and keeps the default read-aloud backend.
        assert!(
            findings.iter().any(|f| f.check == "voice" && f.level == Level::Warn),
            "findings: {}",
            render(&findings)
        );
    }

    #[test]
    fn browser_read_aloud_needs_no_voice_vendor() {
        let text = format!("{BASE}\n[read_aloud]\nbackend = \"browser\"\n");
        let findings = check(&config(&text));
        assert!(
            !findings.iter().any(|f| f.check == "voice" && f.level == Level::Warn),
            "findings: {}",
            render(&findings)
        );
    }
}
