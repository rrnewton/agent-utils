//! The names of GitHub threads, learned off the page and read on it. `#1058 pr-name-enrichment`.
//!
//! # Why
//!
//! [`crate::link_labels`] draws a thread page as `REPO#N`, which says which thread and nothing
//! about it, and a status line naming a dozen of them reads as a dozen numbers. The owner:
//! agents should refer to PRs by name rather than just number, uniformly, and "if we could store
//! it in a machine readable place, maybe we could put the inclusion of the pr name into the
//! hyperlink". This is that machine-readable place: a cache of thread names the renderer reads,
//! so the name is there whether or not the agent that wrote the link remembered to write it.
//!
//! # Reading is cheap; learning is somebody else's job
//!
//! Nothing here is fetched on a path that serves a page. [`name_for`] is a memory lookup: a
//! cached name, or `None` and the plain `REPO#N` of before. A miss on a REAL page read records
//! the thread in a bounded wanted set, and one background task ([`Names::run`]) learns wanted
//! names from GitHub's API through the documents fetcher — the same token
//! (`[documents] github_token_file` / `github_token_command`) and the same `https_proxy` —
//! under a token bucket of about one request a second, a small burst, and a hard hourly
//! ceiling, because that token is shared with the documents fetcher and the owner's other
//! tools. The wanted set is newest first, so what is on the screen is learned before history
//! scrolled past. Only real page reads may want: the search index, summaries, the spoken form
//! and every startup or backfill render look names up without ever queueing a fetch
//! ([`cached_name`], and `queue = false` in [`name_for`]).
//!
//! HTML is rendered on every read, and the render cache's key holds this cache's
//! [`stamp`], so a name learned in the background is drawn from the next read of the
//! message on. A message already on the screen is not patched where it sits.
//!
//! # What a name is, and how long the answer stands
//!
//! A curated short name from `[render] pr_names_file` — a JSON object from `OWNER/REPO#N` to a
//! name — wins over anything fetched; that file is the seam a later glossary takes over, and
//! this server never writes it. Otherwise the name is the thread's GitHub title, made plain
//! and cut by [`crate::link_labels::shorten_name`]. A name is learned once and reused: an open
//! thread's for a day (its title may still be edited), a closed or merged one's for a month.
//! "No such thread" (a 404, which is also what a private thread the token cannot see answers)
//! stands for a day, and a failure — no network, GitHub down, rate limited — for five minutes,
//! so an unreachable GitHub costs at most those retries and never an error or a wait. A stale
//! name is still shown while it is being refreshed, and a 404 or a failure never erases a name
//! already learned.
//!
//! The cache is bounded (entries and wanted both), persisted as one JSON file beside the
//! store — `pr-names-cache.json`, as the documents fold keeps `documents-index.json` — or
//! wherever `[render] pr_names_cache_file` names (the shared file other readers of the
//! format look in), and memory-only in a deployment with neither. Turning the whole thing off is
//! `[render] pr_names = false`: nothing is installed, nothing is fetched, and labels are
//! exactly the address-derived ones of `#227 github-link-abbrev`.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::{Duration, Instant};

use crate::documents::{GithubFetcher, IssueTitle};
use crate::link_labels::{shorten_name, ThreadKind, ThreadRef};

/// The most learned names (and non-names) held at once. Past it, the oldest-learned go first.
pub const MAX_ENTRIES: usize = 4096;
/// The most threads awaiting a fetch at once. Past it, the oldest-wanted go first: the set is
/// newest first, so what this sheds is history scrolled past, not what is on the screen.
pub const MAX_WANTED: usize = 1024;

/// How long an open thread's name stands before it is learned again, in seconds.
const TTL_OPEN_SECS: u64 = 24 * 60 * 60;
/// How long a closed or merged thread's name stands: its title is history now.
const TTL_CLOSED_SECS: u64 = 30 * 24 * 60 * 60;
/// How long "no such thread" stands.
const TTL_MISSING_SECS: u64 = 24 * 60 * 60;
/// How long a failure stands before wanting the thread may queue it again.
const TTL_UNAVAILABLE_SECS: u64 = 5 * 60;

/// Requests the bucket lets through at once before the sustained rate applies.
const BURST: f64 = 5.0;
/// The sustained request rate, per second.
const PER_SECOND: f64 = 1.0;
/// The hard ceiling on requests in any hour. The token is the owner's and shared.
const PER_HOUR: usize = 600;

/// Now, as seconds since the Unix epoch: the clock every standing answer is dated by.
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The cache key `thread` is held under: `owner/repo#number`, the account and repository in
/// lowercase (GitHub reads them in any case; a cache must pick one) and the number without
/// leading zeros.
fn key_of(thread: &ThreadRef) -> String {
    format!(
        "{}/{}#{}",
        thread.owner.to_ascii_lowercase(),
        thread.repo.to_ascii_lowercase(),
        plain_number(&thread.number)
    )
}

/// A number as a key holds it: leading zeros off, and all zeros as one zero.
fn plain_number(number: &str) -> &str {
    match number.trim_start_matches('0') {
        "" => "0",
        rest => rest,
    }
}

/// A cache key read back into the parts a fetch is made with.
fn key_parts(key: &str) -> Option<(&str, &str, &str)> {
    let (owner, rest) = key.split_once('/')?;
    let (repo, number) = rest.split_once('#')?;
    Some((owner, repo, number))
}

