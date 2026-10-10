//! One polling step: read the providers that are stale, append them to the history, refresh the
//! transcript index. Everything happens under the cache lock, so concurrent callers do not all
//! poll: the second one waits, re-reads the history and finds the first one's fresh sample.

use crate::history::{self, Lock};
use crate::model::Sample;
use crate::paths::{claude_dir, ensure_dir, CachePaths, Env};
use crate::transcripts::{Index, ScanStats};
use crate::{claude, codex};

/// What to do.
#[derive(Debug, Clone)]
pub struct PollOptions {
    /// Providers to consider.
    pub providers: Vec<&'static str>,
    /// Reuse a sample younger than this many seconds; `None` never polls (cached only); `Some(0)`
    /// always polls.
    pub max_age: Option<i64>,
    /// Refresh the Claude transcript index.
    pub scan_transcripts: bool,
}

/// What a step produced.
#[derive(Debug, Clone)]
pub struct PollResult {
    /// The history after the step (including the samples just taken).
    pub samples: Vec<Sample>,
    /// Providers polled in this step.
    pub fresh: Vec<&'static str>,
    /// The transcript index, when scanned.
    pub index: Option<Index>,
    /// What the scan read.
    pub scan: Option<ScanStats>,
}

/// Take one reading of `provider`.
pub fn read_provider(provider: &str, env: Env, now: i64) -> Sample {
    match provider {
        "claude" => claude::read(env, now),
        _ => codex::read(env, now),
    }
}

/// Run one step.
pub fn step(
    paths: &CachePaths,
    env: Env,
    opts: &PollOptions,
    now: i64,
) -> Result<PollResult, String> {
    ensure_dir(&paths.dir)?;
    let _lock = Lock::acquire(&paths.lock())?;
    let mut samples = history::read(&paths.history());
    let mut fresh = Vec::new();
    let mut taken = Vec::new();
    if let Some(max_age) = opts.max_age {
        for &provider in &opts.providers {
            let stale = history::latest(&samples, provider).is_none_or(|s| now - s.ts >= max_age);
            if stale {
                taken.push(read_provider(provider, env, now));
                fresh.push(provider);
            }
        }
    }
    if !taken.is_empty() {
        history::append(&paths.history(), &taken)?;
        samples.extend(taken);
        samples.sort_by_key(|s| s.ts);
    }
    let (index, scan) = if opts.scan_transcripts && opts.providers.contains(&"claude") {
        let mut index = Index::load(&paths.claude_index());
        let stats = match claude_dir(env) {
            Ok(dir) => index.scan(&dir.join("projects"), now),
            Err(_) => ScanStats::default(),
        };
        index.save(&paths.claude_index())?;
        (Some(index), Some(stats))
    } else {
        (None, None)
    };
    Ok(PollResult {
        samples,
        fresh,
        index,
        scan,
    })
}
