//! `vibe-talk prompts`: owner-only messages from the local cache, cache-only. `#1529`.

use vibe_talk::prompts::{channel_matches, read_at, render, HEADER};

const HOUR: i64 = 3_600_000;

fn base_ms() -> i64 {
    vibe_talk::clock::instant_ms("2026-09-20T00:00:00Z").expect("base instant")
}

fn config() -> vibe_talk::prompts::PromptsConfig {
    // No provider token anywhere: a cache-only command must not need one.
    let text = format!(
        r#"
[[channels]]
id = "{}"
label = "lead team"

[owner]
user_ids = ["UOWNER"]
"#,
        vibe_talk::testing::WRITE_CHANNEL
    );
    vibe_talk::prompts::parse_lenient(&text).expect("lenient config parses")
}

#[test]
fn prompts_cli_runs_with_a_tokenless_config_file() {
    let (dir, cache) = fixture_cache();
    let config_path = dir.path().join("app.toml");
    std::fs::write(
        &config_path,
        format!(
            "[[channels]]\nid = \"{}\"\nlabel = \"lead team\"\n\n[owner]\nuser_ids = [\"UOWNER\"]\n",
            vibe_talk::testing::WRITE_CHANNEL
        ),
    )
    .expect("write config");
    let options = vibe_talk::prompts::Options {
        config: Some(config_path),
        cache: Some(cache),
        hours: None,
        since: None,
        channel: Some("#lead team".to_owned()),
        json: false,
        overlays: Vec::new(),
    };
    let out = vibe_talk::prompts::run_at(options, base_ms() + 20 * HOUR).expect("run");
    assert!(out.starts_with(vibe_talk::prompts::HEADER), "{out}");
    assert!(out.contains("first reply"), "{out}");
    assert!(out.contains("second reply"), "{out}");
}

fn fixture_cache() -> (vibe_talk::testing::TempDir, std::path::PathBuf) {
    let dir = vibe_talk::testing::TempDir::new("prompts");
    let path = dir.path().join("cache.sqlite3");
    let connection = rusqlite::Connection::open(&path).expect("create cache");
    connection
        .execute_batch(
            "CREATE TABLE channel (chan INTEGER PRIMARY KEY, provider_key TEXT NOT NULL, channel_id TEXT NOT NULL, dropped_ms INTEGER);
             CREATE TABLE message (row_id INTEGER PRIMARY KEY, chan INTEGER NOT NULL, message_id TEXT NOT NULL, numeric_id INTEGER, created_ms INTEGER NOT NULL, observed_ms INTEGER NOT NULL, home_scope TEXT NOT NULL, thread_id TEXT, is_root INTEGER NOT NULL DEFAULT 0, author_id TEXT NOT NULL, author TEXT NOT NULL, content TEXT NOT NULL, content_partial INTEGER NOT NULL DEFAULT 0);
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
    // Old thread root (owner, +6h), in-window owner replies (+11h, +12h), an agent
    // message in window (+11h), and an owner standalone before the window (+9h).
    type Row = (
        &'static str,
        i64,
        &'static str,
        &'static str,
        Option<&'static str>,
        i64,
    );
    let rows: [Row; 5] = [
        ("m-root", 6, "UOWNER", "old root", Some("thread-A"), 1),
        ("m-r1", 11, "UOWNER", "first reply", Some("thread-A"), 0),
        ("m-r2", 12, "UOWNER", "second reply", Some("thread-A"), 0),
        ("m-agent", 11, "UAGENT", "agent note", Some("thread-A"), 0),
        ("m-old", 9, "UOWNER", "before window", None, 0),
    ];
    for (index, (id, hours, author, content, thread, is_root)) in rows.iter().enumerate() {
        connection
            .execute(
                "INSERT INTO message (row_id, chan, message_id, numeric_id, created_ms, observed_ms, home_scope, thread_id, is_root, author_id, author, content) VALUES (?1, 1, ?2, ?3, ?4, ?4, 'thread', ?5, ?6, ?7, 'Owner', ?8)",
                rusqlite::params![
                    index as i64 + 1,
                    id,
                    index as i64 + 1,
                    base_ms() + hours * HOUR,
                    thread,
                    is_root,
                    author,
                    content
                ],
            )
            .expect("message");
    }
    connection
        .execute(
            "INSERT INTO thread (chan, thread_id, reply_count, last_activity_ms) VALUES (1, 'thread-A', 2, ?1)",
            [base_ms() + 12 * HOUR],
        )
        .expect("thread");
    (dir, path)
}

#[test]
fn prompts_returns_only_owner_messages_oldest_first_with_thread_replies() {
    let config = config();
    let (_dir, cache) = fixture_cache();
    let answer = read_at(
        &config,
        &cache,
        Some("#lead team"),
        base_ms() + 10 * HOUR,
        base_ms() + 20 * HOUR,
    )
    .expect("read");
    let contents: Vec<&str> = answer
        .messages
        .iter()
        .map(|message| message.content.as_str())
        .collect();
    assert_eq!(contents, ["first reply", "second reply"]);
    assert!(answer
        .messages
        .iter()
        .all(|m| m.thread_id.as_deref() == Some("thread-A")));
    // 2026-09-20 11:00 UTC is 07:00 EDT.
    assert_eq!(answer.messages[0].et, "2026-09-20 07:00 ET");
    let text = render(&answer);
    assert!(text.starts_with(HEADER), "{text}");
    assert!(text.contains("#lead team"), "{text}");
    assert!(!text.contains("agent note"), "{text}");
    assert!(!text.contains("before window"), "{text}");
}

#[test]
fn prompts_channel_matching_accepts_name_hash_and_id() {
    let config = config();
    let channel = &config.channels[0];
    assert!(channel_matches("lead team", channel));
    assert!(channel_matches("#lead team", channel));
    assert!(channel_matches(channel.id.as_str(), channel));
    assert!(!channel_matches("#other", channel));
}