/// One learned answer about a thread.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Entry {
    /// Its name, already shortened, and whether GitHub had it closed when it was learned.
    Named { name: String, closed: bool, at: u64 },
    /// GitHub answered 404: no such thread, or none the token may see.
    Missing { at: u64 },
    /// The last attempt failed (offline, refused, rate limited). Held only briefly.
    Unavailable { at: u64 },
}

impl Entry {
    /// When the answer was learned.
    fn at(&self) -> u64 {
        match self {
            Entry::Named { at, .. } | Entry::Missing { at } | Entry::Unavailable { at } => *at,
        }
    }

    /// Whether the answer still stands at `now`, by the lifetime of its kind.
    fn fresh(&self, now: u64) -> bool {
        let ttl = match self {
            Entry::Named { closed: true, .. } => TTL_CLOSED_SECS,
            Entry::Named { closed: false, .. } => TTL_OPEN_SECS,
            Entry::Missing { .. } => TTL_MISSING_SECS,
            Entry::Unavailable { .. } => TTL_UNAVAILABLE_SECS,
        };
        now.saturating_sub(self.at()) < ttl
    }
}

/// The names themselves, with no clock of its own and no network in it: every method is told
/// the time, so the whole of it is tested without waiting or asking GitHub anything.
#[derive(Default)]
struct Cache {
    entries: HashMap<String, Entry>,
    /// Curated names by key: read from a file this never writes, and winning over `entries`.
    curated: HashMap<String, String>,
    /// Keys awaiting a fetch, oldest at the front; taken from the back, newest first.
    wanted: VecDeque<String>,
    wanted_set: HashSet<String>,
    /// How many times a name a reader can see has changed. The render cache keys on it.
    generation: u64,
}

impl Cache {
    /// The name to show for `thread` at `now`, queueing it for the fetcher when there is none
    /// (or only a stale one) and `queue` allows. Answers also whether this call newly wanted
    /// it, so the caller can wake the fetcher.
    fn name(&mut self, thread: &ThreadRef, now: u64, queue: bool) -> (Option<String>, bool) {
        // A discussion's title is not on the issues endpoint (it answers 404 there, though
        // the number is the same sequence), so wanting one could only ever learn "missing":
        // a discussion is named from the curated file or not at all, until somebody teaches
        // the fetcher the GraphQL question.
        let queue = queue && thread.kind != ThreadKind::Discussion;
        let key = key_of(thread);
        if let Some(name) = self.curated.get(&key) {
            return (Some(name.clone()), false);
        }
        match self.entries.get(&key) {
            Some(Entry::Named { name, .. }) => {
                let name = name.clone();
                let stale = !self.entries[&key].fresh(now);
                // A stale name is shown while it is refreshed, never blanked for the wait.
                let wanted = stale && queue && self.want(&key);
                (Some(name), wanted)
            }
            Some(entry) if entry.fresh(now) => (None, false),
            _ => (None, queue && self.want(&key)),
        }
    }

    /// The name already held for `thread`, fresh or stale, queueing nothing: for readers that
    /// may not cause a fetch (the spoken form, the index, a summary).
    fn cached_name(&self, thread: &ThreadRef) -> Option<String> {
        let key = key_of(thread);
        if let Some(name) = self.curated.get(&key) {
            return Some(name.clone());
        }
        match self.entries.get(&key) {
            Some(Entry::Named { name, .. }) => Some(name.clone()),
            _ => None,
        }
    }

    /// Queue `key` for the fetcher. Answers whether it is newly queued.
    fn want(&mut self, key: &str) -> bool {
        if !self.wanted_set.insert(key.to_owned()) {
            return false;
        }
        self.wanted.push_back(key.to_owned());
        while self.wanted.len() > MAX_WANTED {
            if let Some(oldest) = self.wanted.pop_front() {
                self.wanted_set.remove(&oldest);
            }
        }
        true
    }

    /// Put `key` back for a later attempt, at the front: it was wanted before whatever is
    /// queued now, and its failure's lifetime decides when it is taken again.
    fn requeue(&mut self, key: &str) {
        if self.wanted_set.insert(key.to_owned()) {
            self.wanted.push_front(key.to_owned());
        }
    }

    /// The next key worth fetching at `now`, newest wanted first. A key whose standing answer
    /// is a name or a "no such thread" needs nothing and is dropped where it stands — a later
    /// read wants it again when the answer goes stale. One whose last attempt failed within
    /// its five minutes is not asked again yet, but stays wanted (set aside this round, behind
    /// everything fetchable) so it is retried when the lifetime ends even if nothing renders
    /// it again first.
    fn next_wanted(&mut self, now: u64) -> Option<String> {
        let mut waiting: Vec<String> = Vec::new();
        let found = loop {
            let Some(key) = self.wanted.pop_back() else {
                break None;
            };
            if self.curated.contains_key(&key) {
                self.wanted_set.remove(&key);
                continue;
            }
            match self.entries.get(&key) {
                Some(Entry::Unavailable { .. }) if self.entries[&key].fresh(now) => {
                    waiting.push(key);
                }
                Some(entry) if entry.fresh(now) => {
                    self.wanted_set.remove(&key);
                }
                _ => {
                    self.wanted_set.remove(&key);
                    break Some(key);
                }
            }
        };
        // Set-aside failures go back at the old end, in the order they were wanted.
        for key in waiting {
            self.wanted.push_front(key);
        }
        found
    }

    /// How many threads are waiting, worth fetching or not (tests and one log line).
    fn wanted_len(&self) -> usize {
        self.wanted.len()
    }

