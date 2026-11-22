//! `vibe-talk-thread-replay` — read one thread from a message-cache copy
//! and print it in the Slack `conversations.replies` answer shape.
//!
//! A dev helper for `scripts/local_clone.py` (`#2060
//! vibe-talk-realistic-testing`): the clone's stub backend answers the
//! server's thread reads by running this over the copied cache, so the
//! page renders a real thread view through the normal provider path.
//! It is never linked into the server binary, opens its file read-only,
//! and takes no other input. The output carries message text; callers
//! must treat it as they treat the cache itself (never log or commit
//! it). A thread the copy does not hold whole answers
//! `{"ok": false, "error": "thread_not_in_cache"}` and exit 2.

use std::path::PathBuf;

use rusqlite::{Connection, OpenFlags, OptionalExtension};

/// The first millisecond Discord counts snowflakes from, as in
/// `crate::slack::ids`: the cache's numeric ids are snowflake-shaped.
const DISCORD_EPOCH_MS: i64 = 1_420_070_400_000;

struct Args {
    cache: PathBuf,
    channel: String,
    thread: Option<String>,
    history: bool,
    limit: i64,
}

fn parse_args() -> Result<Args, String> {
    let mut cache = None;
    let mut channel = None;
    let mut thread = None;
    let mut history = false;
    let mut limit: i64 = 100;
    let mut argv = std::env::args().skip(1);
    while let Some(arg) = argv.next() {
        match arg.as_str() {
            "--history" => history = true,
            "--limit" => {
                limit = argv
                    .next()
                    .ok_or("--limit needs a value")?
                    .parse()
                    .map_err(|_| "--limit must be a number")?;
            }
            "--cache" | "--channel" | "--thread" => {
                let slot = match arg.as_str() {
                    "--cache" => &mut cache,
                    "--channel" => &mut channel,
                    _ => &mut thread,
                };
                *slot = Some(argv.next().ok_or_else(|| format!("{arg} needs a value"))?);
            }
            other => return Err(format!("unrecognized argument {other:?}")),
        }
    }
    if !history && thread.is_none() {
        return Err("--thread is required (or pass --history)".to_owned());
    }
    Ok(Args {
        cache: PathBuf::from(cache.ok_or("--cache is required")?),
        channel: channel.ok_or("--channel is required")?,
        thread,
        history,
        limit: limit.clamp(1, 1000),
    })
}

/// A Slack `ts` for one cached row: its snowflake-shaped numeric id
/// decoded, or its millisecond when no numeric id was recorded.
fn ts_of(numeric_id: Option<i64>, created_ms: i64) -> String {
    let micros = match numeric_id {
        Some(raw) => {
            let ms = (raw >> 22) + DISCORD_EPOCH_MS;
            ms * 1000 + (raw & ((1 << 22) - 1)) % 1000
        }
        None => created_ms * 1000,
    };
    format!("{}.{:06}", micros / 1_000_000, micros % 1_000_000)
}

fn open(args: &Args) -> Result<(Connection, i64), String> {
    let connection = Connection::open_with_flags(
        &args.cache,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("cannot open the cache copy: {error}"))?;
    let chan: i64 = connection
        .query_row(
            "SELECT chan FROM channel WHERE channel_id = ?1",
            [&args.channel],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| format!("channel lookup failed: {error}"))?
        .ok_or("channel_not_in_cache")?;
    Ok((connection, chan))
}

/// The channel's newest main messages, in the `conversations.history`
/// answer shape, so the server's thread scan can find the roots.
fn history(args: &Args) -> Result<String, String> {
    let (connection, chan) = open(args)?;
    let mut statement = connection
        .prepare(
            "SELECT m.numeric_id, m.created_ms, m.author_id, m.content, \
                    t.reply_count \
             FROM message m \
             LEFT JOIN thread t ON t.chan = m.chan AND t.root_id = m.message_id \
             WHERE m.chan = ?1 AND m.content_partial = 0 AND m.thread_id IS NULL \
             ORDER BY m.created_ms DESC, m.numeric_id DESC LIMIT ?2",
        )
        .map_err(|error| format!("history lookup failed: {error}"))?;
    let rows = statement
        .query_map((chan, args.limit), |row| {
            Ok((
                row.get::<_, Option<i64>>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<i64>>(4)?,
            ))
        })
        .map_err(|error| format!("history lookup failed: {error}"))?;
    let mut messages = Vec::new();
    for row in rows {
        let (numeric_id, created_ms, author_id, content, reply_count) =
            row.map_err(|error| format!("history read failed: {error}"))?;
        let ts = ts_of(numeric_id, created_ms);
        let mut message = serde_json::json!({
            "ts": ts,
            "user": author_id,
            "text": content,
        });
        if let Some(count) = reply_count {
            message["reply_count"] = serde_json::json!(count);
            message["thread_ts"] = serde_json::json!(ts);
        }
        messages.push(message);
    }
    Ok(serde_json::json!({
        "ok": true,
        "messages": messages,
        "has_more": false,
    })
    .to_string())
}

fn run(args: &Args) -> Result<String, String> {
    if args.history {
        return history(args);
    }
    let thread_ts = args.thread.as_deref().unwrap_or_default();
    let (connection, chan) = open(args)?;
    let thread: Option<(Option<String>, Option<i64>)> = connection
        .query_row(
            "SELECT root_id, reply_count FROM thread WHERE chan = ?1 AND thread_id = ?2",
            (chan, thread_ts),
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| format!("thread lookup failed: {error}"))?;
    let (root_id, reply_count) = thread.ok_or("thread_not_in_cache")?;
    let mut statement = connection
        .prepare(
            "SELECT message_id, numeric_id, created_ms, author_id, content, is_root \
             FROM message \
             WHERE chan = ?1 AND content_partial = 0 \
               AND (thread_id = ?2 OR message_id = ?3) \
             ORDER BY created_ms, numeric_id",
        )
        .map_err(|error| format!("message lookup failed: {error}"))?;
    let rows = statement
        .query_map((chan, thread_ts, root_id.as_deref().unwrap_or("")), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
            ))
        })
        .map_err(|error| format!("message lookup failed: {error}"))?;
    let mut messages = Vec::new();
    for row in rows {
        let (message_id, numeric_id, created_ms, author_id, content, _is_root) =
            row.map_err(|error| format!("message read failed: {error}"))?;
        let ts = ts_of(numeric_id, created_ms);
        let mut message = serde_json::json!({
            "ts": ts,
            "user": author_id,
            "text": content,
        });
        if ts == thread_ts {
            message["reply_count"] = serde_json::json!(reply_count.unwrap_or(0));
        } else {
            message["thread_ts"] = serde_json::json!(thread_ts);
        }
        let _ = message_id;
        messages.push(message);
    }
    if messages.is_empty() || messages[0]["ts"] != thread_ts {
        return Err("thread_not_in_cache".to_owned());
    }
    Ok(serde_json::json!({
        "ok": true,
        "messages": messages,
        "has_more": false,
    })
    .to_string())
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("vibe-talk-thread-replay: {error}");
            std::process::exit(2);
        }
    };
    match run(&args) {
        Ok(answer) => println!("{answer}"),
        Err(error) => {
            println!("{}", serde_json::json!({"ok": false, "error": error}));
            std::process::exit(2);
        }
    }
}
