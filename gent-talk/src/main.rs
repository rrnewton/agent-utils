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
use gent_talk::elevenlabs::{SignedUrlProvider, SpeechProvider};
use gent_talk::probe::{self, ENV_SKIP_STARTUP_PROBE};
use gent_talk::retrieval::LexicalRanker;
use gent_talk::state::AppState;
use gent_talk::store::disabled::DisabledStore;
use gent_talk::store::sqlite::SqliteStore;
use gent_talk::store::StateStore;

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
    // OVER `summaries.threshold_chars`, which is 400 by default, and it is the only entry here
    // that is. Everything above is between 140 and 340 characters, so against this backlog every
    // summary the page asked for came back `below_threshold` and `#49 cached-summaries` could not
    // be exercised locally AT ALL — the seam existed, the endpoint answered, and nothing a
    // developer could see ever produced a summary. It is also the honest case: the message this
    // whole project exists for is the one you cannot skim at the roadside.
    (
        "codex-eng",
        "seeded: long one, sorry. The cache-key rewrite is done and I want to write down what \
         changed before I forget it. The key used to be the source path plus the compiler \
         version, which looked complete and was not: two builds of the same file with different \
         feature flags hashed identically, so the second one silently reused the first one's \
         object and the flags did nothing. That is the bug behind the three 'it works on my \
         machine' reports from last month, and it explains why clearing the cache always fixed \
         them. The new key folds in the feature set, the target triple and the optimisation \
         level, and it is versioned, so the moment any of that changes every old entry becomes \
         unreachable at once rather than being served under rules that no longer exist. I did \
         NOT make it a cryptographic hash - it is a change detector, not a defence, and calling \
         it a defence would invite somebody to rely on it as one. The migration is a no-op: old \
         entries are simply never hit again and the retention sweep collects them within the \
         week. One thing I want a second opinion on before this lands: the optimisation level is \
         in the key, which means a debug build and a release build no longer share anything, and \
         on this repository that roughly doubles the cache on disk.",
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
            let mut seeded = Vec::new();
            for (author, content) in SEEDED_BACKLOG {
                seeded.push(fake.seed(&channel.id, author, content));
            }
            // A couple of the seeded messages ANSWER earlier ones, because a channel in which
            // nothing has been replied to cannot exercise the inbox view at all: every row would
            // be open, and "replied dims the row" would look identical to "the feature is not
            // wired up". Indices are into SEEDED_BACKLOG and chosen where the text really does
            // read as a reply to the earlier message.
            for (answer, question) in [(5_usize, 4_usize), (9, 3)] {
                if let (Some(a), Some(q)) = (seeded.get(answer), seeded.get(question)) {
                    fake.set_reply_to(a, q);
                }
            }
        }
        fake
    } else {
        Arc::new(HttpDiscordClient::new(&config.discord).context("building the Discord client")?)
    };

    // Open the store BEFORE the probe and before the listener: a storage path that cannot be
    // created is a configuration error, and a server that came up and only discovered it on the
    // first attempt to record a turn would lose that turn and say nothing useful about why.
    let store: Arc<dyn StateStore> = match &config.storage.path {
        Some(path) => {
            let opened = SqliteStore::open(path, config.storage.retention).with_context(|| {
                format!(
                    "opening the durable state store at {}. It must be an absolute path this \
                     process can write to — in a container, a mounted VOLUME rather than a \
                     directory in the image.",
                    path.display()
                )
            })?;
            let retention = config.storage.retention;
            tracing::info!(
                path = %path.display(),
                max_conversations = retention.max_conversations,
                max_turns_per_conversation = retention.max_turns_per_conversation,
                max_summaries = retention.max_summaries,
                retain_days = retention.retain_days,
                "durable state is ON. Conversation transcripts and read marks are written to \
                 this file at 0600 and survive a restart. Read marks are THIS SERVER'S OWN: \
                 nothing is read from or written back to Discord."
            );
            Arc::new(opened)
        }
        None => {
            tracing::warn!(
                setting = gent_talk::store::disabled::SETTING,
                "durable state is OFF. This server keeps NOTHING between restarts, and the \
                 routes that need it refuse with 503 storage_not_configured rather than \
                 pretending. Set an absolute path on a mounted volume to turn it on."
            );
            Arc::new(DisabledStore)
        }
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
    // Say the zone out loud. Leaving `server.timezone` unset is not an error and must not be, but
    // it means every time the agent speaks is UTC — and a wrong-but-plausible clock is precisely
    // the failure this setting exists to prevent, so it should not have to be inferred from a
    // digest read out in the car.
    tracing::info!(
        timezone = config.timezone.name(),
        "message times will be spoken in this zone"
    );
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

    // ONE client, three capabilities. Building a second would mean two connection pools and two
    // user agents against the same vendor for no reason. Built here, before the summariser,
    // because the agent-backed summariser mints its conversations through this same client.
    let elevenlabs_client =
        Arc::new(HttpElevenLabsClient::new().context("building the ElevenLabs client")?);

    // Say WHICH summariser is running, and under which policy version. A page that reported
    // "summarised" without this could imply a model answer produced by truncation, and the
    // version is what a person with a shell greps the cache by when they suspect a stale entry.
    let summarizer: Arc<dyn gent_talk::summarize::Summarizer> = match config.summaries.backend {
        gent_talk::summarize::Backend::Extractive => {
            Arc::new(gent_talk::summarize::extractive::ExtractiveSummarizer)
        }
        gent_talk::summarize::Backend::ElevenLabsAgent => {
            // Selected but unconfigured is a warning here and a per-request refusal there, not a
            // refusal to start: the Discord half of this server is useful on its own, and a
            // deployment that mistyped a key should still come up far enough to say so.
            if let Err(error) = gent_talk::elevenlabs::credentials(&config.elevenlabs) {
                tracing::warn!(
                    %error,
                    "summaries.backend selects the ElevenLabs agent, but ElevenLabs is not fully \
                     configured; every summary request will refuse with this exact message"
                );
            }
            Arc::new(gent_talk::summarize::agent::AgentSummarizer::new(
                Arc::new(
                    gent_talk::elevenlabs::socket::WebSocketTextChatProvider::new(Arc::clone(
                        &elevenlabs_client,
                    )
                        as Arc<dyn SignedUrlProvider>),
                ),
                config.elevenlabs.clone(),
                gent_talk::summarize::agent::PoolPolicy::from_config(&config.summaries),
            ))
        }
    };
    let summary_version =
        gent_talk::summarize::policy_version_for(&config.summaries, summarizer.as_ref());
    tracing::info!(
        backend = summarizer.describe(),
        version = summary_version,
        threshold_chars = config.summaries.threshold_chars,
        target_chars = config.summaries.target_chars,
        "summaries are produced by this backend under this policy version; every cached summary \
         is filed under the version, so changing any summary setting makes the old ones \
         unreachable at once"
    );
    // The sweep. Without it a changed policy leaves the old entries on disk forever: unreachable,
    // invisible, and still a copy of other people's text at rest.
    match store.forget_summaries_except(&summary_version).await {
        Ok(0) => {}
        Ok(swept) => tracing::info!(
            swept,
            "deleted cached summaries produced under an older policy"
        ),
        // Not configured is not a failure: there is no cache, so there is nothing to sweep. A
        // store that IS configured and could not be swept is a real fault and has to be visible
        // at the default RUST_LOG=info, because what it leaves behind is other people's text at
        // rest under a policy that no longer applies.
        Err(gent_talk::store::StoreError::Unavailable(_)) => {}
        Err(error) => tracing::warn!(
            %error,
            "the summary cache could NOT be swept; entries produced under an older policy are \
             still on disk"
        ),
    }

    // `#44 live-push`. Say the ingestion posture out loud in both directions, for the same reason
    // the MCP path and the access log are announced: a page waiting on a stream that nobody
    // publishes to is indistinguishable from a quiet channel, and the setting that causes it is
    // one line in a file the operator is not looking at.
    let live = Arc::new(gent_talk::live::LiveHub::new());
    let poll_seconds = config.discord.live_poll_seconds;
    if poll_seconds == 0 {
        tracing::info!(
            setting = "discord.live_poll_seconds",
            "live ingestion is OFF. GET /api/v1/channels/{{id}}/stream accepts subscribers and \
             nothing publishes to them, so the page falls back to its own timed re-read. Set an \
             interval to turn it on; every tick is one Discord request per channel."
        );
    } else {
        tracing::info!(
            interval_seconds = poll_seconds,
            channels = config.channels.len(),
            "live ingestion is ON: each configured channel is polled on this interval and what is \
             new is pushed to GET /api/v1/channels/{{id}}/stream. This is POLLING, not a Discord \
             Gateway connection -- see the module doc of src/live.rs for why -- and it spends one \
             Discord request per channel per tick against rate limits this server does not handle."
        );
    }

    let bind = config.bind;
    let speech: Arc<dyn SpeechProvider> = Arc::clone(&elevenlabs_client) as Arc<dyn SpeechProvider>;
    let elevenlabs: Arc<dyn SignedUrlProvider> = elevenlabs_client;
    let live_channels: Vec<_> = config.channels.iter().map(|c| c.id.clone()).collect();
    let live_limit = config.discord.default_fetch_limit;
    let state = AppState {
        config: Arc::new(config),
        discord: Arc::clone(&discord),
        ranker: Arc::new(LexicalRanker),
        agent: Arc::new(NoAgentBackend),
        elevenlabs,
        speech,
        store,
        live: Arc::clone(&live),
        summarizer,
        summary_version: summary_version.into(),
    };
    if poll_seconds > 0 {
        tokio::spawn(gent_talk::live::poll_forever(
            discord,
            live,
            live_channels,
            live_limit,
            std::time::Duration::from_secs(poll_seconds),
        ));
    }
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
