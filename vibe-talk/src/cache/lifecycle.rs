//! Opening the message cache, and every erase of its whole file. `#208 persistent-cache`.
//!
//! The configuration's `[message_cache]` table and the owner's switch (the [`marker`] sidecar
//! beside the store) together decide, at every start, whether a cache file is opened behind the
//! server's [`Sink`]:
//!
//! - `enabled = false`, or the owner's "off" recorded in the sidecar: every cache file this
//!   server ever made is erased, and none is opened;
//! - no table at all: none is opened and none erased, and a file left from an earlier
//!   configuration is warned about at every start until it has gone seven days without a table,
//!   when it is erased;
//! - `enabled = true` and no "off": the file is registered in the sidecar FIRST, so that a later
//!   "off" can find it whatever happens next, and then opened.
//!
//! A cache that cannot be opened leaves the server running without one, and says why in a code
//! with no path in it.
//!
//! # The owner's switch
//!
//! While the server runs, the owner may turn the cache off, turn it on again, or erase it and keep
//! it on (`PUT` and `DELETE /api/v1/message-cache`, write scope, no MCP tool):
//!
//! - **off** writes the marker FIRST and erases second ([`marker::switch_off`]): a marker that
//!   cannot be written erases nothing and is reported as a failure, and a crash after it erases at
//!   the next start. Every other file the registry names goes too, and the cache leaves the sink;
//! - **on** is refused while the configuration does not enable the cache — the operator's word
//!   wins — and otherwise clears the marker and opens a fresh file, whose providers' accounts are
//!   checked again before anything is filled ([`Sink::install`]);
//! - **erase** erases the open file and opens a fresh one in its place, and the accounts are
//!   checked again. With no cache open it erases nothing.
//!
//! The purge of the store (`DELETE /api/v1/storage`) erases the file as **erase** does, and every
//! other cache file this server made, and leaves the switch and the marker as they are
//! ([`Lifecycle::purge`]).
//!
//! One switch runs at a time. Each answers the [`MessageCacheStatus`] it left, which is also what
//! client-config carries: measured on the blocking pool, and kept for [`STATUS_FRESH_FOR`] in
//! between, since counting the file's rows costs a scan.
//!
//! Everything here that touches a file or SQLite runs on tokio's blocking pool, never on a
//! runtime worker: an open checks and may rebuild the file, and an erase waits for the writer and
//! for reads in flight.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use super::marker::{self, StartupAction};
use super::sink::Sink;
use super::{
    CacheError, CacheOptions, Clock, Counts, Limits, MessageCache, SearchTextSource, SystemClock,
};
use crate::config::{Config, MessageCacheConfig};
use crate::contract::MessageCacheStatus;

/// How long a measured status is answered again before the file is measured afresh. A switch
/// measures afresh at once.
pub const STATUS_FRESH_FOR: Duration = Duration::from_secs(30);

/// Why a switch of the cache was refused, or failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SwitchError {
    /// The configuration has no `[message_cache]` table: there is no cache to switch.
    #[error("this server's configuration has no [message_cache] table, so there is no message cache to switch")]
    NotConfigured,
    /// Turning it on, while the configuration turns it off.
    #[error("the server's configuration turns the message cache off ([message_cache] enabled = false); only the operator can turn it on")]
    NotEnabled,
    /// The marker beside the store could not be written: nothing was erased, and nothing changed.
    #[error("the message cache's switch could not be saved beside the store; nothing was erased and nothing changed")]
    Unsaved,
}

/// Why a cache the configuration enables is not open. A code, never a sentence with a path in it:
/// it is shown to the owner, and logged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trouble {
    /// The sidecar beside the store could not be read, so the owner's switch is unknown.
    SwitchUnreadable,
    /// The sidecar could not be written, so the file could not be registered before it opened.
    SwitchUnwritable,
    /// The linked SQLite lacks something the cache needs.
    SqliteUnsupported,
    /// The file at the cache's path is not one this server made; it is left as it is.
    ForeignFile,
    /// Another process, or another handle, holds the cache's path.
    Locked,
    /// The directory or the file could not be created, or is not private.
    Filesystem,
    /// SQLite failed.
    Sqlite,
}

