//! `gent-talk` server entry point.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use gent_talk::agent_backend::NoAgentBackend;
use gent_talk::config::{Config, ENV_CONFIG_PATH};
use gent_talk::discord::fake::FakeDiscord;
use gent_talk::discord::http::HttpDiscordClient;
use gent_talk::discord::DiscordClient;
use gent_talk::elevenlabs::http::HttpElevenLabsClient;
use gent_talk::elevenlabs::SignedUrlProvider;
use gent_talk::probe::{self, ENV_SKIP_STARTUP_PROBE};
use gent_talk::retrieval::LexicalRanker;
use gent_talk::state::AppState;

const USAGE: &str = "\
gent-talk — a Discord bridge for a voice agent

USAGE:
    gent-talk [--config PATH] [--fake-discord] [--skip-startup-probe]

OPTIONS:
    --config PATH         configuration file (default: $GENT_TALK_CONFIG, else ./gent-talk.toml)
    --fake-discord        run against an in-memory Discord with seeded messages, for local
                          development
    --skip-startup-probe  do not check at startup that each configured channel is readable.
                          Also settable as GENT_TALK_SKIP_STARTUP_PROBE=1. The skip is logged
                          loudly, because an unchecked channel surfaces later as an empty result
                          that looks like a bug in this server.
    --version             print the version and exit
    --help                print this message and exit

Every secret is read from the configuration file or from the environment. See the project README
for the full list and for the threat model.
";

/// The backlog `--fake-discord` seeds into every configured channel.
///
/// Deliberately long-winded and repetitive, because that is the problem this project exists for:
/// a driver returns to a dozen verbose agent messages and cannot skim them. Two cheerful one-liners
/// would make the digest and the scrollback look like they work when neither had anything to do.
/// Several entries also share vocabulary ("runner", "cache", "token") so that
/// `POST /resolve` is exercised against real competition rather than a single obvious hit.
const SEEDED_BACKLOG: &[(&str, &str)] = &[
    (
        "codex-eng",
        "seeded: starting on the retry-budget work. Branch is codex/retry-budget off integration \
         at 4f21ab0. I'll push as soon as the first commit is coherent.",
    ),
    (
        "codex-eng",
        "seeded: first cut pushed. The budget is per-host rather than per-request, which is the \
         only shape that survives a fan-out; a per-request budget lets N parallel calls each spend \
         the whole allowance. Focused tests pass locally (14 of 14 in the retry module).",
    ),
    (
        "claude-integ",
        "seeded: heads up, the mac runner went offline mid-deploy so the arm64 job never reported. \
         The run shows green at the run level but the job list has one queued job that never \
         started, so that green is not evidence of anything. Re-firing.",
    ),
    (
        "claude-integ",
        "seeded: re-fired. Same runner, same stall. I think the runner pool is down to one mac and \
         it is wedged rather than busy - queue wait is 41 minutes and climbing with nothing \
         executing.",
    ),
    (
        "codex-review",
        "seeded: review of the retry-budget branch. One real finding: the budget is consulted \
         before the jitter is applied, so under contention every caller wakes at the same instant \
         and the budget is spent in one burst. Suggest consulting it after the sleep.",
    ),
    (
        "codex-eng",
        "seeded: good catch, fixed and pushed. Also removed the cache-key rewrite I snuck in - it \
         was unrelated to this branch and it belongs in its own change.",
    ),
    (
        "build-bot",
        "seeded: nightly cache rebuild finished in 22m14s, 3.1 GB written, 0 evictions. Previous \
         night was 24m02s so this is noise, not an improvement.",
    ),
    (
        "build-bot",
        "seeded: WARNING - the token used by the release job expires in 6 days. Rotating it \
         requires the maintainer account; nothing automated can do it.",
    ),
    (
        "claude-integ",
        "seeded: the arm64 job finally reported after the runner was recycled by hand. Green, and \
         this one has a real job list: 11 completed, 0 skipped. Landing the retry-budget branch on \
         that evidence.",
    ),
    (
        "claude-integ",
        "seeded: landed. integration is at 9c07d3e and the tip is green. Branch deleted; the PR is \
         closed as merged rather than closed unmerged, which matters for the changelog script.",
    ),
    (
        "codex-eng",
        "seeded: next up is the cache-key rewrite I pulled out earlier. It is small but it changes \
         a durable format, so it is a sequential change - nobody else should touch the cache \
         version namespace while it is in flight.",
    ),
    (
        "claude-qa",
        "seeded: filed two issues from the overnight sweep. The first is a genuine flake - a test \
         that depends on wall-clock ordering between two spawned processes. The second is not a \
         flake, it is a real ordering bug that only shows up when the machine is loaded, and I \
         would rather we not label it flaky because that is how it gets ignored.",
    ),
];

