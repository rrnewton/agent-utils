//! `vibe-talk search` / `threads` / `thread`: the unified cache-only read
//! CLI. `#2106 vibe-talk-unified-cache-read-cli`.
//!
//! Every test runs against a synthetic fixture cache (invented channels,
//! authors and text) and a tokenless config: no server is configured, no
//! token exists, and the reader's only door is the cache file opened
//! read-only, so a green run is itself the no-network/no-server evidence.

use vibe_talk::read_cli::{message_matches, split_query, Term};

const HOUR: i64 = 3_600_000;
const THREAD_A: &str = "1730000000.000100";
const THREAD_B: &str = "1730000100.000200";

fn base_ms() -> i64 {
    vibe_talk::clock::instant_ms("2026-09-20T00:00:00Z").expect("base instant")
}

fn term(text: &str) -> Term {
    Term {
        text: text.to_owned(),
        phrase: false,
    }
}

#[test]
fn matching_is_message_granular_and_phrase_aware() {
    // foo and bar on different lines of ONE message: an AND hit, which
    // line grep and paragraph grep both miss.
    let split = "foo at the top\nand bar down here";
    assert!(message_matches(
        "Owner",
        split,
        &[term("foo"), term("bar")],
        false
    ));
    assert!(!message_matches(
        "Owner",
        "only foo",
        &[term("foo"), term("bar")],
        false
    ));
    assert!(message_matches(
        "Owner",
        "only foo",
        &[term("foo"), term("bar")],
        true
    ));
    // A quoted phrase must be contiguous; its words apart do not hit.
    let phrase = split_query("\"foo at\"");
    assert_eq!(phrase.len(), 1);
    assert!(message_matches("Owner", split, &phrase, false));
    assert!(!message_matches(
        "Owner",
        split,
        &split_query("\"foo bar\""),
        false
    ));
    // Case folds; the author is searched too.
    assert!(message_matches("Owner", "nothing", &[term("OWNER")], false));
    assert!(message_matches("x", "a BAR none", &[term("bar")], false));
    // No terms matches everything (the --mine-with-no-terms case).
    assert!(message_matches("x", "y", &[], false));
}

#[test]
fn server_flags_are_refused_so_no_path_reaches_a_server() {
    for flag in ["--server", "--token-file", "--base-url"] {
        let err = vibe_talk::read_cli::parse_search(vec![
            "foo".to_owned(),
            flag.to_owned(),
            "http://127.0.0.1:1".to_owned(),
        ])
        .expect_err("server flag refused");
        assert!(err.to_string().contains("cache-only"), "{flag}: {err}");
    }
}