impl Trouble {
    /// The code, as the status names it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SwitchUnreadable => "switch_unreadable",
            Self::SwitchUnwritable => "switch_unwritable",
            Self::SqliteUnsupported => "sqlite_unsupported",
            Self::ForeignFile => "foreign_file",
            Self::Locked => "locked",
            Self::Filesystem => "filesystem",
            Self::Sqlite => "sqlite",
        }
    }

    /// The code for an open that failed.
    #[must_use]
    pub const fn of(error: &CacheError) -> Self {
        match error {
            CacheError::Unavailable(_) => Self::SqliteUnsupported,
            CacheError::Foreign(_) => Self::ForeignFile,
            CacheError::Locked(_) => Self::Locked,
            CacheError::Filesystem(_) => Self::Filesystem,
            CacheError::Sqlite(_) => Self::Sqlite,
        }
    }
}

/// Where this server keeps what the cache needs on disk. There is none without a store: the
/// sidecar lives beside it, and the default cache path is named after it.
#[derive(Clone, Debug)]
struct Files {
    /// The durable store's file, which the cache must never be.
    store: PathBuf,
    /// The sidecar: the owner's switch and the registry of cache paths.
    sidecar: PathBuf,
    /// The cache file the configuration names, or the default one beside the store.
    cache: PathBuf,
    /// The store's directory, where a cache at the default path would be.
    default_dir: Option<PathBuf>,
}

impl Files {
    fn new(store: &Path, table: Option<&MessageCacheConfig>) -> Self {
        Self {
            store: store.to_owned(),
            sidecar: marker::sidecar_path(store),
            cache: table
                .and_then(|table| table.path.clone())
                .unwrap_or_else(|| super::file::default_path(store, &super::file::host_name())),
            default_dir: store
                .parent()
                .filter(|dir| !dir.as_os_str().is_empty())
                .map(Path::to_owned),
        }
    }
}

/// What the server knows about its cache file across its life: what the configuration says, where
/// the files are, the owner's switch, and why a cache that should be open is not.
pub struct Lifecycle {
    /// The `[message_cache]` table; `None` when the configuration has none.
    table: Option<MessageCacheConfig>,
    /// `None` without a store.
    files: Option<Files>,
    /// How bodies are rendered for the index of every cache this opens.
    search_text: Arc<dyn SearchTextSource>,
    /// Local time, for the caches this opens and the times the sidecar records.
    clock: Arc<dyn Clock>,
    /// The owner's switch, as the sidecar says.
    off: AtomicBool,
    /// Why no cache is open although the configuration enables one and the owner has not turned
    /// it off.
    trouble: Mutex<Option<Trouble>>,
    /// Held across each switch, an erase and the purge's erase: one at a time.
    serial: tokio::sync::Mutex<()>,
    /// The last status measured, and when.
    memo: Mutex<Memo>,
}

/// The last status measured. `round` moves whenever what it says may have changed, so a
/// measurement that started before a switch is answered once but never kept.
#[derive(Debug, Default)]
struct Memo {
    round: u64,
    kept: Option<(tokio::time::Instant, MessageCacheStatus)>,
}

impl std::fmt::Debug for Lifecycle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lifecycle")
            .field("table", &self.table.is_some())
            .field("off", &self.off.load(Ordering::SeqCst))
            .field("trouble", &self.trouble())
            .finish_non_exhaustive()
    }
}

impl Lifecycle {
    /// Whether the configuration has a `[message_cache]` table.
    #[must_use]
    pub fn configured(&self) -> bool {
        self.table.is_some()
    }

    /// Whether the configuration enables the cache and the owner has not turned it off.
    #[must_use]
    pub fn enabled(&self) -> bool {
        self.table.as_ref().is_some_and(|table| table.enabled) && !self.off.load(Ordering::SeqCst)
    }