    /// Record a learned name: GitHub's `title` for `key`, and whether it had the thread
    /// closed. The visible name changing is what advances the generation.
    fn record_found(&mut self, key: &str, title: &str, closed: bool, now: u64) {
        let name = shorten_name(title);
        if name.is_empty() {
            // A title with nothing showable in it teaches no name: it stands as a failure
            // would, briefly and never erasing a name already learned.
            self.record_unavailable(key, now);
            return;
        }
        let visible_before = self.visible_name(key);
        self.entries.insert(
            key.to_owned(),
            Entry::Named {
                name,
                closed,
                at: now,
            },
        );
        if self.visible_name(key) != visible_before {
            self.generation += 1;
        }
        self.evict();
    }

    /// Record a 404, unless a name is already held: losing sight of a thread (a token's reach
    /// shrank, a repository moved) must not unname what was already learned from it.
    fn record_missing(&mut self, key: &str, now: u64) {
        if matches!(self.entries.get(key), Some(Entry::Named { .. })) {
            return;
        }
        self.entries
            .insert(key.to_owned(), Entry::Missing { at: now });
        self.evict();
    }

    /// Record a failed attempt, under the same rule as [`Cache::record_missing`].
    fn record_unavailable(&mut self, key: &str, now: u64) {
        if matches!(self.entries.get(key), Some(Entry::Named { .. })) {
            return;
        }
        self.entries
            .insert(key.to_owned(), Entry::Unavailable { at: now });
        self.evict();
    }

    /// The name a reader sees for `key` now, curated first.
    fn visible_name(&self, key: &str) -> Option<String> {
        if let Some(name) = self.curated.get(key) {
            return Some(name.clone());
        }
        match self.entries.get(key) {
            Some(Entry::Named { name, .. }) => Some(name.clone()),
            _ => None,
        }
    }

    /// Hold the bound: past [`MAX_ENTRIES`], the oldest-learned answers go first.
    fn evict(&mut self) {
        while self.entries.len() > MAX_ENTRIES {
            let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.at())
                .map(|(key, _)| key.clone())
            else {
                return;
            };
            self.entries.remove(&oldest);
        }
    }
}

/// The fetcher's budget: a token bucket refilling at [`PER_SECOND`] up to a [`BURST`], under a
/// hard ceiling of [`PER_HOUR`] requests in any hour. Told the time at every step, like
/// [`Cache`], so its arithmetic is tested without sleeping.
struct Bucket {
    tokens: f64,
    refilled: Instant,
    /// When each request in the last hour was made, oldest first.
    hour: VecDeque<Instant>,
    burst: f64,
    per_second: f64,
    per_hour: usize,
    /// Whether the current run of waiting has been logged: one line per episode, not per wait.
    throttled: bool,
}

impl Bucket {
    fn new(now: Instant) -> Self {
        Self::with_limits(now, BURST, PER_SECOND, PER_HOUR)
    }

    fn with_limits(now: Instant, burst: f64, per_second: f64, per_hour: usize) -> Self {
        Self {
            tokens: burst,
            refilled: now,
            hour: VecDeque::new(),
            burst,
            per_second,
            per_hour,
            throttled: false,
        }
    }

    /// Age the bucket to `now`: refill its tokens and forget requests an hour old.
    fn age(&mut self, now: Instant) {
        let elapsed = now.saturating_duration_since(self.refilled).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.per_second).min(self.burst);
        self.refilled = now;
        while self
            .hour
            .front()
            .is_some_and(|at| now.saturating_duration_since(*at) >= Duration::from_secs(3600))
        {
            self.hour.pop_front();
        }
    }

    /// How long until a request may be made at `now`, or `None` when it may be made at once.
    fn wait(&mut self, now: Instant) -> Option<Duration> {
        self.age(now);
        if self.hour.len() >= self.per_hour {
            let oldest = *self.hour.front()?;
            let until = oldest + Duration::from_secs(3600);
            return Some(
                until
                    .saturating_duration_since(now)
                    .max(Duration::from_secs(1)),
            );
        }
        if self.tokens >= 1.0 {
            return None;
        }
        Some(Duration::from_secs_f64(
            ((1.0 - self.tokens) / self.per_second).max(0.05),
        ))
    }

    /// Spend one request's worth of budget at `now`. Only called when [`Bucket::wait`] said
    /// none; a pass ends any throttling episode, so the next one is logged as its own.
    fn take(&mut self, now: Instant) {
        self.age(now);
        self.tokens = (self.tokens - 1.0).max(0.0);
        self.hour.push_back(now);
        self.throttled = false;
    }

    /// Note that the fetcher had to wait. Answers whether this wait opens an episode worth
    /// its one log line.
    fn note_throttled(&mut self) -> bool {
        let first = !self.throttled;
        self.throttled = true;
        first
    }
}

/// The persisted cache: each standing answer worth keeping across a restart. A failure is not
/// one of them — five minutes is shorter than most restarts.
#[derive(serde::Serialize, serde::Deserialize)]
struct Persisted {
    version: u32,
    entries: BTreeMap<String, Saved>,
}

/// One persisted answer: a name (absent for "no such thread") and when it was learned.
#[derive(serde::Serialize, serde::Deserialize)]
struct Saved {
    name: Option<String>,
    closed: bool,
    at: u64,
}

impl Cache {
    /// This cache as it is written to disk.
    fn persisted(&self) -> Persisted {
        let entries = self
            .entries
            .iter()
            .filter_map(|(key, entry)| match entry {
                Entry::Named { name, closed, at } => Some((
                    key.clone(),
                    Saved {
                        name: Some(name.clone()),
                        closed: *closed,
                        at: *at,
                    },
                )),
                Entry::Missing { at } => Some((
                    key.clone(),
                    Saved {
                        name: None,
                        closed: false,
                        at: *at,
                    },
                )),
                Entry::Unavailable { .. } => None,
            })
            .collect();
        Persisted {
            version: 1,
            entries,
        }
    }