/// A synthetic cache + config + short-refs file in a scratch directory.
/// Returns (dir guard, config path, cache path, short-refs path).
fn fixture() -> (
    vibe_talk::testing::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let dir = vibe_talk::testing::TempDir::new("readcli");
    let cache = dir.path().join("cache.sqlite3");
    let connection = rusqlite::Connection::open(&cache).expect("create cache");
    connection
        .execute_batch(
            "CREATE TABLE channel (chan INTEGER PRIMARY KEY, provider_key TEXT NOT NULL, channel_id TEXT NOT NULL, dropped_ms INTEGER);
             CREATE TABLE message (row_id INTEGER PRIMARY KEY, chan INTEGER NOT NULL, message_id TEXT NOT NULL, numeric_id INTEGER, created_ms INTEGER NOT NULL, observed_ms INTEGER NOT NULL, home_scope TEXT NOT NULL, thread_id TEXT, is_root INTEGER NOT NULL DEFAULT 0, author_id TEXT NOT NULL, author TEXT NOT NULL, content TEXT NOT NULL, content_partial INTEGER NOT NULL DEFAULT 0, extra TEXT NOT NULL DEFAULT '{}');
             CREATE TABLE thread (chan INTEGER NOT NULL, thread_id TEXT NOT NULL, reply_count INTEGER, last_activity_ms INTEGER NOT NULL, PRIMARY KEY (chan, thread_id));",
        )
        .expect("schema");
    let channel = vibe_talk::testing::WRITE_CHANNEL;
    connection
        .execute(
            "INSERT INTO channel (chan, provider_key, channel_id) VALUES (1, 'test', ?1)",
            [channel],
        )
        .expect("channel");
    // (id, hours after base, author id, author name, content, thread, is_root, extra)
    type Row = (
        &'static str,
        i64,
        &'static str,
        &'static str,
        &'static str,
        Option<&'static str>,
        i64,
        &'static str,
    );
    let rows: [Row; 9] = [
        (
            "m-old",
            2,
            "UOWNER",
            "Owner",
            "old foo bar note",
            None,
            0,
            "{}",
        ),
        (
            "m-root",
            6,
            "UOWNER",
            "Owner",
            "kickoff foo notes",
            Some(THREAD_A),
            1,
            r#"{"thread":{"id":"1730000000.000100","is_root":true,"display_name":"alpha-launch-plan"}}"#,
        ),
        (
            "m-r1",
            11,
            "UAGENT",
            "Agent",
            "agent reply about bar charts",
            Some(THREAD_A),
            0,
            "{}",
        ),
        (
            "m-mine",
            12,
            "UOWNER",
            "Owner",
            "foo at the top\nand bar down here",
            Some(THREAD_A),
            0,
            "{}",
        ),
        (
            "m-foo",
            13,
            "UOTHER",
            "Other",
            "only foo here",
            None,
            0,
            "{}",
        ),
        (
            "m-bar",
            13,
            "UOTHER",
            "Other",
            "only Bar here",
            None,
            0,
            "{}",
        ),
        (
            "m-phrase",
            14,
            "UOTHER",
            "Other",
            "the deploy freeze starts now",
            None,
            0,
            "{}",
        ),
        (
            "m-root2",
            15,
            "UOTHER",
            "Other",
            "another topic entirely",
            Some(THREAD_B),
            1,
            r#"{"thread":{"id":"1730000100.000200","is_root":true,"display_name":"beta-review-thread"}}"#,
        ),
        (
            "m-r9",
            16,
            "UOTHER",
            "Other",
            "foo again in thread b",
            Some(THREAD_B),
            0,
            "{}",
        ),
    ];
    for (index, (id, hours, author_id, author, content, thread, is_root, extra)) in
        rows.iter().enumerate()
    {
        connection
            .execute(
                "INSERT INTO message (row_id, chan, message_id, numeric_id, created_ms, observed_ms, home_scope, thread_id, is_root, author_id, author, content, extra) VALUES (?1, 1, ?2, ?3, ?4, ?4, 'thread', ?5, ?6, ?7, ?8, ?9, ?10)",
                rusqlite::params![
                    index as i64 + 1, id, index as i64 + 1, base_ms() + hours * HOUR,
                    thread, is_root, author_id, author, content, extra
                ],
            )
            .expect("message");
    }
    connection
        .execute("INSERT INTO thread (chan, thread_id, reply_count, last_activity_ms) VALUES (1, ?1, 2, ?2)",
            rusqlite::params![THREAD_A, base_ms() + 12 * HOUR])
        .expect("thread a");
    connection
        .execute("INSERT INTO thread (chan, thread_id, reply_count, last_activity_ms) VALUES (1, ?1, 1, ?2)",
            rusqlite::params![THREAD_B, base_ms() + 16 * HOUR])
        .expect("thread b");

    let refs = dir.path().join("short-refs.json");
    std::fs::write(
        &refs,
        format!(
            r#"{{"schema":"agentctl-short-refs/v1","threads":[{{"n":60,"channel":"{channel}","root_ts":"{THREAD_A}","root_message":"{THREAD_A}"}},{{"n":61,"channel":"{channel}","root_ts":"{THREAD_B}","root_message":"{THREAD_B}"}}],"messages":[]}}"#
        ),
    )
    .expect("short refs");
    let config_path = dir.path().join("app.toml");
    std::fs::write(
        &config_path,
        format!(
            "[[channels]]\nid = \"{channel}\"\nlabel = \"lead team\"\n\n[owner]\nuser_ids = [\"UOWNER\"]\n\n[render]\nshort_refs_files = [\"{}\"]\n",
            refs.display()
        ),
    )
    .expect("config");
    (dir, config_path, cache, refs)
}

/// `now` for the fixture runs: 20h after base, so the default 24h window
/// holds every fixture row.
fn now() -> i64 {
    base_ms() + 20 * HOUR
}

