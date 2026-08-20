//! `gent-talk` — a Discord bridge for a voice agent.
//!
//! The server owns the front door. A hosted voice agent (ElevenLabs, in the first intended
//! deployment) reaches this process over HTTP, asks it what has been said in a Discord channel,
//! and — with explicit approval — asks it to post a reply. Nothing here pushes: every read is
//! pulled at the moment the owner asks a question.
//!
//! # What is kept
//!
//! Almost nothing. Channel content is never cached: every question is a fresh Discord fetch.
//! [`store`] is the single exception, and it holds only what this server itself authored — the
//! `/voice` transcript, and how far the owner has read. Read state in particular is OURS: Discord
//! shares none with a bot, so nothing here is synchronised with it in either direction.
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
pub mod clock;
pub mod config;
pub mod discord;
pub mod elevenlabs;
pub mod http;
pub mod mcp;
pub mod model;
pub mod ops;
pub mod probe;
pub mod retrieval;
pub mod state;
pub mod store;
pub mod summarize;
pub mod summary;
pub mod testing;
pub mod untrusted;