    /// Take in what a previous run of the server persisted.
    fn restore(&mut self, persisted: Persisted) {
        for (key, saved) in persisted.entries {
            let entry = match saved.name {
                Some(name) if !name.is_empty() => Entry::Named {
                    name,
                    closed: saved.closed,
                    at: saved.at,
                },
                _ => Entry::Missing { at: saved.at },
            };
            self.entries.insert(key, entry);
        }
        self.evict();
    }
}

/// The name service a whole server shares: the cache behind a lock, the file it persists to,
/// and the bell a fresh want rings for the fetcher.
pub struct Names {
    /// This service's own number in the process: half of its [`Names::stamp`], so two caches
    /// never share a stamp at the same generation. The server installs exactly one; tests on
    /// parallel threads hold one each, beside a render cache the whole process shares.
    instance: u64,
    cache: Mutex<Cache>,
    wake: tokio::sync::Notify,
    path: Option<PathBuf>,
}

/// The next [`Names`] instance number in this process. Zero is never given out: it is the
/// stamp of no service installed at all.
static NEXT_INSTANCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl Names {
    /// An empty service persisting nowhere, with `curated` names already in it.
    #[must_use]
    pub fn new(curated: HashMap<String, String>) -> Self {
        Self {
            instance: NEXT_INSTANCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            cache: Mutex::new(Cache {
                curated,
                ..Cache::default()
            }),
            wake: tokio::sync::Notify::new(),
            path: None,
        }
    }

    /// The service for a server: persisted names read back from `path` when there are any. A
    /// file that cannot be read is said so once and the cache starts empty — names are a
    /// reading aid, never a reason the server does not start.
    #[must_use]
    pub fn load(path: Option<PathBuf>, curated: HashMap<String, String>) -> Arc<Self> {
        let mut names = Self::new(curated);
        if let Some(path) = &path {
            match std::fs::read_to_string(path) {
                Ok(text) => match serde_json::from_str::<Persisted>(&text) {
                    Ok(persisted) if persisted.version == 1 => {
                        names
                            .cache
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .restore(persisted);
                    }
                    Ok(persisted) => {
                        tracing::warn!(
                            version = persisted.version,
                            path = %path.display(),
                            "pr-names cache is of a version this build does not read; starting empty"
                        );
                    }
                    Err(error) => {
                        tracing::warn!(
                            %error,
                            path = %path.display(),
                            "pr-names cache not loaded; starting empty"
                        );
                    }
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    tracing::warn!(
                        %error,
                        path = %path.display(),
                        "pr-names cache not loaded; starting empty"
                    );
                }
            }
        }
        names.path = path;
        Arc::new(names)
    }

    /// The name to show for `thread`. With `queue`, a miss wants the thread fetched in the
    /// background; only a real page read passes `queue = true`. Never waits on the network.
    #[must_use]
    pub fn name_for(&self, thread: &ThreadRef, queue: bool) -> Option<String> {
        let (name, wanted) =
            self.cache
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .name(thread, now_secs(), queue);
        if wanted {
            self.wake.notify_one();
        }
        name
    }

    /// The name already held for `thread`, queueing nothing.
    #[must_use]
    pub fn cached_name(&self, thread: &ThreadRef) -> Option<String> {
        self.cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .cached_name(thread)
    }

    /// How many times a visible name has changed in this cache.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .generation
    }

    /// This cache at this generation, as one number another cache at the same generation
    /// never also holds: the render cache keys a drawn body by it.
    #[must_use]
    pub fn stamp(&self) -> u64 {
        self.instance.wrapping_shl(32) | (self.generation() & 0xffff_ffff)
    }

    /// Learn `thread`'s name out of thin air, for tests in other modules that need a name to
    /// be held (and the generation to advance) without a GitHub to ask.
    #[cfg(test)]
    pub(crate) fn learn(&self, thread: &ThreadRef, title: &str) {
        let key = key_of(thread);
        self.cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .record_found(&key, title, false, now_secs());
    }

    /// How many threads are awaiting the fetcher (tests).
    #[must_use]
    pub fn wanted_len(&self) -> usize {
        self.cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .wanted_len()
    }

    /// Write the standing answers to the cache file, tmp-then-rename so a crash mid-write
    /// leaves the last whole file. No file configured, nothing written; a failure is one
    /// warning, and the names live on in memory.
    fn persist(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let persisted = self
            .cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .persisted();
        let written = serde_json::to_string(&persisted)
            .map_err(|e| e.to_string())
            .and_then(|text| {
                let tmp = path.with_extension("json.tmp");
                std::fs::write(&tmp, text)
                    .and_then(|()| std::fs::rename(&tmp, path))
                    .map_err(|e| e.to_string())
            });
        if let Err(error) = written {
            tracing::warn!(%error, path = %path.display(), "pr-names cache not written");
        }
    }

    /// Learn wanted names until the process ends: the one background task of this module.
    ///
    /// Each name is one API request through `fetcher`, paced by its request bucket. GitHub saying
    /// "rate limited" puts the thread back and ends the round for the failure lifetime — every
    /// other answer would be the same — and an ordinary failure is recorded and moved past.
    /// Whatever was learned in a round is persisted at its end.
    pub async fn run(self: Arc<Self>, fetcher: GithubFetcher) {
        let mut bucket = Bucket::new(Instant::now());
        let mut limited = false;
        loop {
            let mut learned = false;
            'drain: loop {
                let Some(key) = self
                    .cache
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .next_wanted(now_secs())
                else {
                    break 'drain;
                };
                // The budget is waited out holding the key: it was the newest wanted when it
                // was picked, and nothing is spent until a request is really made.
                while let Some(wait) = bucket.wait(Instant::now()) {
                    if bucket.note_throttled() {
                        tracing::info!(
                            wanted = self.wanted_len(),
                            "pr-names fetcher throttled to its request budget; names follow as it allows"
                        );
                    }
                    tokio::time::sleep(wait).await;
                }
                bucket.take(Instant::now());
                let Some((owner, repo, number)) = key_parts(&key) else {
                    continue 'drain;
                };
                let (owner, repo, number) = (owner.to_owned(), repo.to_owned(), number.to_owned());
                match fetcher.issue_title(&owner, &repo, &number).await {
                    Ok(IssueTitle::Found { title, closed }) => {
                        self.cache
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .record_found(&key, &title, closed, now_secs());
                        learned = true;
                        limited = false;
                    }
                    Ok(IssueTitle::NotFound) => {
                        self.cache
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .record_missing(&key, now_secs());
                        learned = true;
                        limited = false;
                    }
                    Ok(IssueTitle::RateLimited) => {
                        {
                            let mut cache = self.cache.lock().unwrap_or_else(|p| p.into_inner());
                            cache.record_unavailable(&key, now_secs());
                            cache.requeue(&key);
                        }
                        if !limited {
                            tracing::warn!(
                                "GitHub rate-limited the pr-names fetcher; pausing it for five minutes"
                            );
                            limited = true;
                        }
                        break 'drain;
                    }
                    Err(error) => {
                        tracing::debug!(%error, key, "pr-names fetch failed");
                        self.cache
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .record_unavailable(&key, now_secs());
                    }
                }
            }
            if learned {
                self.persist();
            }
            // Sleep until a read wants something new. A rate-limited round also wakes here,
            // but its wanted keys are inside their failure lifetime, so the next round drops
            // them and sleeps again rather than asking GitHub early.
            tokio::select! {
                () = self.wake.notified() => {}
                () = tokio::time::sleep(Duration::from_secs(60)) => {}
            }
        }
    }
}