fn search(args: &[&str]) -> String {
    let (_dir, config, cache, _refs) = fixture();
    let mut full: Vec<String> = args.iter().map(|s| (*s).to_owned()).collect();
    full.push("--config".to_owned());
    full.push(config.display().to_string());
    full.push("--cache".to_owned());
    full.push(cache.display().to_string());
    // Keep the scratch dir alive until the run is done: leak it (tests are
    // short-lived; the dir is under the test tmp root).
    std::mem::forget(_dir);
    let options = vibe_talk::read_cli::parse_search(full)
        .expect("parse")
        .expect("options");
    vibe_talk::read_cli::search_at(options, now()).expect("search")
}

#[test]
fn search_and_phrase_any_and_case() {
    // AND at message granularity: foo+bar across lines hits only m-mine
    // (m-old also holds both but sits outside the --since window below;
    // widen with --hours 24 from now=+20h: window starts -4h, so it hits).
    let and = search(&["foo", "bar", "--hours", "24"]);
    assert!(and.contains("foo at the top"), "{and}");
    assert!(and.contains("old foo bar note"), "{and}");
    assert!(!and.contains("only foo here"), "{and}");
    assert!(!and.contains("only Bar here"), "{and}");
    // A quoted phrase needs contiguity.
    let phrase = search(&["\"foo at the top\"", "--hours", "24"]);
    assert!(phrase.contains("foo at the top"), "{phrase}");
    let apart = search(&["\"foo bar\"", "--hours", "24"]);
    assert!(!apart.contains("foo at the top"), "{apart}");
    // OR and case-insensitivity.
    let any = search(&["--any", "FOO", "bar", "--hours", "24"]);
    assert!(any.contains("only foo here"), "{any}");
    assert!(any.contains("only Bar here"), "{any}");
    // Whole message by default; --brief is one line.
    let brief = search(&["foo", "--brief", "--hours", "24"]);
    assert!(
        brief.lines().filter(|l| l.contains("foo")).count() >= 3,
        "{brief}"
    );
    assert!(!brief.contains("and bar down here"), "{brief}");
}

#[test]
fn mine_with_no_terms_is_prompts() {
    let (_dir, config, cache, _refs) = fixture();
    std::mem::forget(_dir);
    let prompts_options = vibe_talk::prompts::Options {
        config: Some(config.clone()),
        cache: Some(cache.clone()),
        hours: Some(10),
        since: None,
        channel: None,
        json: false,
        overlays: Vec::new(),
    };
    let prompts = vibe_talk::prompts::run_at(prompts_options, now()).expect("prompts");

    let options = vibe_talk::read_cli::parse_search(vec![
        "--mine".to_owned(),
        "--hours".to_owned(),
        "10".to_owned(),
        "--config".to_owned(),
        config.display().to_string(),
        "--cache".to_owned(),
        cache.display().to_string(),
    ])
    .expect("parse")
    .expect("options");
    let found = vibe_talk::read_cli::search_at(options, now()).expect("search --mine");
    assert_eq!(found, prompts, "prompts == search --mine with no terms");
    assert!(found.contains("foo at the top"), "{found}");
    assert!(!found.contains("only foo here"), "{found}");
}

#[test]
fn time_windows_since_until_range_in_et() {
    // m-r1 is at +11h = 2026-09-20 07:00 ET exactly.
    let at_since = search(&["charts", "--since", "2026-09-20 07:00"]);
    assert!(
        at_since.contains("bar charts"),
        "since is inclusive: {at_since}"
    );
    let before = search(&["charts", "--until", "2026-09-20 07:00"]);
    assert!(
        !before.contains("bar charts"),
        "until instant is exclusive: {before}"
    );
    let whole_day = search(&["charts", "--until", "2026-09-20"]);
    assert!(
        whole_day.contains("bar charts"),
        "date-only until covers its day: {whole_day}"
    );
    let ranged = search(&["bar", "--range", "2026-09-20 06:00..2026-09-20 08:00"]);
    assert!(ranged.contains("bar charts"), "{ranged}");
    assert!(!ranged.contains("foo again in thread b"), "{ranged}");
    // Conflicting windows refuse.
    assert!(vibe_talk::read_cli::parse_search(vec![
        "x".into(),
        "--hours".into(),
        "2".into(),
        "--since".into(),
        "2026-09-20".into()
    ])
    .is_err());
    assert!(vibe_talk::read_cli::parse_search(vec![
        "x".into(),
        "--range".into(),
        "2026-09-20..2026-09-21".into(),
        "--until".into(),
        "2026-09-22".into()
    ])
    .is_err());
}

