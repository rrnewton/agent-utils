//! `agent-teams`: message another team's coordinator, across hosts, exactly once.
//!
//! A team is a coordinator plus the workers it manages, on one host. Only
//! coordinators (and registered services) send team messages, addressed to
//! another team's coordinator. Delivery is behind the
//! [`deliver::Deliverer`] trait and transport behind the
//! [`route::Route`] trait, so the tool is independent of Herdr and agentctl:
//! a hook-inbox team needs neither.
//!
//! Design: `ai_docs/transient/2026-10-24-agentctl-team-messaging.md`.

pub mod cli;
pub mod deliver;
pub mod envelope;
pub mod hostindex;
pub mod jsonfile;
pub mod ledger;
pub mod model;
pub mod proof;
pub mod route;
pub mod sender;
pub mod serve;
pub mod timeutil;
