//! The startup channel probe, exercised through the actual binary.
//!
//! The unit tests in `src/probe.rs` prove the probe classifies correctly. They cannot prove
//! anybody calls it — a probe that exists and is never invoked is the exact silent no-op this
//! feature was added to remove, and it would leave every unit test green. So these tests run the
//! real `vibe-talk` binary, against a Discord that is not there, and check what it does.

use std::io::{BufRead as _, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// A configuration whose Discord is a closed port, so every read fails at the transport.
///
/// Port 1 is reserved and nothing listens on it, which makes "the request never reached Discord"
/// the deterministic answer rather than a timing accident.
///
/// **There is deliberately no `[elevenlabs]` section.** That is not an omission for brevity: it
/// is the bare deployment, and every test below runs against it, so a change that made the
/// ElevenLabs summariser a startup requirement would take the whole file red rather than one
/// assertion.
fn config_text() -> String {
    r#"
[server]
bind = "127.0.0.1:0"

[discord]
bot_token = "startup-probe-test-bot-token"
api_base = "http://127.0.0.1:1"

[auth]
read_token = "startup-read-token-000000000000"
write_token = "startup-write-token-00000000000"

[[channels]]
id = "1111111111"
label = "lead team"
writable = true

[[channels]]
id = "2222222222"
label = "build noise"
writable = false
"#
    .to_owned()
}

fn write_config(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("vibe-talk-startup-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let path = dir.join("vibe-talk.toml");
    std::fs::write(&path, config_text()).expect("write config");
    path
}

/// The binary under test, built by cargo for this integration test.
fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_vibe-talk")
}

/// Wait up to `limit` for the child to exit, killing it if it does not.
fn wait_for_exit(
    child: &mut std::process::Child,
    limit: Duration,
) -> Option<std::process::ExitStatus> {
    let deadline = std::time::Instant::now() + limit;
    loop {
        match child.try_wait().expect("polling the child") {
            Some(status) => return Some(status),
            None if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }
}

/// Everything the child printed, both streams together.
fn collect_output(child: std::process::Child) -> String {
    let out = child.wait_with_output().expect("collecting output");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn an_unreachable_channel_stops_the_server_from_starting() {
    let config = write_config("fails");
    let mut child = Command::new(binary())
        .arg("--config")
        .arg(&config)
        // Make sure an ambient value on the developer's machine cannot turn this check off and
        // silently pass the test.
        .env_remove("VIBE_TALK_SKIP_STARTUP_PROBE")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawning the binary");

    // Bounded, because the interesting regression is a probe that passes when it should not — and
    // then this server does exactly what it is built to do and serves forever. Waiting on exit
    // without a deadline would turn that regression into a hung suite instead of a red one.
    let status = wait_for_exit(&mut child, Duration::from_secs(20));
    let output = collect_output(child);
    let status = status.unwrap_or_else(|| {
        panic!(
            "the server did not exit at all: it must REFUSE to start when a configured channel \
             cannot be read, and instead it kept running.\n{output}"
        )
    });

    assert!(
        !status.success(),
        "the server must REFUSE to start when a configured channel cannot be read; it exited \
         {status:?}\n{output}"
    );
    let text = output;
    assert!(
        text.contains("FAILED"),
        "no failure marker in output:\n{text}"
    );
    assert!(
        text.contains("1111111111") && text.contains("lead team"),
        "every failing channel must be named by id AND by the label the operator wrote:\n{text}"
    );
    // Shared diagnostic vocabulary states the cause; the detail identifies the configured
    // endpoint, which may be a bridge rather than the provider's public API.
    assert!(
        text.contains("never reached the vendor"),
        "the cause must be stated, not collapsed into 'unreachable':\n{text}"
    );
    assert!(
        text.contains("127.0.0.1:1"),
        "the detail must say which configured host could not be reached:\n{text}"
    );
    assert!(
        text.contains("refusing to start"),
        "the refusal must say it is a refusal:\n{text}"
    );
}

#[test]
fn the_skip_switch_starts_the_server_anyway_and_says_so() {
    let config = write_config("skips");
    let mut child = Command::new(binary())
        .arg("--config")
        .arg(&config)
        .env("VIBE_TALK_SKIP_STARTUP_PROBE", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawning the binary");

    // Read both streams: the structured log goes to stdout, the operator-facing warning to stderr.
    let (tx, rx) = mpsc::channel::<String>();
    for stream in [
        Box::new(child.stdout.take().expect("stdout")) as Box<dyn std::io::Read + Send>,
        Box::new(child.stderr.take().expect("stderr")),
    ] {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
    }
    drop(tx);

    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut seen = String::new();
    let mut warned = false;
    let mut listening = false;
    while std::time::Instant::now() < deadline && !(warned && listening) {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(line) => {
                if line.contains("SKIPPING the startup channel probe")
                    || line.contains("startup channel probe SKIPPED")
                {
                    warned = true;
                }
                if line.contains("vibe-talk listening") {
                    listening = true;
                }
                seen.push_str(&line);
                seen.push('\n');
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();

    assert!(
        listening,
        "with the probe skipped the server must start despite an unreachable Discord:\n{seen}"
    );
    assert!(
        warned,
        "a skipped probe must announce itself; an invisible skip is the silent start this \
         feature exists to prevent:\n{seen}"
    );
}

/// `#343 pwa-app-name`. An app name the server cannot use stops it before it serves anything, with
/// the key named; one it can use is said out loud at startup.
#[test]
fn an_unusable_app_name_stops_the_server_and_a_usable_one_is_announced() {
    let config = write_config("app-name");
    let mut child = Command::new(binary())
        .arg("--config")
        .arg(&config)
        .env("VIBE_TALK_APP_NAME", "work\nchat")
        .env("VIBE_TALK_SKIP_STARTUP_PROBE", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawning the binary");
    let status = wait_for_exit(&mut child, Duration::from_secs(20));
    let output = collect_output(child);
    let status = status.unwrap_or_else(|| {
        panic!("the server kept running with a line break in its app name:\n{output}")
    });
    assert!(
        !status.success(),
        "an unusable app name must stop the server; it exited {status:?}\n{output}"
    );
    assert!(
        output.contains("server.app_name is invalid") && output.contains("U+000A"),
        "the refusal must name the key and the character:\n{output}"
    );

    let seen = run_until_listening(
        &config,
        &[
            ("VIBE_TALK_APP_NAME", "vibe-talk at work".to_owned()),
            ("VIBE_TALK_APP_SHORT_NAME", "Work".to_owned()),
        ],
    );
    let announced = seen
        .lines()
        .find(|line| line.contains("the web app is installed under this name"))
        .unwrap_or_else(|| panic!("the app's name was not announced:\n{seen}"));
    assert!(
        announced.contains("vibe-talk at work") && announced.contains("Work"),
        "{announced}"
    );
}

/// Run the binary until it says it is listening, then stop it. Returns everything it printed.
///
/// Shared by the sweep test below and nothing else yet: what it is for is asserting on something
/// the server does BEFORE it serves, which needs the process to have got all the way up.
fn run_until_listening(config: &std::path::Path, extra_env: &[(&str, String)]) -> String {
    let mut command = Command::new(binary());
    command
        .arg("--config")
        .arg(config)
        .env("VIBE_TALK_SKIP_STARTUP_PROBE", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in extra_env {
        command.env(name, value);
    }
    let mut child = command.spawn().expect("spawning the binary");

    let (tx, rx) = mpsc::channel::<String>();
    for stream in [
        Box::new(child.stdout.take().expect("stdout")) as Box<dyn std::io::Read + Send>,
        Box::new(child.stderr.take().expect("stderr")),
    ] {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
    }
    drop(tx);

    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let mut seen = String::new();
    let mut listening = false;
    while std::time::Instant::now() < deadline && !listening {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(line) => {
                listening = line.contains("vibe-talk listening");
                seen.push_str(&line);
                seen.push('\n');
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(listening, "the server never came up:\n{seen}");
    seen
}

#[tokio::test]
async fn starting_the_server_sweeps_summaries_from_a_policy_that_no_longer_applies() {
    // The sweep is implemented in the store and tested there. What NOTHING tested is that anybody
    // calls it: deleting the call in `main` left every other test green, which is the same silent
    // no-op the startup probe's own tests exist to prevent. So this runs the real binary against a
    // real file and looks at what is left in it afterwards.
    use vibe_talk::store::{StateStore as _, SummaryKey};

    let config = write_config("sweeps");
    let file = config
        .parent()
        .expect("temp dir")
        .join("sweep-state.sqlite3");
    let _ = std::fs::remove_file(&file);
    // Built the way `main` builds it — `summarizer_for` plus `policy_version_for` — and NOT as
    // `policy_version(config, slug)`. The binary's key folds the agent's own policy input (the
    // plain-text rule and `agent=(unset)`, since this configuration names no agent), so a bare
    // slug would compile, would sweep the "still valid" control away, and would fail as though
    // the sweep were too aggressive rather than as though the test had computed a key the binary
    // never writes.
    let summarizer = vibe_talk::summarize::summarizer_for(
        &vibe_talk::config::ElevenLabsConfig::default(),
        &vibe_talk::config::SummariesConfig::default(),
        std::sync::Arc::new(
            vibe_talk::elevenlabs::socket::WebSocketTextChatProvider::new(std::sync::Arc::new(
                vibe_talk::elevenlabs::http::HttpElevenLabsClient::new().expect("client"),
            )
                as std::sync::Arc<dyn vibe_talk::elevenlabs::SignedUrlProvider>),
        ),
    );
    let current = vibe_talk::summarize::policy_version_for(
        &vibe_talk::config::SummariesConfig::default(),
        summarizer.as_ref(),
    );
    let key = |version: &str| SummaryKey {
        channel: vibe_talk::model::ChannelId("1111111111".to_owned()),
        message: vibe_talk::model::MessageId("1000000000000000200".to_owned()),
        content_hash: 7,
        version: version.to_owned(),
    };
    {
        let store = vibe_talk::store::sqlite::SqliteStore::open(
            &file,
            vibe_talk::store::Retention::default(),
        )
        .expect("open");
        // KEPT as `v1-extractive-…` on purpose, and it is the one deliberate mention of the
        // deleted backend left in this tree. It is precisely the key that backend wrote, so this
        // test is an upgrade rehearsal: a real database carried across this change holds entries
        // under exactly this prefix, and what has to happen to them is that they go.
        store
            .cache_summary(
                &key("v1-extractive-w3-c160-0000000000000000"),
                "stale",
                None,
            )
            .await
            .expect("cache");
        store
            .cache_summary(&key(&current), "still valid", None)
            .await
            .expect("cache");
    }

    let output = run_until_listening(
        &config,
        &[(
            "VIBE_TALK_STORAGE_PATH",
            file.to_string_lossy().into_owned(),
        )],
    );

    let store =
        vibe_talk::store::sqlite::SqliteStore::open(&file, vibe_talk::store::Retention::default())
            .expect("reopen");
    assert_eq!(
        store
            .cached_summary(&key("v1-extractive-w3-c160-0000000000000000"))
            .await
            .expect("read"),
        None,
        "the entry from a policy that no longer applies survived a start, so nothing calls the \
         sweep:\n{output}"
    );
    // The control: the sweep is not simply a table drop. An entry under the CURRENT policy is
    // what the cache is for, and a start that emptied it would make the cache useless while
    // passing the assertion above.
    assert_eq!(
        store
            .cached_summary(&key(&current))
            .await
            .expect("read")
            .map(|hit| hit.text)
            .as_deref(),
        Some("still valid"),
        "the start swept away the entries it was supposed to keep:\n{output}"
    );
    let _ = std::fs::remove_file(&file);
}

/// The child's output with ANSI escapes removed.
///
/// The server colours its log only when it writes to a terminal (`#490 storage-io-audit`), so a
/// pipe gets none. This stays for a run under a pseudo-terminal, where the colouring lands
/// BETWEEN a structured field's name and its value: `summaries_available` `ESC[0m` `ESC[2m` `=`
/// `ESC[0m` `false`. So a naive `contains("name=value")` reads as absent when the field is
/// present — and, far worse, passes on any PROSE in the same log that happens to spell the pair
/// out, which is a test asserting on a sentence while believing it asserts on a value. Both
/// halves of that really happened here.
fn without_colour(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        // CSI: `ESC [ <params> <final byte in @..~>`. Anything else escape-ish is dropped whole,
        // which is fine for a log nothing but a person reads.
        if chars.next() == Some('[') {
            for c in chars.by_ref() {
                if ('\u{40}'..='\u{7e}').contains(&c) {
                    break;
                }
            }
        }
    }
    out
}

#[test]
fn a_deployment_with_no_elevenlabs_still_boots_and_says_summaries_will_fail() {
    // The regression test for the whole shape of this change. There is now exactly one
    // summariser and it needs a vendor, so the obvious way to implement "the agent is the
    // default" is to require credentials at load — which would take every deployment that has
    // only wired up Discord off the air, permanently, on an upgrade. `config_text()` has no
    // `[elevenlabs]` section at all, and this asserts the server comes all the way up anyway.
    //
    // The second half is the reason it is not enough to assert it merely boots. A server that
    // starts and then fails every summary in silence is the worst of the three outcomes: the
    // reader sees red rows and has nothing to connect them to. So the banner has to be there,
    // it has to say the summaries will fail, and it has to name the settings that fix it.
    let config = write_config("no-elevenlabs");
    let output = without_colour(&run_until_listening(&config, &[]));

    assert!(
        output.contains("vibe-talk listening"),
        "a deployment with no ElevenLabs credentials must still serve Discord:\n{output}"
    );
    assert!(
        output.contains("EVERY summary will fail"),
        "the server came up without saying that nothing can be summarised; the reader would see \
         red rows with nothing to connect them to:\n{output}"
    );
    assert!(
        output.contains("elevenlabs.api_key") && output.contains("elevenlabs.agent_id"),
        "the banner has to name BOTH settings that turn summaries on:\n{output}"
    );
    assert!(
        output.contains("summaries_available=false"),
        "the backend line itself has to carry the verdict, so a reader who greps for it does not \
         also have to find the warning above it:\n{output}"
    );
}

#[tokio::test]
async fn the_first_start_re_keys_summaries_and_no_later_start_asks_for_one_again() {
    // `#208 persistent-cache`, milestone `summary-keep`. A file written by the previous build holds
    // summaries under `policy_version_v1`, which hashed the threshold, the deadline and the cap.
    // The first start moves them to the wording-only version instead of deleting them, marks the
    // ones that were cut, and sweeps only an older wording. Every later start finds nothing to do —
    // including one whose threshold and deadline changed, which used to sweep everything.
    use vibe_talk::store::{StateStore as _, SummaryKey};

    let dir = std::env::temp_dir().join(format!("vibe-talk-startup-{}-rekey", std::process::id()));
    private_dir(&dir);
    let file = dir.join("rekey-state.sqlite3");
    let _ = std::fs::remove_file(&file);
    // `[message_cache]` rides along: a cache is opened beside the store, and changes nothing the
    // store holds.
    let config = dir.join("vibe-talk.toml");
    std::fs::write(
        &config,
        config_text() + "\n[message_cache]\nenabled = true\nmax_days = 14\n",
    )
    .expect("write config");
    let summaries = vibe_talk::config::SummariesConfig::default();
    let summarizer = vibe_talk::summarize::summarizer_for(
        &vibe_talk::config::ElevenLabsConfig::default(),
        &summaries,
        std::sync::Arc::new(
            vibe_talk::elevenlabs::socket::WebSocketTextChatProvider::new(std::sync::Arc::new(
                vibe_talk::elevenlabs::http::HttpElevenLabsClient::new().expect("client"),
            )
                as std::sync::Arc<dyn vibe_talk::elevenlabs::SignedUrlProvider>),
        ),
    );
    let previous = vibe_talk::summarize::policy_version_v1_for(&summaries, summarizer.as_ref());
    let current = vibe_talk::summarize::policy_version_for(&summaries, summarizer.as_ref());
    assert_ne!(previous, current);
    let key = |message: &str, version: &str| SummaryKey {
        channel: vibe_talk::model::ChannelId("1111111111".to_owned()),
        message: vibe_talk::model::MessageId(message.to_owned()),
        content_hash: 7,
        version: version.to_owned(),
    };
    let rows = [
        (
            "1000000000000000300",
            previous.as_str(),
            "WHOLE-ANSWER-TEXT.",
        ),
        ("1000000000000000301", previous.as_str(), "CUT-ANSWER-TEXT…"),
        (
            "thread-sentence:spaces/A/threads/one",
            previous.as_str(),
            r#"{"summary":"CUT-SENTENCE-TEXT…","at":3}"#,
        ),
        (
            "thread-name:spaces/A/threads/one",
            vibe_talk::store::KEPT_VERSION,
            r#"{"name":"runner-wedged","touched":1}"#,
        ),
        (
            "1000000000000000302",
            "v1-extractive-w3-c160-0000000000000000",
            "OLD-WORDING-TEXT",
        ),
    ];
    {
        let store = vibe_talk::store::sqlite::SqliteStore::open(
            &file,
            vibe_talk::store::Retention::default(),
        )
        .expect("open");
        for (message, version, text) in rows {
            store
                .cache_summary(&key(message, version), text, None)
                .await
                .expect("cache");
        }
    }
    let storage = [(
        "VIBE_TALK_STORAGE_PATH",
        file.to_string_lossy().into_owned(),
    )];

    let first = without_colour(&run_until_listening(&config, &storage));
    assert!(
        first.contains("re-keyed=3") && first.contains("swept=1"),
        "the first start did not re-key the previous version's rows and sweep only the older \
         wording:\n{first}"
    );
    assert!(
        first.contains("the message cache is open"),
        "the table's cache was not opened:\n{first}"
    );
    // Numbers only: no summary text, no message id, no content hash, no key.
    for secret in [
        "WHOLE-ANSWER-TEXT",
        "CUT-ANSWER-TEXT",
        "CUT-SENTENCE-TEXT",
        "OLD-WORDING-TEXT",
        "1000000000000000301",
        "spaces/A/threads/one",
        "0000000000000007",
    ] {
        assert!(
            !first.contains(secret),
            "{secret} reached the startup log:\n{first}"
        );
    }
    async fn held(
        store: &vibe_talk::store::sqlite::SqliteStore,
        message: &str,
        version: &str,
    ) -> Option<vibe_talk::store::CachedSummary> {
        store
            .cached_summary(&SummaryKey {
                channel: vibe_talk::model::ChannelId("1111111111".to_owned()),
                message: vibe_talk::model::MessageId(message.to_owned()),
                content_hash: 7,
                version: version.to_owned(),
            })
            .await
            .expect("read")
    }
    {
        let store = vibe_talk::store::sqlite::SqliteStore::open(
            &file,
            vibe_talk::store::Retention::default(),
        )
        .expect("reopen");
        let whole = held(&store, "1000000000000000300", &current)
            .await
            .expect("re-keyed");
        assert_eq!(
            (whole.text.as_str(), whole.cut_at),
            ("WHOLE-ANSWER-TEXT.", None)
        );
        let cut_at = Some(
            u32::try_from(vibe_talk::summarize::hard_cap_chars(summaries.target_chars))
                .expect("small"),
        );
        let cut = held(&store, "1000000000000000301", &current)
            .await
            .expect("re-keyed");
        assert_eq!(
            cut.cut_at, cut_at,
            "a cut answer was not marked as cut at the old cap"
        );
        let sentence = held(&store, "thread-sentence:spaces/A/threads/one", &current)
            .await
            .expect("re-keyed");
        assert_eq!(sentence.cut_at, cut_at);
        assert!(held(
            &store,
            "thread-name:spaces/A/threads/one",
            vibe_talk::store::KEPT_VERSION
        )
        .await
        .is_some());
        assert!(held(
            &store,
            "1000000000000000302",
            "v1-extractive-w3-c160-0000000000000000"
        )
        .await
        .is_none());
        assert!(held(&store, "1000000000000000300", &previous)
            .await
            .is_none());
    }

    // A later start with only a setting that changes no word of a summary moved: the threshold
    // and the reply deadline. Nothing is re-keyed, nothing is swept, every summary is still there.
    std::fs::write(
        &config,
        config_text() + "\n[summaries]\nthreshold_chars = 250\nreply_timeout_seconds = 45\n",
    )
    .expect("rewrite config");
    let second = without_colour(&run_until_listening(&config, &storage));
    assert!(
        second.contains("re-keyed=0") && second.contains("swept=0"),
        "a restart with only the threshold and deadline changed re-keyed or swept something:\n\
         {second}"
    );
    // The table is gone, so the cache the first start made is left, unused, and said to be.
    assert!(
        !second.contains("the message cache is open") && second.contains("present and unused"),
        "{second}"
    );
    let store =
        vibe_talk::store::sqlite::SqliteStore::open(&file, vibe_talk::store::Retention::default())
            .expect("reopen");
    for message in [
        "1000000000000000300",
        "1000000000000000301",
        "thread-sentence:spaces/A/threads/one",
    ] {
        assert!(
            held(&store, message, &current).await.is_some(),
            "{message} did not survive a restart:\n{second}"
        );
    }
    let _ = std::fs::remove_file(&file);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A fresh directory only this account can enter, as a store's directory is and as the message
/// cache requires of the one it is created in.
fn private_dir(dir: &std::path::Path) {
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir).expect("temp dir");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).expect("private");
    }
}

#[test]
fn the_binary_opens_the_message_cache_its_table_enables_and_erases_it_when_disabled() {
    // `#208 persistent-cache`. The library's startup decisions are tested against the library;
    // this proves the binary makes them, in both directions, before it serves anything.
    let dir = std::env::temp_dir().join(format!("vibe-talk-startup-{}-cache", std::process::id()));
    private_dir(&dir);
    let store = dir.join("cache-state.sqlite3");
    let storage = [(
        "VIBE_TALK_STORAGE_PATH",
        store.to_string_lossy().into_owned(),
    )];
    let config = dir.join("vibe-talk.toml");
    let cache = vibe_talk::cache::file::default_path(&store, &vibe_talk::cache::file::host_name());

    std::fs::write(
        &config,
        config_text() + "\n[message_cache]\nenabled = true\nmax_days = 14\n",
    )
    .expect("write config");
    let on = without_colour(&run_until_listening(&config, &storage));
    assert!(on.contains("the message cache is open"), "{on}");
    assert!(
        cache.exists(),
        "no cache file at {}:\n{on}",
        cache.display()
    );
    let sidecar = vibe_talk::cache::marker::sidecar_path(&store);
    assert_eq!(
        vibe_talk::cache::marker::load(&sidecar)
            .expect("the sidecar")
            .paths,
        vec![cache.clone()]
    );

    std::fs::write(
        &config,
        config_text() + "\n[message_cache]\nenabled = false\n",
    )
    .expect("rewrite config");
    let off = without_colour(&run_until_listening(&config, &storage));
    assert!(
        off.contains("every message cache file this server made has been erased"),
        "{off}"
    );
    assert!(!off.contains("the message cache is open"), "{off}");
    assert!(
        !cache.exists(),
        "a disabled cache survived the start:\n{off}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// `#718 vt-startup-probe-restart`. A Discord stand-in on an ephemeral local port: every message
/// read is answered by `messages`, which is given how many reads came before it; a channel object
/// by one naming the channel asked for, when `others` is a success; and every other call by
/// `others`. Each answer is `(status, body)`. Returns the API base to configure.
fn fake_discord(
    messages: impl Fn(u32) -> (u16, String) + Send + 'static,
    others: (u16, &'static str),
) -> String {
    use std::io::{Read as _, Write as _};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("binding the fake Discord");
    let base = format!("http://{}", listener.local_addr().expect("address"));
    std::thread::spawn(move || {
        let mut reads = 0_u32;
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => request.extend_from_slice(&buffer[..read]),
                }
            }
            let text = String::from_utf8_lossy(&request);
            let path = text
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("")
                .split('?')
                .next()
                .unwrap_or("")
                .to_owned();
            let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
            let (status, body) = match segments.as_slice() {
                [.., "channels", _, "messages"] => {
                    reads += 1;
                    messages(reads - 1)
                }
                // The channel object the probe reads first, to learn the channel's kind.
                [.., "channels", id] if others.0 == 200 => {
                    (200, format!(r#"{{"id":"{id}","type":0}}"#))
                }
                _ => (others.0, others.1.to_owned()),
            };
            let reply = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
                 connection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    base
}

/// Discord's own 429 body, asking for `seconds`.
fn rate_limited(seconds: f64) -> (u16, String) {
    (
        429,
        format!(
            r#"{{"message":"You are being rate limited.","retry_after":{seconds},"global":false}}"#
        ),
    )
}

const IDENTITY: (u16, &str) = (
    200,
    r#"{"id":"3000000000000000001","username":"probe-bot"}"#,
);

fn write_config_against(name: &str, api_base: &str) -> std::path::PathBuf {
    let path = write_config(name);
    let text = config_text().replace("http://127.0.0.1:1", api_base);
    std::fs::write(&path, text).expect("write config");
    path
}

/// Start the binary with the probe on, its two output streams merged into one line channel.
fn spawn_probing(
    config: &std::path::Path,
    extra_env: &[(&str, &str)],
) -> (std::process::Child, mpsc::Receiver<String>) {
    let mut command = Command::new(binary());
    command
        .arg("--config")
        .arg(config)
        .env_remove("VIBE_TALK_SKIP_STARTUP_PROBE")
        .env_remove("VIBE_TALK_STARTUP_PROBE_WAIT_SECONDS")
        .env("NO_COLOR", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, value) in extra_env {
        command.env(name, value);
    }
    let mut child = command.spawn().expect("spawning the binary");
    let (tx, rx) = mpsc::channel::<String>();
    for stream in [
        Box::new(child.stdout.take().expect("stdout")) as Box<dyn std::io::Read + Send>,
        Box::new(child.stderr.take().expect("stderr")),
    ] {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    return;
                }
            }
        });
    }
    (child, rx)
}

/// Everything printed so far, without waiting.
fn drain(rx: &mpsc::Receiver<String>, seen: &mut String) {
    while let Ok(line) = rx.try_recv() {
        seen.push_str(&line);
        seen.push('\n');
    }
}

/// Wait for the "vibe-talk listening" line and return the address it names.
fn listening_address(rx: &mpsc::Receiver<String>, seen: &mut String) -> String {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) {
            seen.push_str(&line);
            seen.push('\n');
            if line.contains("vibe-talk listening") {
                let address = line
                    .split("bind=")
                    .nth(1)
                    .and_then(|rest| rest.split_whitespace().next())
                    .unwrap_or_else(|| panic!("no address on the listening line: {line}"));
                return address.to_owned();
            }
        }
    }
    panic!("the server never said it was listening:\n{seen}");
}

/// `GET path` on the running server: the status code and the JSON body.
fn get(address: &str, path: &str) -> (u16, serde_json::Value) {
    use std::io::{Read as _, Write as _};
    let mut stream = std::net::TcpStream::connect(address).expect("connecting to the server");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("timeout");
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nhost: localhost\r\nconnection: close\r\n\r\n"
    )
    .expect("request");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("response");
    let status = response
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("no status line: {response}"));
    let body = response.split("\r\n\r\n").nth(1).unwrap_or("");
    (
        status,
        serde_json::from_str(body).unwrap_or(serde_json::Value::Null),
    )
}

#[test]
fn a_rate_limited_probe_leaves_the_server_answering_and_it_becomes_ready_when_the_limit_clears() {
    let released = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let api_base = {
        let released = std::sync::Arc::clone(&released);
        fake_discord(
            move |_| {
                if released.load(std::sync::atomic::Ordering::SeqCst) {
                    (200, "[]".to_owned())
                } else {
                    rate_limited(0.2)
                }
            },
            IDENTITY,
        )
    };
    let config = write_config_against("rate-limited-recovers", &api_base);
    let (mut child, rx) = spawn_probing(&config, &[]);
    let mut seen = String::new();
    let address = listening_address(&rx, &mut seen);

    // The listener answers while the probe is still being rate-limited, and says so.
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let (status, health) = loop {
        let (status, health) = get(&address, "/healthz");
        let probe = health["startup_probe"].as_str().unwrap_or("").to_owned();
        if probe.starts_with("probe pending (rate limited, retry in") {
            break (status, health);
        }
        drain(&rx, &mut seen);
        assert!(
            std::time::Instant::now() < deadline,
            "/healthz never reported the rate-limited probe; last {health}\n{seen}"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(
        status, 200,
        "/healthz is liveness and answers 200: {health}"
    );
    assert_eq!(health["status"], "starting", "{health}");
    let (status, ready) = get(&address, "/readyz");
    assert_eq!(status, 503, "not ready while the probe is pending: {ready}");
    assert_eq!(child.try_wait().expect("polling the child"), None, "{seen}");

    released.store(true, std::sync::atomic::Ordering::SeqCst);
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let (status, ready) = get(&address, "/readyz");
        if status == 200 {
            assert_eq!(ready["status"], "ok", "{ready}");
            assert_eq!(ready["startup_probe"], "passed", "{ready}");
            break;
        }
        drain(&rx, &mut seen);
        assert!(
            std::time::Instant::now() < deadline,
            "the server never became ready once the limit cleared; last {ready}\n{seen}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let running = child.try_wait().expect("polling the child");
    let _ = child.kill();
    let _ = child.wait();
    drain(&rx, &mut seen);
    assert_eq!(
        running, None,
        "a rate limit must not end the process:\n{seen}"
    );
    let warned = seen
        .lines()
        .find(|line| line.contains("the startup channel probe was RATE LIMITED"))
        .unwrap_or_else(|| panic!("each wait is logged at WARN:\n{seen}"));
    assert!(
        warned.contains("WARN") && warned.contains("budget_left_seconds="),
        "{warned}"
    );
}

#[test]
fn a_probe_rate_limited_past_its_budget_stops_the_server_and_says_why() {
    let api_base = fake_discord(|_| rate_limited(0.2), IDENTITY);
    let config = write_config_against("rate-limited-forever", &api_base);
    let (mut child, rx) = spawn_probing(&config, &[("VIBE_TALK_STARTUP_PROBE_WAIT_SECONDS", "2")]);
    let status = wait_for_exit(&mut child, Duration::from_secs(60));
    let mut seen = String::new();
    // The reader threads end when the child's streams close.
    while let Ok(line) = rx.recv_timeout(Duration::from_secs(5)) {
        seen.push_str(&line);
        seen.push('\n');
    }
    let status = status.unwrap_or_else(|| panic!("the server waited past its budget:\n{seen}"));
    assert!(!status.success(), "{status:?}\n{seen}");
    assert!(
        seen.contains("vibe-talk listening"),
        "it served meanwhile:\n{seen}"
    );
    assert!(seen.contains("GAVE UP"), "{seen}");
    assert!(
        seen.contains("still rate-limited after the startup probe waited")
            && seen.contains("VIBE_TALK_STARTUP_PROBE_WAIT_SECONDS"),
        "the refusal names the budget and how to raise it:\n{seen}"
    );
}

#[test]
fn a_rejected_token_still_stops_the_server_at_once() {
    let unauthorized = r#"{"message":"401: Unauthorized","code":0}"#;
    let api_base = fake_discord(move |_| (401, unauthorized.to_owned()), (401, unauthorized));
    let config = write_config_against("rejected-token", &api_base);
    let started = std::time::Instant::now();
    let (mut child, rx) = spawn_probing(&config, &[]);
    let status = wait_for_exit(&mut child, Duration::from_secs(30));
    let elapsed = started.elapsed();
    let mut seen = String::new();
    while let Ok(line) = rx.recv_timeout(Duration::from_secs(5)) {
        seen.push_str(&line);
        seen.push('\n');
    }
    let status = status.unwrap_or_else(|| panic!("a rejected token must stop the server:\n{seen}"));
    assert!(!status.success(), "{status:?}\n{seen}");
    assert!(seen.contains("rejected the bot token"), "{seen}");
    assert!(seen.contains("refusing to start"), "{seen}");
    assert!(
        !seen.contains("RATE LIMITED"),
        "nothing is waited out:\n{seen}"
    );
    assert!(
        elapsed < Duration::from_secs(20),
        "a definite failure refuses at once, not after a wait: {elapsed:?}\n{seen}"
    );
}