#[test]
fn channel_json_brief_and_thread_identity() {
    let json_text = search(&["foo", "--hours", "24", "--json"]);
    let value: serde_json::Value = serde_json::from_str(&json_text).expect("json");
    let hits = value["hits"].as_array().expect("hits");
    let mine = hits
        .iter()
        .find(|h| h["message_id"] == "m-mine")
        .expect("m-mine hit");
    assert_eq!(mine["thread_id"], THREAD_A);
    assert_eq!(mine["thread_slug"], "alpha-launch-plan");
    assert_eq!(mine["thread_short"], ":thread:60");
    assert!(mine["et"].as_str().expect("et").ends_with(" ET"), "{mine}");
    assert_eq!(mine["channel"], "lead team");
    // Channel filter by #name and by unknown name.
    let only = search(&["foo", "--hours", "24", "--channel", "#lead team"]);
    assert!(only.contains("only foo here"), "{only}");
    let none = search(&["foo", "--hours", "24", "--channel", "#elsewhere"]);
    assert!(!none.contains("only foo here"), "{none}");
    // Text hits carry the thread triple too.
    let text = search(&["charts", "--hours", "24"]);
    assert!(
        text.contains(THREAD_A)
            && text.contains("alpha-launch-plan")
            && text.contains(":thread:60"),
        "{text}"
    );
}

#[test]
fn threads_listing_and_thread_read_by_each_name() {
    let (_dir, config, cache, _refs) = fixture();
    std::mem::forget(_dir);
    let base_args = |extra: &[&str]| -> Vec<String> {
        let mut v: Vec<String> = extra.iter().map(|s| (*s).to_owned()).collect();
        v.push("--config".to_owned());
        v.push(config.display().to_string());
        v.push("--cache".to_owned());
        v.push(cache.display().to_string());
        v
    };
    let listed = vibe_talk::read_cli::threads_at(
        vibe_talk::read_cli::parse_threads(base_args(&["--hours", "24"]))
            .expect("parse")
            .expect("options"),
        now(),
    )
    .expect("threads");
    assert!(
        listed.contains(THREAD_A) && listed.contains(THREAD_B),
        "{listed}"
    );
    assert!(
        listed.contains("alpha-launch-plan") && listed.contains(":thread:60"),
        "{listed}"
    );
    assert!(
        listed.contains("beta-review-thread") && listed.contains(":thread:61"),
        "{listed}"
    );
    // Newest activity first: thread B (+16h) before thread A (+12h).
    assert!(listed.find(THREAD_B) < listed.find(THREAD_A), "{listed}");

    for reference in [THREAD_A, "alpha-launch-plan", ":thread:60", "60"] {
        let read = vibe_talk::read_cli::thread_at(
            vibe_talk::read_cli::parse_thread(base_args(&[reference]))
                .expect("parse")
                .expect("options"),
            now(),
        )
        .expect("thread read");
        assert!(read.contains("kickoff foo notes"), "{reference}: {read}");
        assert!(read.contains("foo at the top"), "{reference}: {read}");
        assert!(
            !read.contains("foo again in thread b"),
            "{reference}: {read}"
        );
    }
    // Searching inside one thread.
    let searched = vibe_talk::read_cli::thread_at(
        vibe_talk::read_cli::parse_thread(base_args(&[THREAD_A, "charts"]))
            .expect("parse")
            .expect("options"),
        now(),
    )
    .expect("thread search");
    assert!(
        searched.contains("bar charts") && !searched.contains("kickoff"),
        "{searched}"
    );
    // Unknown thread refuses.
    assert!(vibe_talk::read_cli::thread_at(
        vibe_talk::read_cli::parse_thread(base_args(&["no-such-thread"]))
            .expect("parse")
            .expect("options"),
        now(),
    )
    .is_err());
}