    /// Why a cache that should be open is not, if it is not.
    #[must_use]
    pub fn trouble(&self) -> Option<Trouble> {
        *self.trouble.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn set_trouble(&self, trouble: Option<Trouble>) {
        *self.trouble.lock().unwrap_or_else(PoisonError::into_inner) = trouble;
    }

    fn memo(&self) -> std::sync::MutexGuard<'_, Memo> {
        self.memo.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The cache as Settings shows it, or `None` when the configuration has no table. Measured at
    /// most once per [`STATUS_FRESH_FOR`], on the blocking pool.
    pub async fn status(&self, sink: &Sink) -> Option<MessageCacheStatus> {
        let table = self.table.as_ref()?;
        Some(self.status_of(sink, table).await)
    }

    async fn status_of(&self, sink: &Sink, table: &MessageCacheConfig) -> MessageCacheStatus {
        let round = {
            let memo = self.memo();
            if let Some((at, status)) = &memo.kept {
                if at.elapsed() < STATUS_FRESH_FOR {
                    return status.clone();
                }
            }
            memo.round
        };
        let open = sink.cache().filter(|cache| cache.is_open());
        let available = open.is_some();
        let (bytes, counts, oldest) = match open {
            Some(cache) => tokio::task::spawn_blocking(move || {
                (
                    cache.bytes_on_disk(),
                    cache.counts().served().unwrap_or_default(),
                    cache.oldest_covered().served().flatten(),
                )
            })
            .await
            .unwrap_or_default(),
            None => (0, Counts::default(), None),
        };
        let reason = if !table.enabled {
            Some("config_disabled")
        } else if self.off.load(Ordering::SeqCst) {
            Some("switched_off")
        } else if available {
            None
        } else {
            Some(self.trouble().map_or("not_open", Trouble::as_str))
        };
        let status = MessageCacheStatus {
            available,
            enabled: self.enabled(),
            reason,
            bytes,
            messages: counts.messages,
            channels: counts.channels,
            oldest_covered: oldest.map(crate::clock::iso_from_ms),
            max_days: table.max_days,
            max_megabytes: table.max_megabytes,
        };
        let mut memo = self.memo();
        if memo.round == round {
            memo.kept = Some((tokio::time::Instant::now(), status.clone()));
        }
        status
    }

    /// Measure afresh at the next [`Lifecycle::status`], and keep nothing measured before now.
    fn forget_status(&self) {
        let mut memo = self.memo();
        memo.round += 1;
        memo.kept = None;
    }

    /// The status a switch answers, measured afresh now that it is done.
    async fn switched(&self, sink: &Sink, table: &MessageCacheConfig) -> MessageCacheStatus {
        self.forget_status();
        self.status_of(sink, table).await
    }

    /// The owner's "on". See the module documentation.
    ///
    /// # Errors
    ///
    /// [`SwitchError::NotConfigured`] without a table, [`SwitchError::NotEnabled`] when the
    /// configuration turns the cache off, and [`SwitchError::Unsaved`] when the marker could not
    /// be cleared. A file that then fails to open is not an error: the status says why.
    pub async fn switch_on(&self, sink: &Sink) -> Result<MessageCacheStatus, SwitchError> {
        let table = self.table.as_ref().ok_or(SwitchError::NotConfigured)?;
        let Some(files) = self.files.clone().filter(|_| table.enabled) else {
            return Err(SwitchError::NotEnabled);
        };
        let _serial = self.serial.lock().await;
        let cleared =
            tokio::task::spawn_blocking(move || marker::switch_on(&files.sidecar, &files.cache))
                .await
                .unwrap_or_else(|_| Err("the write did not finish".to_owned()));
        if let Err(error) = cleared {
            tracing::warn!(
                %error,
                "the message cache could not be turned on: its switch beside the store cannot be \
                 saved"
            );
            return Err(SwitchError::Unsaved);
        }
        let was_off = self.off.swap(false, Ordering::SeqCst);
        match sink.cache() {
            Some(cache) if cache.is_open() => {}
            // Behind the sink but closed: a fresh file failed to open after an erase. Try again.
            Some(cache) => drop(self.erase_and_reopen(sink, cache).await),
            None => drop(self.open(sink).await),
        }
        if was_off {
            tracing::info!(
                opened = sink.cache().is_some_and(|cache| cache.is_open()),
                "the owner turned the message cache on"
            );
        }
        Ok(self.switched(sink, table).await)
    }

    /// The owner's "off". See the module documentation.
    ///
    /// # Errors
    ///
    /// [`SwitchError::NotConfigured`] without a table, and [`SwitchError::Unsaved`] when the
    /// marker could not be written, in which case nothing was erased.
    pub async fn switch_off(&self, sink: &Sink) -> Result<MessageCacheStatus, SwitchError> {
        let table = self.table.as_ref().ok_or(SwitchError::NotConfigured)?;
        let _serial = self.serial.lock().await;
        // With no store nothing can be open, and there is nowhere to keep a marker.
        if let Some(files) = self.files.clone() {
            let now_ms = self.clock.now_ms();
            let cache = sink.cache();
            let erased = tokio::task::spawn_blocking(move || -> Result<u64, String> {
                // Read first: a registry that cannot be read is a marker that cannot be written.
                let registry = marker::load(&files.sidecar)?;
                let held = cache.as_ref().map(|cache| cache.path().to_owned());
                let mut bytes = match &cache {
                    Some(cache) => {
                        marker::switch_off(cache, &files.sidecar, now_ms)
                            .map_err(|error| error.to_string())?
                            .bytes
                    }
                    None => marker::mark_off(&files.sidecar, now_ms).map(|_| 0)?,
                };
                // The marker is written: from here on nothing is reported as a failure.
                bytes += marker::erase_registered_except(
                    &registry,
                    files.default_dir.as_deref(),
                    held.as_deref(),
                );
                Ok(bytes)
            })
            .await
            .unwrap_or_else(|_| Err("the erase did not finish".to_owned()));
            match erased {
                Ok(bytes) => {
                    drop(sink.uninstall());
                    self.off.store(true, Ordering::SeqCst);
                    self.set_trouble(None);
                    tracing::info!(
                        bytes,
                        "the owner turned the message cache off; every message cache file this \
                         server made has been erased"
                    );
                }
                Err(error) => {
                    tracing::warn!(
                        %error,
                        "the message cache could not be turned off: its switch beside the store \
                         cannot be saved, so nothing was erased"
                    );
                    return Err(SwitchError::Unsaved);
                }
            }
        }
        Ok(self.switched(sink, table).await)
    }

    /// The owner's "erase now": erase the open file and open a fresh one. See the module
    /// documentation.
    ///
    /// # Errors
    ///
    /// [`SwitchError::NotConfigured`] without a table.
    pub async fn erase_now(&self, sink: &Sink) -> Result<MessageCacheStatus, SwitchError> {
        let table = self.table.as_ref().ok_or(SwitchError::NotConfigured)?;
        let _serial = self.serial.lock().await;
        if let Some(cache) = sink.cache() {
            if let Some(bytes) = self.erase_and_reopen(sink, cache).await {
                tracing::info!(
                    bytes,
                    "the owner erased the message cache; a fresh, empty file is open"
                );
            }
        }
        Ok(self.switched(sink, table).await)
    }

    /// The purge's part (`DELETE /api/v1/storage`): erase the open file, and open a fresh one in
    /// its place while the cache is on; and erase every other cache file this server made, such as
    /// one left from an earlier configuration. The owner's switch and the marker are not the
    /// store's, and are left exactly as they are: a purge never turns the cache back on, nor off.
    /// Returns the bytes erased.
    pub async fn purge(&self, sink: &Sink) -> u64 {
        let _serial = self.serial.lock().await;
        let mut bytes = 0;
        let cache = sink.cache();
        let held = cache.as_ref().map(|cache| cache.path().to_owned());
        if let Some(cache) = cache {
            bytes += self.erase_and_reopen(sink, cache).await.unwrap_or(0);
        }
        if let Some(files) = self.files.clone() {
            bytes += tokio::task::spawn_blocking(move || {
                // A registry that cannot be read names nothing; the store's directory is still
                // searched.
                let registry = marker::load(&files.sidecar).unwrap_or_default();
                marker::erase_registered_except(
                    &registry,
                    files.default_dir.as_deref(),
                    held.as_deref(),
                )
            })
            .await
            .unwrap_or(0);
        }
        self.forget_status();
        bytes
    }

    /// Erase the cache behind the sink and open a fresh file in its place, then check every
    /// provider's account against it again. The bytes erased, or `None` when the fresh file could
    /// not be opened: the old one is erased all the same, the server runs without a cache, and
    /// [`Lifecycle::trouble`] says why.
    async fn erase_and_reopen(&self, sink: &Sink, cache: Arc<MessageCache>) -> Option<u64> {
        let erased = tokio::task::spawn_blocking(move || cache.erase_file(true))
            .await
            .unwrap_or_else(|_| {
                Err(CacheError::Filesystem(
                    "the erase did not finish".to_owned(),
                ))
            });
        match erased {
            Ok(report) => {
                self.set_trouble(None);
                sink.reverify();
                Some(report.bytes)
            }
            Err(error) => {
                let trouble = Trouble::of(&error);
                tracing::warn!(
                    %error,
                    reason = trouble.as_str(),
                    "the message cache was erased and a fresh one cannot be opened; the server \
                     runs without one"
                );
                self.set_trouble(Some(trouble));
                None
            }
        }
    }

    /// Register the cache's path, open it, and put it behind `sink`. On failure the server runs
    /// without a cache, and [`Lifecycle::trouble`] says why.
    async fn open(&self, sink: &Sink) -> Result<(), Trouble> {
        let (Some(files), Some(table)) = (self.files.clone(), self.table.as_ref()) else {
            return Err(Trouble::SwitchUnwritable);
        };
        let options = CacheOptions {
            path: files.cache.clone(),
            store_path: Some(files.store.clone()),
            limits: Limits::from_config(table),
            search_text: Arc::clone(&self.search_text),
            clock: Arc::clone(&self.clock),
        };
        // Logged here rather than on the blocking pool, so the line comes from the task that
        // asked: every log line about the cache says what it says from one place.
        let opened = tokio::task::spawn_blocking(move || {
            marker::register(&files.sidecar, &files.cache)
                .map_err(|error| (Trouble::SwitchUnwritable, error))?;
            MessageCache::open(options).map_err(|error| (Trouble::of(&error), error.to_string()))
        })
        .await
        .unwrap_or_else(|_| Err((Trouble::Filesystem, "the open did not finish".to_owned())))
        .map_err(|(trouble, error)| {
            if trouble == Trouble::SwitchUnwritable {
                tracing::warn!(
                    %error,
                    "the message cache's switch beside the store cannot be written, so its file \
                     cannot be registered; no message cache is opened"
                );
            } else {
                tracing::warn!(
                    %error,
                    reason = trouble.as_str(),
                    "the message cache cannot be opened; the server runs without one"
                );
            }
            trouble
        });
        match opened {
            Ok(cache) => {
                self.set_trouble(None);
                sink.install(Arc::new(cache));
                Ok(())
            }
            Err(trouble) => {
                self.set_trouble(Some(trouble));
                Err(trouble)
            }
        }
    }
}

/// What [`start`] found and did, for the caller's tests and nothing else: the log says it too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Started {
    /// No store, so no sidecar and no default directory: nothing to reconcile.
    NoStore,
    /// No table, no sidecar and no cache file: a server that never had a cache.
    Untouched,
    /// Every cache file this server made was erased, and none opened.
    Erased {
        /// Bytes the erased files held.
        bytes: u64,
    },
    /// No table: nothing opened, nothing erased.
    Left {
        /// Bytes the files from an earlier configuration hold.
        bytes: u64,
    },
    /// The cache is open behind the sink.
    Opened,
    /// The cache should have opened and did not.
    Failed(Trouble),
}