/// Read the curated names from `path`: a JSON object from `OWNER/REPO#N` to a short name.
/// Keys are read as cache keys (case aside) and names made plain by
/// [`crate::link_labels::shorten_name`]; a row that is neither is skipped and said so once. An
/// unreadable or unparseable file names nothing and is one warning — curated names are an
/// operator's extra, never a reason the server does not start.
#[must_use]
pub fn load_curated(path: &Path) -> HashMap<String, String> {
    let read = std::fs::read_to_string(path)
        .map_err(|e| e.to_string())
        .and_then(|text| {
            serde_json::from_str::<HashMap<String, String>>(&text).map_err(|e| e.to_string())
        });
    let rows = match read {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "curated pr names not loaded");
            return HashMap::new();
        }
    };
    let mut names = HashMap::new();
    let mut skipped = 0;
    for (key, name) in rows {
        let Some(parsed) = parse_curated_key(&key) else {
            skipped += 1;
            continue;
        };
        let name = shorten_name(&name);
        if name.is_empty() {
            skipped += 1;
            continue;
        }
        names.insert(parsed, name);
    }
    if skipped > 0 {
        tracing::warn!(
            skipped,
            path = %path.display(),
            "curated pr names held rows that are not OWNER/REPO#N to a name; skipped"
        );
    }
    names
}

/// A curated key as a cache key, or `None` when it is not `OWNER/REPO#N` shaped. The parts
/// need only be the plain characters a key made from an address holds, so a file can never
/// hold a key no address would ever ask for in a shape this cannot see.
fn parse_curated_key(raw: &str) -> Option<String> {
    let (owner, repo, number) = key_parts(raw.trim())?;
    let plain = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    };
    if !plain(owner)
        || !plain(repo)
        || number.is_empty()
        || !number.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    Some(format!(
        "{}/{}#{}",
        owner.to_ascii_lowercase(),
        repo.to_ascii_lowercase(),
        plain_number(number)
    ))
}

/// The service for a whole server configuration, or `None` when `[render] pr_names` is off:
/// the cache persists beside the store unless there is no store, beside the documents fold's
/// file, and curated names come from `[render] pr_names_file` when it is set.
#[must_use]
pub fn service_for(config: &crate::config::Config) -> Option<Arc<Names>> {
    if !config.render.pr_names {
        return None;
    }
    let curated = match &config.render.pr_names_file {
        Some(path) => load_curated(path),
        None => HashMap::new(),
    };
    Some(Names::load(cache_path(config), curated))
}

/// Where the learned names persist for `config`: `[render] pr_names_cache_file` when it names
/// one — the place other readers of the shared format look, conventionally
/// `${XDG_STATE_HOME:-~/.local/state}/agent-utils/pr-names-cache.json` — else beside the
/// store, and nowhere (memory only) when there is no store either.
fn cache_path(config: &crate::config::Config) -> Option<PathBuf> {
    if let Some(path) = &config.render.pr_names_cache_file {
        return Some(path.clone());
    }
    config
        .storage
        .path
        .as_ref()
        .and_then(|store| store.parent().map(|dir| dir.join("pr-names-cache.json")))
}