struct Args {
    config: Option<PathBuf>,
    fake_discord: bool,
    skip_startup_probe: bool,
}

fn parse_args() -> anyhow::Result<Option<Args>> {
    let mut args = Args {
        config: None,
        fake_discord: false,
        skip_startup_probe: false,
    };
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print!("{USAGE}");
                return Ok(None);
            }
            "--version" => {
                println!("gent-talk {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--config" => {
                let value = argv.next().context("--config needs a path")?;
                args.config = Some(PathBuf::from(value));
            }
            "--fake-discord" => args.fake_discord = true,
            "--skip-startup-probe" => args.skip_startup_probe = true,
            other => anyhow::bail!("unrecognized argument {other:?}\n\n{USAGE}"),
        }
    }
    Ok(Some(args))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let Some(args) = parse_args()? else {
        return Ok(());
    };

    // An explicitly named file must exist. A defaulted one need not: a container is often given
    // everything through the environment, and that is a supported deployment rather than an error.
    // It is announced loudly either way, because silently running on a different configuration
    // than the operator believes is the failure mode worth preventing.
    let explicit = args.config.is_some();
    let config_path = args
        .config
        .or_else(|| std::env::var_os(ENV_CONFIG_PATH).map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("gent-talk.toml"));
    let config = if config_path.exists() {
        tracing::info!(path = %config_path.display(), "reading configuration file");
        Config::load(&config_path)
            .with_context(|| format!("loading configuration from {}", config_path.display()))?
    } else if explicit {
        anyhow::bail!(
            "configuration file {} does not exist",
            config_path.display()
        );
    } else {
        tracing::warn!(
            path = %config_path.display(),
            "no configuration file found; taking the ENTIRE configuration from the environment"
        );
        Config::from_toml_and_env("", &std::env::vars().collect())
            .context("assembling configuration from the environment alone")?
    };

    let discord: Arc<dyn DiscordClient> = if args.fake_discord {
        tracing::warn!(
            "--fake-discord: serving an IN-MEMORY Discord. Nothing is read from or posted to a \
             real channel."
        );
        let fake = Arc::new(FakeDiscord::new());
        for channel in &config.channels {
            for (author, content) in SEEDED_BACKLOG {
                fake.seed(&channel.id, author, content);
            }
        }
        fake
    } else {
        Arc::new(HttpDiscordClient::new(&config.discord).context("building the Discord client")?)
    };

    run_startup_probe(discord.as_ref(), &config, args.skip_startup_probe).await?;

    for channel in &config.channels {
        tracing::info!(
            channel = %channel.id,
            label = %channel.label,
            writable = channel.writable,
            "configured channel"
        );
    }
    // Say the ElevenLabs wiring out loud, in both directions. An agent with "Enable
    // Authentication" turned on cannot be reached without a signed URL, and the way that failure
    // presents — a conversation that will not start, from a phone, in a car — is about as far as
    // possible from the setting that causes it.
    match gent_talk::elevenlabs::credentials(&config.elevenlabs) {
        Ok((agent_id, _key)) => tracing::info!(
            agent_id,
            "ElevenLabs configured; GET /api/v1/signed-url will mint signed conversation URLs, \
             and /voice is the page that uses them"
        ),
        Err(error) => tracing::warn!(
            %error,
            "ElevenLabs is NOT fully configured; /api/v1/signed-url will refuse with this exact \
             message rather than handing out an unsigned URL"
        ),
    }
    // Say the MCP endpoint out loud at startup: an agent platform has to be told a URL, and the
    // most common deployment mistake is pointing it at the wrong path and getting a bare 404.
    tracing::info!(
        path = gent_talk::mcp::transport::MCP_PATH,
        protocol = gent_talk::mcp::protocol::PROTOCOL_VERSION,
        public_base_url = config
            .public_base_url
            .as_deref()
            .unwrap_or("(not configured)"),
        "MCP Streamable HTTP endpoint mounted; it requires the same bearer tokens as the API"
    );

    // Say the access log exists, at startup, in the log itself. It is what makes an EMPTY log a
    // finding rather than an ambiguity: a reader who sees no request lines below this banner
    // knows that no client called, rather than wondering whether this server logs requests at all.
    tracing::info!(
        target: "gent_talk::access",
        "per-request access logging is ON at INFO. Every HTTP request, every MCP JSON-RPC \
         message, and every tool call leaves a line below. If you see NO lines below this one, \
         nothing called this server -- that is a finding, not a gap in the log. Tokens are never \
         logged; message content is DEBUG only."
    );

    let bind = config.bind;
    let elevenlabs: Arc<dyn SignedUrlProvider> =
        Arc::new(HttpElevenLabsClient::new().context("building the ElevenLabs client")?);
    let state = AppState {
        config: Arc::new(config),
        discord,
        ranker: Arc::new(LexicalRanker),
        agent: Arc::new(NoAgentBackend),
        elevenlabs,
    };
    let app = gent_talk::http::router(state);

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("binding {bind}"))?;
    tracing::info!(%bind, "gent-talk listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("serving")?;
    Ok(())
}