/// Reconcile the configuration with the sidecar at startup, open the cache when both say so, and
/// attach what the server needs to switch it later to `sink`. See the module documentation.
///
/// Called once, after the server's summaries are attached to `sink` (so a cache put behind it is
/// followed by the task that keeps them in step) and before the sink is armed.
pub async fn start(config: &Config, sink: &Sink) -> Started {
    start_with(config, sink, Arc::new(SystemClock)).await
}

/// [`start`], reading time from `clock`: for a test that must place the cache's clock.
pub async fn start_with(config: &Config, sink: &Sink, clock: Arc<dyn Clock>) -> Started {
    let table = config.message_cache.clone();
    let files = config
        .storage
        .path
        .as_deref()
        .map(|store| Files::new(store, table.as_ref()));
    let lifecycle = Arc::new(Lifecycle {
        table: table.clone(),
        files: files.clone(),
        search_text: Arc::new(crate::render::SearchText::from_config(config)),
        clock: Arc::clone(&clock),
        off: AtomicBool::new(false),
        trouble: Mutex::new(None),
        serial: tokio::sync::Mutex::new(()),
        memo: Mutex::new(Memo::default()),
    });
    sink.attach_lifecycle(Arc::clone(&lifecycle));
    let Some(files) = files else {
        if let Some(table) = &table {
            tracing::info!(
                enabled = table.enabled,
                "[message_cache] is read; with no storage.path there is no message cache to open \
                 or erase"
            );
        }
        return Started::NoStore;
    };
    let now_ms = clock.now_ms();
    let reconciled = {
        let files = files.clone();
        let table = table.clone();
        tokio::task::spawn_blocking(move || {
            // A server that never had a cache, and whose configuration says nothing of one,
            // leaves no trace: not even a sidecar recording that the table is absent.
            let never_had_one = table.is_none()
                && std::fs::symlink_metadata(&files.sidecar).is_err()
                && marker::present_bytes(&marker::Sidecar::default(), files.default_dir.as_deref())
                    == 0;
            if never_had_one {
                return Ok(None);
            }
            marker::reconcile_at_start(
                table.as_ref(),
                &files.sidecar,
                files.default_dir.as_deref(),
                now_ms,
            )
            .map(Some)
        })
        .await
        .unwrap_or_else(|_| Err("the startup check did not finish".to_owned()))
    };
    let reconciled = match reconciled {
        Ok(Some(reconciled)) => reconciled,
        Ok(None) => return Started::Untouched,
        Err(error) => {
            tracing::warn!(
                %error,
                "the message cache's switch beside the store cannot be read or written; no \
                 message cache is opened"
            );
            lifecycle.set_trouble(Some(Trouble::SwitchUnreadable));
            return Started::Failed(Trouble::SwitchUnreadable);
        }
    };
    let off = reconciled.off;
    lifecycle.off.store(off, Ordering::SeqCst);
    match reconciled.action {
        StartupAction::EraseAll => {
            tracing::info!(
                bytes = reconciled.erased_bytes,
                configured = table.is_some(),
                switched_off = off,
                "the message cache is off; every message cache file this server made has been \
                 erased"
            );
            Started::Erased {
                bytes: reconciled.erased_bytes,
            }
        }
        StartupAction::Leave => {
            if reconciled.present_bytes > 0 {
                tracing::warn!(
                    bytes = reconciled.present_bytes,
                    "a message cache of this many bytes from an earlier configuration is present \
                     and unused; with no [message_cache] table it is neither opened nor erased, \
                     until the table has been absent for seven days"
                );
            }
            Started::Left {
                bytes: reconciled.present_bytes,
            }
        }
        StartupAction::Open => match lifecycle.open(sink).await {
            Ok(()) => {
                tracing::info!(
                    path = %files.cache.display(),
                    max_days = table.as_ref().map_or(0, |t| t.max_days),
                    max_megabytes = table.as_ref().map_or(0, |t| t.max_megabytes),
                    "the message cache is open; it is filled from what write-scope and \
                     server-owned reads see, and nothing is served from it yet"
                );
                Started::Opened
            }
            Err(trouble) => Started::Failed(trouble),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_open_failure_has_a_code_with_no_path_in_it() {
        for (error, code) in [
            (CacheError::Unavailable("/x".into()), "sqlite_unsupported"),
            (CacheError::Foreign("/x".into()), "foreign_file"),
            (CacheError::Locked("/x".into()), "locked"),
            (CacheError::Filesystem("/x".into()), "filesystem"),
            (CacheError::Sqlite("/x".into()), "sqlite"),
        ] {
            assert_eq!(Trouble::of(&error).as_str(), code);
            assert!(!code.contains('/'));
        }
    }
}