/// The GitHub fetcher the name learner asks through: the documents fetcher's own token and
/// proxy, against the API. See [`crate::documents::GithubFetcher::issue_title`].
#[must_use]
pub fn fetcher_for(config: &crate::config::Config) -> GithubFetcher {
    GithubFetcher::new(
        crate::documents::token_source_from_config(&config.documents),
        None,
        config.documents.https_proxy.clone(),
    )
}

// --- the installed service ---------------------------------------------------------------

/// The names this process reads: none until [`install`], as the reference links are none until
/// theirs are set. Free functions reach the renderer and the spoken form, which no `AppState`
/// reaches, exactly why the renderer's other configuration lives the same way.
static INSTALLED: LazyLock<RwLock<Option<Arc<Names>>>> = LazyLock::new(|| RwLock::new(None));

/// Read names from `names` in every body rendered or spoken after this. Called once at
/// startup when `[render] pr_names` is on.
pub fn install(names: Arc<Names>) {
    *INSTALLED
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(names);
}

#[cfg(test)]
thread_local! {
    /// A test's own names, on its own thread, so tests with different names run side by side.
    static TEST_NAMES: std::cell::RefCell<Option<Arc<Names>>> =
        const { std::cell::RefCell::new(None) };
}

/// This thread reads a test's own names until the guard is dropped.
#[cfg(test)]
pub(crate) struct NamesGuard(Option<Arc<Names>>);

#[cfg(test)]
impl NamesGuard {
    pub(crate) fn set(names: Arc<Names>) -> Self {
        Self(TEST_NAMES.with(|cell| cell.replace(Some(names))))
    }
}

#[cfg(test)]
impl Drop for NamesGuard {
    fn drop(&mut self) {
        TEST_NAMES.with(|cell| *cell.borrow_mut() = self.0.take());
    }
}

