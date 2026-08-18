//! `gent-talk` server entry point.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context as _;
use gent_talk::agent_backend::NoAgentBackend;
use gent_talk::config::{Config, ENV_CONFIG_PATH};
use gent_talk::discord::fake::FakeDiscord;
use gent_talk::discord::http::HttpDiscordClient;
use gent_talk::discord::DiscordClient;
use gent_talk::retrieval::LexicalRanker;
use gent_talk::state::AppState;

const USAGE: &str = "\
gent-talk — a Discord bridge for a voice agent

USAGE:
    gent-talk [--config PATH] [--fake-discord]

OPTIONS:
    --config PATH    configuration file (default: $GENT_TALK_CONFIG, else ./gent-talk.toml)
    --fake-discord   run against an in-memory Discord with seeded messages, for local development
    --version        print the version and exit
    --help           print this message and exit

Every secret is read from the configuration file or from the environment. See the project README
for the full list and for the threat model.
";

struct Args {
    config: Option<PathBuf>,
    fake_discord: bool,
}

fn parse_args() -> anyhow::Result<Option<Args>> {
    let mut args = Args {
        config: None,
        fake_discord: false,
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
            fake.seed(
                &channel.id,
                "codex-eng",
                "seeded: the release build finished green.",
            );
            fake.seed(
                &channel.id,
                "codex-integ",
                "seeded: the mac runner went offline mid-deploy so the arm64 job never reported.",
            );
        }
        fake
    } else {
        Arc::new(HttpDiscordClient::new(&config.discord).context("building the Discord client")?)
    };

    for channel in &config.channels {
        tracing::info!(
            channel = %channel.id,
            label = %channel.label,
            writable = channel.writable,
            "configured channel"
        );
    }
    if config.elevenlabs.agent_id.is_none() {
        tracing::info!(
            "no ElevenLabs agent configured; the API is reachable but nothing calls it yet"
        );
    }

    let bind = config.bind;
    let state = AppState {
        config: Arc::new(config),
        discord,
        ranker: Arc::new(LexicalRanker),
        agent: Arc::new(NoAgentBackend),
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
