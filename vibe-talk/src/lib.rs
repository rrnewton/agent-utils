//! `vibe-talk` — a chat bridge for a voice agent and phone web app.
//!
//! The server owns the front door. A hosted voice agent (ElevenLabs, in the first intended
//! deployment) reaches this process over HTTP, asks it what has been said in a chat channel,
//! and — with explicit approval — asks it to post a reply.
//!
//! # One thing here pushes
//!
//! This used to say "nothing here pushes: every read is pulled at the moment the owner asks a
//! question", and that stopped being true with [`live`]. A deployment chooses one producer:
//! bounded built-in polling with `discord.live_poll_seconds`, or authenticated provider-neutral
//! adapter push with `ingest.token`. Both fan changes out through
//! `GET /api/v1/channels/{id}/stream`, so a page learns about them without asking. Both are OFF
//! unless configured; the channel allowlist applies to ingestion and to the browser stream.
//!
//! # What is kept
//!
//! Almost nothing. A channel is never cached as a channel: every question is a fresh Discord
//! fetch. [`store`] is the single exception, and it holds three things — the `/voice` transcript,
//! how far the owner has read, and one short cached SUMMARY per long message. The first two are
//! what this server itself authored; the third is derived from somebody else's message — a
//! model's paraphrase of one, which carries its content as surely as a quotation would — so it is
//! treated as a second at-rest copy of third-party text: bounded by the same retention, erased by
//! the same purge, and never written on behalf of a read-scope token. Read state in particular is OURS:
//! Discord shares none with a bot, so nothing here is synchronised with it in either direction.
//!
//! # Untrusted input
//!
//! Discord message content is written by other parties. Everything this crate returns from a
//! channel is DATA, never instructions, and it is the caller's job to keep treating it that way.
//! [`untrusted`] exists to make that boundary explicit at the one place where channel text is
//! handed to a language model.

#![forbid(unsafe_code)]

pub mod access;
pub mod agent_backend;
pub mod auth;
pub mod chat;
pub mod clock;
pub mod config;
pub mod conversation;
pub mod diagnostics;
pub mod directory;
pub mod discord;
pub mod elevenlabs;
pub mod http;
pub mod live;
pub mod mcp;
pub mod model;
pub mod ops;
pub mod probe;
pub mod replay;
pub mod retrieval;
pub mod speakable;
pub mod speech;
pub mod speech_tickets;
pub mod state;
pub mod store;
pub mod summarize;
pub mod summary;
pub mod testing;
pub mod threads;
pub mod untrusted;
pub mod voice_agent;
pub mod voice_health;
