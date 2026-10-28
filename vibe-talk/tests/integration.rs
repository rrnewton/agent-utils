//! Every integration test in one binary (`vibe-talk-test-debuginfo`, 2026-10-26).
//!
//! Each `tests/*.rs` file used to link its own copy of the whole server: 91 binaries of about
//! 110 MB each in a debug build (330 MB with full debuginfo), and a link step apiece in every
//! validation run. Here each file is a module of one test binary instead, so the server is
//! linked once. A file is still run alone with a filter on its module name:
//! `cargo test --test integration rich_headers::`.
//!
//! Two files stay binaries of their own, because they would change what the others see if
//! they shared a process: `egress_proxy` points `HTTPS_PROXY` at a fake proxy for the whole
//! process, and `voice_page` runs the page's node suites, which validation runs separately and
//! skips here by its exact name. Both are `[[test]]` targets in `Cargo.toml`.
//!
//! `autotests = false` means cargo no longer finds a new `tests/*.rs` by itself: add it below
//! (or as a `[[test]]` target). `every_test_file_is_compiled` fails when one is in neither.

// The loopback Slack Web API fake the Slack tests share; each imports it with
// `use crate::slack_fake;`.
mod slack_fake;

#[path = "agent_summaries.rs"]
mod agent_summaries;

#[path = "alias.rs"]
mod alias;

#[path = "api.rs"]
mod api;

#[path = "app_name.rs"]
mod app_name;

#[path = "asset_caching.rs"]
mod asset_caching;

#[path = "bookmark.rs"]
mod bookmark;

#[path = "cache_bench.rs"]
mod cache_bench;

#[path = "cache_source.rs"]
mod cache_source;

#[path = "channel_heads.rs"]
mod channel_heads;

#[path = "chat_provider.rs"]
mod chat_provider;

#[path = "client_settings.rs"]
mod client_settings;

#[path = "containerfile.rs"]
mod containerfile;

#[path = "contract.rs"]
mod contract;

#[path = "diagnostics.rs"]
mod diagnostics;

#[path = "documents.rs"]
mod documents;

#[path = "elevenlabs_mock.rs"]
mod elevenlabs_mock;

#[path = "explicit_send.rs"]
mod explicit_send;

#[path = "fallback_chain.rs"]
mod fallback_chain;

#[path = "hidden_channels.rs"]
mod hidden_channels;

#[path = "ingest.rs"]
mod ingest;

#[path = "live.rs"]
mod live;

#[path = "llm_summaries.rs"]
mod llm_summaries;

#[path = "local_tts.rs"]
mod local_tts;

#[path = "logging.rs"]
mod logging;

#[path = "mcp.rs"]
mod mcp;

#[path = "message_cache_access.rs"]
mod message_cache_access;

#[path = "message_cache_fill.rs"]
mod message_cache_fill;

#[path = "message_cache_serve.rs"]
mod message_cache_serve;

#[path = "message_cache_shadow.rs"]
mod message_cache_shadow;

#[path = "message_cache_switch.rs"]
mod message_cache_switch;

#[path = "message_lookups.rs"]
mod message_lookups;

#[path = "multi_provider.rs"]
mod multi_provider;

#[path = "naming_budget.rs"]
mod naming_budget;

#[path = "noise.rs"]
mod noise;

#[path = "op_key_carrier.rs"]
mod op_key_carrier;

#[path = "own_read.rs"]
mod own_read;

#[path = "page_build.rs"]
mod page_build;

#[path = "pairing.rs"]
mod pairing;

#[path = "pins.rs"]
mod pins;

#[path = "post_gate.rs"]
mod post_gate;

#[path = "post_prefix.rs"]
mod post_prefix;

#[path = "post_split.rs"]
mod post_split;

#[path = "provider_diagnostics_ids.rs"]
mod provider_diagnostics_ids;

#[path = "range_reads.rs"]
mod range_reads;

#[path = "read_ahead.rs"]
mod read_ahead;

#[path = "read_aloud.rs"]
mod read_aloud;

#[path = "recent_many.rs"]
mod recent_many;

#[path = "recent_view.rs"]
mod recent_view;