/// The installed names, when any are.
#[must_use]
pub fn names() -> Option<Arc<Names>> {
    #[cfg(test)]
    if let Some(names) = TEST_NAMES.with(|cell| cell.borrow().clone()) {
        return Some(names);
    }
    INSTALLED
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// The name to show for `thread` from the installed service; `queue` only from a real page
/// read. `None` when no service is installed or no name is held yet.
#[must_use]
pub fn name_for(thread: &ThreadRef, queue: bool) -> Option<String> {
    names()?.name_for(thread, queue)
}

/// The name already held for `thread`, queueing nothing.
#[must_use]
pub fn cached_name(thread: &ThreadRef) -> Option<String> {
    names()?.cached_name(thread)
}

/// The installed cache's stamp, or 0 when none is installed: what the render cache keys a
/// body by beside its text, so a name learned later is drawn on the next read.
#[must_use]
pub fn stamp() -> u64 {
    names().map_or(0, |names| names.stamp())
}

#[cfg(test)]
mod tests {
    use super::{
        key_of, load_curated, Bucket, Cache, Names, MAX_ENTRIES, MAX_WANTED, TTL_CLOSED_SECS,
        TTL_MISSING_SECS, TTL_OPEN_SECS, TTL_UNAVAILABLE_SECS,
    };
    use crate::link_labels::{ThreadKind, ThreadRef};
    use std::collections::HashMap;
    use std::time::{Duration, Instant};

    fn thread(number: &str) -> ThreadRef {
        ThreadRef {
            owner: "Octo".to_owned(),
            repo: "Widgets".to_owned(),
            number: number.to_owned(),
            kind: ThreadKind::Pull,
        }
    }

    fn key(number: &str) -> String {
        key_of(&thread(number))
    }

    #[test]
    fn a_key_is_owner_repo_number_in_one_case() {
        assert_eq!(key("5275"), "octo/widgets#5275");
        assert_eq!(key("05275"), "octo/widgets#5275");
    }

    #[test]
    fn a_miss_wants_the_thread_once_and_only_when_queueing_is_allowed() {
        let mut cache = Cache::default();
        // Lookup-only readers never want.
        assert_eq!(cache.name(&thread("41"), 100, false), (None, false));
        assert_eq!(cache.wanted_len(), 0);
        assert_eq!(cache.cached_name(&thread("41")), None);
        // A real read wants it, once however often the body is drawn.
        assert_eq!(cache.name(&thread("41"), 100, true), (None, true));
        assert_eq!(cache.name(&thread("41"), 101, true), (None, false));
        assert_eq!(cache.wanted_len(), 1);
        // The fetch lands; the name stands and nothing is wanted any more.
        assert_eq!(cache.next_wanted(102).as_deref(), Some("octo/widgets#41"));
        cache.record_found("octo/widgets#41", "Repair the sprocket", false, 103);
        assert_eq!(
            cache.name(&thread("41"), 104, true),
            (Some("Repair the sprocket".to_owned()), false)
        );
        assert_eq!(cache.wanted_len(), 0);
    }

    #[test]
    fn an_answer_stands_for_its_lifetime_and_a_stale_name_is_shown_while_refreshed() {
        let mut cache = Cache::default();
        let learned = 10_000;
        cache.record_found(&key("1"), "open thread", false, learned);
        cache.record_found(&key("2"), "closed thread", true, learned);
        cache.record_missing(&key("3"), learned);
        cache.record_unavailable(&key("4"), learned);
        // Within every lifetime: names shown, nothing wanted.
        let soon = learned + TTL_UNAVAILABLE_SECS - 1;
        for number in ["1", "2", "3", "4"] {
            assert!(!cache.name(&thread(number), soon, true).1, "{number}");
        }
        // The failure's five minutes end first: wanted again, still no name.
        let at = learned + TTL_UNAVAILABLE_SECS;
        assert_eq!(cache.name(&thread("4"), at, true), (None, true));
        assert!(!cache.name(&thread("1"), at, true).1);
        // An open thread's day ends: the stale name is still the answer, and a refresh wants.
        let at = learned + TTL_OPEN_SECS;
        assert_eq!(
            cache.name(&thread("1"), at, true),
            (Some("open thread".to_owned()), true)
        );
        assert_eq!(cache.name(&thread("3"), at, true), (None, true));
        assert!(!cache.name(&thread("2"), at, true).1);
        // A closed thread's month, and a 404's day (checked above at the same instant the open
        // name went stale, both being a day).
        let at = learned + TTL_CLOSED_SECS;
        assert!(cache.name(&thread("2"), at, true).1);
        assert_eq!(TTL_MISSING_SECS, TTL_OPEN_SECS);
        // Lookup-only still sees a stale name and wants nothing.
        assert_eq!(
            cache.cached_name(&thread("2")).as_deref(),
            Some("closed thread")
        );
    }

    #[test]
    fn a_discussion_is_never_fetched_but_a_curated_name_still_names_it() {
        let discussion = ThreadRef {
            kind: ThreadKind::Discussion,
            ..thread("41")
        };
        let mut cache = Cache::default();
        assert_eq!(cache.name(&discussion, 100, true), (None, false));
        assert_eq!(cache.wanted_len(), 0);
        let mut cache = Cache {
            curated: HashMap::from([(key("41"), "the roadmap chat".to_owned())]),
            ..Cache::default()
        };
        assert_eq!(
            cache.name(&discussion, 100, true),
            (Some("the roadmap chat".to_owned()), false)
        );
    }

    #[test]
    fn a_failure_or_a_404_never_erases_a_learned_name() {
        let mut cache = Cache::default();
        cache.record_found(&key("5"), "still here", false, 100);
        cache.record_unavailable(&key("5"), 200);
        cache.record_missing(&key("5"), 300);
        assert_eq!(
            cache.cached_name(&thread("5")).as_deref(),
            Some("still here")
        );
        // Nor does a later title with nothing showable in it: it teaches no name, and the
        // generation does not move for it either.
        let generation = cache.generation;
        cache.record_found(&key("5"), " \u{202e}\n", false, 400);
        assert_eq!(
            cache.cached_name(&thread("5")).as_deref(),
            Some("still here")
        );
        assert_eq!(cache.generation, generation);
        // With nothing held yet it is a brief failure, not a standing absence.
        cache.record_found(&key("6"), " \u{202e}\n", false, 400);
        assert_eq!(cache.name(&thread("6"), 401, true), (None, false));
        assert_eq!(
            cache.name(&thread("6"), 400 + TTL_UNAVAILABLE_SECS, true),
            (None, true)
        );
    }

    #[test]
    fn the_cache_file_is_the_named_one_else_beside_the_store() {
        let mut config = crate::testing::config();
        config.storage.path = Some(std::path::PathBuf::from("/tmp/vibe-test/store.sqlite3"));
        assert_eq!(
            super::cache_path(&config),
            Some(std::path::PathBuf::from(
                "/tmp/vibe-test/pr-names-cache.json"
            ))
        );
        config.render.pr_names_cache_file =
            Some(std::path::PathBuf::from("/tmp/elsewhere/names.json"));
        assert_eq!(
            super::cache_path(&config),
            Some(std::path::PathBuf::from("/tmp/elsewhere/names.json"))
        );
        config.render.pr_names_cache_file = None;
        config.storage.path = None;
        assert_eq!(
            super::cache_path(&config),
            None,
            "no store, no file: memory only"
        );
    }

    #[test]
    fn a_curated_name_wins_is_never_fetched_and_masks_generation() {
        let mut cache = Cache {
            curated: HashMap::from([(key("7"), "the sprocket one".to_owned())]),
            ..Cache::default()
        };
        assert_eq!(
            cache.name(&thread("7"), 100, true),
            (Some("the sprocket one".to_owned()), false)
        );
        let generation = cache.generation;
        cache.record_found(&key("7"), "Repair the sprocket properly", false, 101);
        // What a reader sees did not change, so no body needs redrawing.
        assert_eq!(cache.generation, generation);
        assert_eq!(
            cache.cached_name(&thread("7")).as_deref(),
            Some("the sprocket one")
        );
        // Even if something queued it before the curated file named it, it is not fetched.
        cache.want(&key("7"));
        assert_eq!(cache.next_wanted(102), None);
        assert_eq!(cache.wanted_len(), 0);
    }

    #[test]
    fn the_generation_advances_only_when_a_visible_name_changes() {
        let mut cache = Cache::default();
        assert_eq!(cache.generation, 0);
        cache.record_found(&key("9"), "first title", false, 100);
        assert_eq!(cache.generation, 1);
        cache.record_found(&key("9"), "first title", false, 200);
        assert_eq!(
            cache.generation, 1,
            "relearning the same name redraws nothing"
        );
        cache.record_found(&key("9"), "renamed title", false, 300);
        assert_eq!(cache.generation, 2);
        cache.record_missing(&key("10"), 100);
        cache.record_unavailable(&key("11"), 100);
        assert_eq!(cache.generation, 2, "non-names are not visible changes");
    }

    #[test]
    fn wanted_is_newest_first_bounded_and_waits_out_a_failure() {
        let mut cache = Cache::default();
        for number in 0..5 {
            cache.name(&thread(&number.to_string()), 100, true);
        }
        assert_eq!(cache.next_wanted(100).as_deref(), Some("octo/widgets#4"));
        assert_eq!(cache.next_wanted(100).as_deref(), Some("octo/widgets#3"));
        // A failure still in its five minutes is set aside, not dropped and not asked again,
        // while older wants behind it are still fetched.
        cache.record_unavailable(&key("2"), 100);
        assert_eq!(cache.next_wanted(100).as_deref(), Some("octo/widgets#1"));
        assert_eq!(cache.next_wanted(100).as_deref(), Some("octo/widgets#0"));
        assert_eq!(cache.next_wanted(100), None);
        assert_eq!(cache.wanted_len(), 1, "the failed thread stays wanted");
        let later = 100 + TTL_UNAVAILABLE_SECS;
        assert_eq!(cache.next_wanted(later).as_deref(), Some("octo/widgets#2"));
        // The bound sheds the oldest wants, never the newest.
        let mut cache = Cache::default();
        for number in 0..(MAX_WANTED + 10) {
            cache.want(&key(&number.to_string()));
        }
        assert_eq!(cache.wanted_len(), MAX_WANTED);
        let newest = (MAX_WANTED + 9).to_string();
        assert_eq!(
            cache.next_wanted(100).as_deref(),
            Some(key(&newest).as_str())
        );
    }

    #[test]
    fn entries_are_bounded_oldest_learned_first() {
        let mut cache = Cache::default();
        for number in 0..(MAX_ENTRIES + 5) {
            cache.record_found(
                &key(&number.to_string()),
                "a title",
                false,
                100 + number as u64,
            );
        }
        assert_eq!(cache.entries.len(), MAX_ENTRIES);
        assert_eq!(
            cache.cached_name(&thread("0")),
            None,
            "the oldest went first"
        );
        assert!(cache
            .cached_name(&thread(&(MAX_ENTRIES + 4).to_string()))
            .is_some());
    }

    #[test]
    fn the_bucket_bursts_then_paces_and_never_passes_its_hour() {
        let start = Instant::now();
        let mut bucket = Bucket::with_limits(start, 3.0, 1.0, 5);
        // The burst goes at once.
        for _ in 0..3 {
            assert_eq!(bucket.wait(start), None);
            bucket.take(start);
        }
        // Then about a second a request.
        let wait = bucket.wait(start).expect("paced");
        assert!(wait <= Duration::from_secs(1), "{wait:?}");
        let second = start + Duration::from_secs(1);
        assert_eq!(bucket.wait(second), None);
        bucket.take(second);
        let third = start + Duration::from_secs(3);
        assert_eq!(bucket.wait(third), None);
        bucket.take(third);
        // Five in the hour: the hard ceiling, however full the bucket meanwhile gets.
        let hour_later = start + Duration::from_secs(60);
        let wait = bucket.wait(hour_later).expect("the hourly ceiling");
        assert!(wait > Duration::from_secs(3000), "{wait:?}");
        let next_hour = start + Duration::from_secs(3601);
        assert_eq!(bucket.wait(next_hour), None);
        // One log line per throttling episode.
        let mut bucket = Bucket::with_limits(start, 1.0, 1.0, 100);
        bucket.take(start);
        assert!(bucket.wait(start).is_some());
        assert!(bucket.note_throttled());
        assert!(!bucket.note_throttled());
        bucket.take(start + Duration::from_secs(2));
        assert!(bucket.note_throttled(), "a pass ends the episode");
    }

    #[test]
    fn names_persist_and_a_corrupt_file_starts_empty() {
        let dir = std::env::temp_dir().join(format!(
            "vibe-talk-pr-names-test-{}-persist",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("test dir");
        let path = dir.join("pr-names-cache.json");
        {
            let names = Names::load(Some(path.clone()), HashMap::new());
            {
                let mut cache = names.cache.lock().expect("cache");
                cache.record_found(&key("41"), "Repair the sprocket", true, 500);
                cache.record_missing(&key("42"), 500);
                cache.record_unavailable(&key("43"), 500);
            }
            names.persist();
        }
        let reloaded = Names::load(Some(path.clone()), HashMap::new());
        assert_eq!(
            reloaded.cached_name(&thread("41")).as_deref(),
            Some("Repair the sprocket")
        );
        assert_eq!(reloaded.cached_name(&thread("42")), None);
        // The 404 persisted (it stands for a day); the failure did not.
        {
            let mut cache = reloaded.cache.lock().expect("cache");
            assert_eq!(cache.name(&thread("42"), 501, true), (None, false));
            assert_eq!(cache.name(&thread("43"), 501, true), (None, true));
        }
        std::fs::write(&path, "{ not json").expect("corrupt the file");
        let empty = Names::load(Some(path), HashMap::new());
        assert_eq!(empty.cached_name(&thread("41")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn curated_rows_are_read_as_keys_and_names_made_plain() {
        let dir = std::env::temp_dir().join(format!(
            "vibe-talk-pr-names-test-{}-curated",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("test dir");
        let path = dir.join("names.json");
        std::fs::write(
            &path,
            r#"{
                "Octo/Widgets#41": "  the   sprocket\none ",
                "octo/widgets#0042": "leading zeros",
                "not a key": "skipped",
                "octo/widgets#x": "skipped",
                "octo/widgets#43": "  "
            }"#,
        )
        .expect("write curated");
        let curated = load_curated(&path);
        assert_eq!(curated.len(), 2);
        assert_eq!(
            curated.get("octo/widgets#41").map(String::as_str),
            Some("the sprocket one")
        );
        assert_eq!(
            curated.get("octo/widgets#42").map(String::as_str),
            Some("leading zeros")
        );
        // An unreadable file names nothing rather than failing.
        assert!(load_curated(&dir.join("absent.json")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