/// Check at startup that every configured channel can actually be read, and refuse to start if
/// one cannot.
///
/// This is the same posture the configuration already takes toward equal or short tokens: a
/// single-user tool that comes up in a state the operator did not mean is worse than one that
/// refuses and says why. The failure it exists for — a bot that was created, tokened, and
/// configured, but never added to the channel — is invisible at startup today and surfaces later
/// as an empty digest, which reads like a bug in this code.
async fn run_startup_probe(
    discord: &dyn DiscordClient,
    config: &Config,
    skip_flag: bool,
) -> anyhow::Result<()> {
    let skip_env = std::env::var(ENV_SKIP_STARTUP_PROBE).ok();
    match probe::startup_check(discord, &config.channels, skip_flag, skip_env.as_deref()).await {
        // Loudly, and naming which switch did it: a skipped check that says nothing is the same
        // silent start this probe was added to remove.
        probe::StartupCheck::Skipped { source } => {
            tracing::warn!(
                source,
                "SKIPPING the startup channel probe. This server has NOT checked that its bot can \
                 read the configured channels; if it cannot, reads will come back empty rather \
                 than failing."
            );
            eprintln!(
                "WARNING: startup channel probe SKIPPED via {source}; channel reachability is \
                 unverified."
            );
        }
        probe::StartupCheck::Passed(report) => {
            if report.warnings().is_empty() {
                tracing::info!("{}", report.render());
            } else {
                tracing::warn!("{}", report.render());
            }
        }
        probe::StartupCheck::Failed(report) => {
            let failed = report.failures().len().max(usize::from(report.aborted));
            // The detail goes to stderr rather than through `tracing`, because RUST_LOG is an
            // operator-settable filter and a startup refusal must not be suppressible by it. One
            // structured line still goes to the log, so `podman logs` shows why the container
            // exited without repeating the whole report twice.
            tracing::error!(
                failed,
                configured = config.channels.len(),
                "startup channel probe FAILED; the per-channel diagnosis is on stderr"
            );
            eprintln!("{}", report.render());
            anyhow::bail!(
                "{failed} of {} configured channel(s) could not be read; refusing to start. Fix \
                 the configuration above, or re-run with {} if you know the check is wrong.",
                config.channels.len(),
                probe::SKIP_FLAG
            );
        }
    }
    Ok(())
}

async fn shutdown_signal() {
    // A container stops with SIGTERM, not SIGINT. Handling only ctrl-c means every `podman stop`
    // waits out its ten-second grace period and then SIGKILLs the process, which is both slow and
    // a lie about whether shutdown was clean.
    let interrupt = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "could not install the interrupt handler");
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut stream) => {
                stream.recv().await;
            }
            Err(error) => {
                tracing::error!(%error, "could not install the SIGTERM handler");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = interrupt => tracing::info!("interrupted; shutting down"),
        () = terminate => tracing::info!("terminated; shutting down"),
    }
}