#[path = "rendering.rs"]
mod rendering;

#[path = "replay_bridge.rs"]
mod replay_bridge;

#[path = "reply_read.rs"]
mod reply_read;

#[path = "rich_headers.rs"]
mod rich_headers;

#[path = "root_page.rs"]
mod root_page;

#[path = "scope_first.rs"]
mod scope_first;

#[path = "send_approval.rs"]
mod send_approval;

#[path = "send_confirm_thread.rs"]
mod send_confirm_thread;

#[path = "send_dedupe.rs"]
mod send_dedupe;

#[path = "send_double_check.rs"]
mod send_double_check;

#[path = "send_double_check_page.rs"]
mod send_double_check_page;

#[path = "send_log.rs"]
mod send_log;

#[path = "send_order.rs"]
mod send_order;

#[path = "send_settle.rs"]
mod send_settle;

#[path = "server_versions.rs"]
mod server_versions;

#[path = "signed_url.rs"]
mod signed_url;

#[path = "slack_deployment.rs"]
mod slack_deployment;

#[path = "slack_edited_mark.rs"]
mod slack_edited_mark;

#[path = "slack_emoji_shortcodes.rs"]
mod slack_emoji_shortcodes;

#[path = "slack_file_posts.rs"]
mod slack_file_posts;

#[path = "slack_flat_calls.rs"]
mod slack_flat_calls;

#[path = "slack_group_dm_names.rs"]
mod slack_group_dm_names;

#[path = "slack_ids.rs"]
mod slack_ids;

#[path = "slack_live_ingest.rs"]
mod slack_live_ingest;

#[path = "slack_mention_names.rs"]
mod slack_mention_names;

#[path = "slack_provider.rs"]
mod slack_provider;

#[path = "slack_quote_lines.rs"]
mod slack_quote_lines;

#[path = "slack_reactions.rs"]
mod slack_reactions;

#[path = "slack_reply_by_id.rs"]
mod slack_reply_by_id;

#[path = "slack_reply_live.rs"]
mod slack_reply_live;

#[path = "slack_system_bots.rs"]
mod slack_system_bots;

#[path = "slack_upstream_clear.rs"]
mod slack_upstream_clear;

#[path = "startup.rs"]
mod startup;

#[path = "summaries.rs"]
mod summaries;

#[path = "thread_hues.rs"]
mod thread_hues;

#[path = "thread_reads.rs"]
mod thread_reads;

#[path = "threads.rs"]
mod threads;

#[path = "todo.rs"]
mod todo;

#[path = "upstream_clear.rs"]
mod upstream_clear;

#[path = "upstream_profile.rs"]
mod upstream_profile;

#[path = "upstream_url.rs"]
mod upstream_url;

#[path = "voice_agent.rs"]
mod voice_agent;

#[path = "voice_catchup.rs"]
mod voice_catchup;

#[path = "voice_prompt.rs"]
mod voice_prompt;

#[path = "voice_relay.rs"]
mod voice_relay;

#[path = "wit_tts.rs"]
mod wit_tts;

/// `autotests = false` turns a forgotten test file into one that silently never runs, so every
/// `tests/*.rs` must be a module above or a `[[test]]` target of its own.
#[test]
fn every_test_file_is_compiled() {
    let root = env!("CARGO_MANIFEST_DIR");
    let listed = include_str!("integration.rs");
    let manifest = std::fs::read_to_string(format!("{root}/Cargo.toml")).expect("Cargo.toml");
    let mut missing = Vec::new();
    for entry in std::fs::read_dir(format!("{root}/tests")).expect("tests directory") {
        let name = entry
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .into_owned();
        let Some(stem) = name.strip_suffix(".rs") else {
            continue;
        };
        if stem == "integration" {
            continue;
        }
        let module = format!("#[path = \"{name}\"]\nmod {stem};");
        let target = format!("path = \"tests/{name}\"");
        if !listed.contains(&module) && !manifest.contains(&target) {
            missing.push(name);
        }
    }
    assert!(
        missing.is_empty(),
        "test files in no test target: {missing:?}"
    );
}
