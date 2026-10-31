//! Archive garbage collection at parity with
//! `py/wrkviz/archive_gc.py`: classify every file as live,
//! reclaimable, or held; reclaim only into a per-generation trash
//! with a receipt; empty the trash only as a separate pass.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::inspect::schema_3_completeness;
use crate::json::{dump_canonical, py_repr_str, read_json};
use crate::pipeline::{archive_marker_path, archive_writer_lock};
use crate::timeline_v3::SCHEMA_3_BOOTSTRAP_PATH;

const SCHEMA_2_BOOTSTRAP_PATH: &str = "data/timeline-v2.json";
const SCHEMA_2_PRESENTATION_ROOT: &str = "data/timeline-v2";
const SCHEMA_3_PRESENTATION_ROOT: &str = "data/timeline-v3";
use crate::transcript_io::write_text_if_changed;

/// Current trash directory name.
pub const TRASH_ROOT: &str = ".wrkviz-trash";
/// Pre-rename trash directory name.
pub const LEGACY_TRASH_ROOT: &str = ".agent-team-timeline-trash";

const RECEIPT_NAME: &str = "receipt.json";
const TRASH_PAYLOAD: &str = "files";
const SOURCE_SNAPSHOT_DIRECTORY: &str = "source_snapshots";
const SCHEMA_1_TIMELINE: &str = "data/timeline.json";
const SCHEMA_2_OBJECT_ROOT: &str = "data/timeline-v2/objects";
const SCHEMA_2_MANIFEST: &str = "data/timeline-v2/manifest.json";
const SCHEMA_2_CAPABILITY: &str = "timeline_shards.SCHEMA_2_IS_PUBLISHED";
const WEBSITE_APP: &str = "app.js";

/// One classified file.
#[derive(Clone, Debug)]
pub struct GcFile {
    /// Archive-relative path.
    pub relative_path: String,
    /// Byte size.
    pub bytes: i64,
}

impl GcFile {
    fn to_json_obj(&self) -> Value {
        let mut out = serde_json::Map::new();
        out.insert("path".into(), Value::String(self.relative_path.clone()));
        out.insert("bytes".into(), self.bytes.into());
        Value::Object(out)
    }
}

/// One classification category (always carries its reason).
#[derive(Clone, Debug)]
pub struct GcCategory {
    /// Category name.
    pub name: String,
    /// Whether its files may be swept.
    pub reclaimable: bool,
    /// Human reason.
    pub reason: String,
    /// Classified files.
    pub files: Vec<GcFile>,
}

impl GcCategory {
    fn bytes(&self) -> i64 {
        self.files.iter().map(|item| item.bytes).sum()
    }

    fn count(&self) -> i64 {
        self.files.len() as i64
    }

    fn to_json_obj(&self, include_files: bool) -> Value {
        let mut out = serde_json::Map::new();
        out.insert("category".into(), Value::String(self.name.clone()));
        out.insert("reclaimable".into(), Value::Bool(self.reclaimable));
        out.insert("reason".into(), Value::String(self.reason.clone()));
        out.insert("files".into(), self.count().into());
        out.insert("bytes".into(), self.bytes().into());
        if include_files {
            out.insert(
                "paths".into(),
                Value::Array(self.files.iter().map(GcFile::to_json_obj).collect()),
            );
        }
        Value::Object(out)
    }
}

/// One gc invocation's report.
#[derive(Clone, Debug)]
pub struct GcReport {
    /// Resolved archive root.
    pub root: String,
    /// dry-run | swept | emptied.
    pub action: String,
    /// Total bytes.
    pub total_bytes: i64,
    /// Total files.
    pub total_files: i64,
    /// Categories in report order.
    pub categories: Vec<GcCategory>,
    /// Trash directory name.
    pub trash_relative_path: String,
    /// Trash generation (swept only).
    pub trash_generation: Option<String>,
    /// Files moved (swept only).
    pub moved: Vec<GcFile>,
    /// Directories removed (or planned, for dry-run).
    pub removed_directories: Vec<String>,
    /// Bytes freed by empty-trash.
    pub emptied_bytes: i64,
}

impl GcReport {
    fn reclaimable_bytes(&self) -> i64 {
        self.categories
            .iter()
            .filter(|item| item.reclaimable)
            .map(GcCategory::bytes)
            .sum()
    }

    fn reclaimable_files(&self) -> i64 {
        self.categories
            .iter()
            .filter(|item| item.reclaimable)
            .map(GcCategory::count)
            .sum()
    }

    fn held_bytes(&self) -> i64 {
        self.categories
            .iter()
            .filter(|item| !item.reclaimable)
            .map(GcCategory::bytes)
            .sum()
    }

    fn category(&self, name: &str) -> &GcCategory {
        self.categories
            .iter()
            .find(|item| item.name == name)
            .expect("category always exists")
    }

    /// The JSON report form.
    pub fn to_json_obj(&self, include_files: bool) -> Value {
        let mut out = serde_json::Map::new();
        out.insert("schema_version".into(), 1.into());
        out.insert("kind".into(), Value::String("archive-gc-report".into()));
        out.insert("archive".into(), Value::String(self.root.clone()));
        out.insert("action".into(), Value::String(self.action.clone()));
        out.insert("archive_bytes".into(), self.total_bytes.into());
        out.insert("archive_files".into(), self.total_files.into());
        out.insert("reclaimable_bytes".into(), self.reclaimable_bytes().into());
        out.insert("reclaimable_files".into(), self.reclaimable_files().into());
        out.insert("held_bytes".into(), self.held_bytes().into());
        out.insert(
            "trash".into(),
            Value::String(self.trash_relative_path.clone()),
        );
        out.insert(
            "trash_generation".into(),
            self.trash_generation
                .clone()
                .map_or(Value::Null, Value::String),
        );
        out.insert(
            "moved".into(),
            Value::Array(self.moved.iter().map(GcFile::to_json_obj).collect()),
        );
        out.insert(
            "removed_directories".into(),
            Value::Array(
                self.removed_directories
                    .iter()
                    .map(|item| Value::String(item.clone()))
                    .collect(),
            ),
        );
        out.insert("emptied_bytes".into(), self.emptied_bytes.into());
        out.insert(
            "categories".into(),
            Value::Array(
                self.categories
                    .iter()
                    .map(|item| item.to_json_obj(include_files))
                    .collect(),
            ),
        );
        Value::Object(out)
    }
}

fn is_symlink(path: &Path) -> bool {
    path.symlink_metadata()
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
}

fn archive_path(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let parts: Vec<&str> = relative.split('/').collect();
    if relative.starts_with('/')
        || parts.is_empty()
        || parts.iter().any(|part| matches!(*part, "" | "." | ".."))
    {
        return Err(format!("unsafe archive path: {}", py_repr_str(relative)));
    }
    let resolved_root = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut cursor = root.to_path_buf();
    for part in &parts[..parts.len() - 1] {
        cursor.push(part);
        if is_symlink(&cursor) {
            return Err(format!(
                "archive path crosses a symlink: {}",
                py_repr_str(relative)
            ));
        }
    }
    let candidate = root.join(relative);
    if is_symlink(&candidate) {
        return Err(format!(
            "refusing to collect through a symlink: {}",
            py_repr_str(relative)
        ));
    }
    if let Some(parent) = candidate.parent() {
        if parent.exists() {
            let parent_real =
                std::fs::canonicalize(parent).unwrap_or_else(|_| parent.to_path_buf());
            if !parent_real.starts_with(&resolved_root) {
                return Err(format!(
                    "archive path escapes the archive: {}",
                    py_repr_str(relative)
                ));
            }
        }
    }
    Ok(candidate)
}

fn size_of(root: &Path, relative: &str) -> Option<i64> {
    let path = root.join(relative);
    let meta = path.symlink_metadata().ok()?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return None;
    }
    Some(meta.len() as i64)
}

fn walk_files(root: &Path, relative_root: &str) -> Vec<(String, i64)> {
    let start = if relative_root.is_empty() {
        root.to_path_buf()
    } else {
        root.join(relative_root)
    };
    let mut out = Vec::new();
    if !start.is_dir() || is_symlink(&start) {
        return out;
    }
    // os.walk is top-down: a directory's files precede deeper files.
    fn walk_top(dir: &Path, root: &Path, out: &mut Vec<(String, i64)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut dirs: Vec<PathBuf> = Vec::new();
        let mut names: Vec<PathBuf> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if is_symlink(&path) {
                continue;
            }
            if path.is_dir() {
                dirs.push(path);
            } else {
                names.push(path);
            }
        }
        dirs.sort();
        names.sort();
        for file_path in names {
            let Ok(meta) = file_path.symlink_metadata() else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            if let Ok(relative) = file_path.strip_prefix(root) {
                out.push((
                    relative.display().to_string().replace('\\', "/"),
                    meta.len() as i64,
                ));
            }
        }
        for dir_path in dirs {
            walk_top(&dir_path, root, out);
        }
    }
    walk_top(&start, root, &mut out);
    out
}

fn files_with_sizes(root: &Path, relatives: &[String]) -> Vec<GcFile> {
    let unique: BTreeSet<&String> = relatives.iter().collect();
    unique
        .into_iter()
        .filter_map(|relative| {
            size_of(root, relative).map(|bytes| GcFile {
                relative_path: relative.clone(),
                bytes,
            })
        })
        .collect()
}

fn bootstrap(
    root: &Path,
    relative: &str,
    version: i64,
    kind: &str,
) -> Option<serde_json::Map<String, Value>> {
    let path = root.join(relative);
    if is_symlink(&path) || !path.is_file() {
        return None;
    }
    let document = read_json(&path).ok()?;
    let obj = document.as_object()?.clone();
    if obj.get("schema_version").and_then(Value::as_i64) != Some(version)
        || obj.get("kind").and_then(Value::as_str) != Some(kind)
    {
        return None;
    }
    Some(obj)
}

fn website_refusal(root: &Path) -> Option<String> {
    let path = root.join(WEBSITE_APP);
    if is_symlink(&path) || !path.is_file() {
        return None;
    }
    let source = String::from_utf8_lossy(&std::fs::read(&path).ok()?).into_owned();
    if source.contains(SCHEMA_3_BOOTSTRAP_PATH) {
        return None;
    }
    Some(format!(
        "held: this archive's own {WEBSITE_APP} has no schema-3 mode -- it does not mention {SCHEMA_3_BOOTSTRAP_PATH} -- so it still loads {SCHEMA_2_BOOTSTRAP_PATH} as its timeline and falls back to {SCHEMA_1_TIMELINE}, and this pass offers both. A build copies {WEBSITE_APP} into the archive and `gc` does not rewrite it, so a newer tool is not enough on its own: re-run the build against this archive, which costs no tokens and republishes the bundle, and the generation becomes collectable"
    ))
}

fn cross_generation_refusal(root: &Path) -> Option<String> {
    let schema_3 = bootstrap(root, SCHEMA_3_BOOTSTRAP_PATH, 3, "timeline-v3-bootstrap")?;
    let schema_2 = bootstrap(root, SCHEMA_2_BOOTSTRAP_PATH, 2, "timeline-bootstrap")?;
    let theirs = schema_3.get("source_digest")?;
    let ours = schema_2.get("source_digest")?;
    if theirs == ours {
        return None;
    }
    Some(format!(
        "held: {SCHEMA_3_BOOTSTRAP_PATH} and {SCHEMA_2_BOOTSTRAP_PATH} describe different builds -- source_digest {} against {} -- so the schema-3 generation is not a successor to this schema 2, it is a bystander from another one. Publication is schema 2 and then schema 3, so a build that died between them leaves exactly this: a complete, self-consistent schema 3 that is a build behind, and reclaiming on its strength would take the newer generation and keep the older. The reader refuses the same mismatch when a transcript search makes it look at both. Re-run the build; it republishes schema 3 against this source and the two agree again",
        py_repr_value(theirs),
        py_repr_value(ours),
    ))
}

fn py_repr_value(value: &Value) -> String {
    match value {
        Value::String(text) => py_repr_str(text),
        other => crate::json::dump_compact(other),
    }
}

fn schema_1_category(root: &Path) -> GcCategory {
    let relatives = vec![
        SCHEMA_1_TIMELINE.to_string(),
        format!("{SCHEMA_1_TIMELINE}.gz"),
    ];
    let files = files_with_sizes(root, &relatives);
    let (complete, declined) = schema_3_completeness(root);
    if !complete {
        return GcCategory {
            name: "schema-1-monolith".into(),
            reclaimable: false,
            reason: format!("held: no complete schema-3 generation supersedes it ({declined})"),
            files,
        };
    }
    if website_refusal(root).is_some() && size_of(root, SCHEMA_2_BOOTSTRAP_PATH).is_none() {
        return GcCategory {
            name: "schema-1-monolith".into(),
            reclaimable: false,
            reason: format!(
                "held: this archive's own {WEBSITE_APP} has no schema-3 mode, so it reads schema 2 and falls back to schema 1, and {SCHEMA_2_BOOTSTRAP_PATH} is absent. Re-run the build against this archive, which republishes the bundle"
            ),
            files,
        };
    }
    let beside = if size_of(root, SCHEMA_2_BOOTSTRAP_PATH).is_some() {
        format!(" beside {SCHEMA_2_BOOTSTRAP_PATH}")
    } else {
        String::new()
    };
    GcCategory {
        name: "schema-1-monolith".into(),
        reclaimable: true,
        reason: format!(
            "superseded by a complete {SCHEMA_3_BOOTSTRAP_PATH}{beside}; no build writes it any more"
        ),
        files,
    }
}

fn schema_2_manifest_sets(root: &Path) -> Option<(BTreeSet<String>, BTreeSet<String>)> {
    let path = root.join(SCHEMA_2_MANIFEST);
    if is_symlink(&path) || !path.is_file() {
        return None;
    }
    let document = read_json(&path).ok()?;
    let manifest = document.as_object()?;
    if manifest.get("schema_version").and_then(Value::as_i64) != Some(1)
        || manifest.get("kind").and_then(Value::as_str) != Some("timeline-shard-files")
    {
        return None;
    }
    let read_set = |key: &str| -> Option<BTreeSet<String>> {
        let items = manifest.get(key)?.as_array()?;
        let mut out = BTreeSet::new();
        for item in items {
            out.insert(item.as_str()?.to_string());
        }
        Some(out)
    };
    Some((read_set("current_objects")?, read_set("retained_objects")?))
}

fn object_digest_shape(name: &str) -> bool {
    // [0-9a-f]{64}\.json(?:\.gz)?
    let stem = name.strip_suffix(".gz").unwrap_or(name);
    let Some(digest) = stem.strip_suffix(".json") else {
        return false;
    };
    digest.len() == 64
        && digest
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
}

fn digest_shape(value: &str) -> bool {
    value.len() == 64
        && value
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() && !ch.is_ascii_uppercase())
}

fn schema_2_search_paths(root: &Path) -> Option<BTreeSet<String>> {
    let path = root.join(SCHEMA_2_BOOTSTRAP_PATH);
    if is_symlink(&path) || !path.is_file() {
        return Some(BTreeSet::new());
    }
    let document = bootstrap(root, SCHEMA_2_BOOTSTRAP_PATH, 2, "timeline-bootstrap")?;
    let Some(raw_search) = document.get("search") else {
        return Some(BTreeSet::new());
    };
    let mut named: BTreeSet<String> = [
        SCHEMA_2_BOOTSTRAP_PATH.to_string(),
        format!("{SCHEMA_2_BOOTSTRAP_PATH}.gz"),
    ]
    .into_iter()
    .collect();
    let search = raw_search.as_object()?;
    let shards = search.get("shards")?.as_array()?;
    for raw_shard in shards {
        let shard = raw_shard.as_object()?;
        let mut references = vec![shard];
        if let Some(linkage) = shard.get("linkage") {
            references.push(linkage.as_object()?);
        }
        for reference in references {
            let digest = reference.get("sha256")?.as_str()?;
            if !digest_shape(digest) {
                return None;
            }
            named.insert(format!("{SCHEMA_2_OBJECT_ROOT}/{digest}.json"));
            named.insert(format!("{SCHEMA_2_OBJECT_ROOT}/{digest}.json.gz"));
        }
    }
    Some(named)
}

fn schema_2_generation_files(root: &Path) -> Vec<GcFile> {
    let mut relatives = vec![
        SCHEMA_2_BOOTSTRAP_PATH.to_string(),
        format!("{SCHEMA_2_BOOTSTRAP_PATH}.gz"),
    ];
    relatives.extend(
        walk_files(root, SCHEMA_2_PRESENTATION_ROOT)
            .into_iter()
            .map(|(relative, _)| relative),
    );
    files_with_sizes(root, &relatives)
}

fn schema_2_retirement_refusal(root: &Path) -> Option<String> {
    // schema_2_is_published() is False in this build.
    let (complete, declined) = schema_3_completeness(root);
    if !complete {
        return Some(format!(
            "held: no complete schema-3 generation supersedes it ({declined})"
        ));
    }
    if let Some(mismatch) = cross_generation_refusal(root) {
        return Some(mismatch);
    }
    website_refusal(root)
}

fn holds_raw_turns(root: &Path) -> bool {
    let stores = crate::build_store::candidate_store_roots(root).unwrap_or_default();
    for store in stores {
        if !store.is_dir() || is_symlink(&store) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&store) else {
            continue;
        };
        for entry in entries.flatten() {
            let raw = entry.path().join("raw");
            if raw.is_dir() && !is_symlink(&raw) {
                return true;
            }
        }
    }
    false
}

fn schema_2_reclaim_reason(root: &Path) -> String {
    let monolith_gone = size_of(root, SCHEMA_1_TIMELINE).is_none();
    let chain = if monolith_gone {
        format!(
            "{SCHEMA_1_TIMELINE} is already gone, so schema 3 becomes the only generation this archive has"
        )
    } else {
        format!(
            "{SCHEMA_1_TIMELINE} is still on disk, but the schema-1-monolith category above offers it under conditions these already satisfy, so one --delete takes both and schema 3 becomes the only generation this archive has"
        )
    };
    let rebuild = if holds_raw_turns(root) {
        "the way back is a rebuild, which costs no tokens and reads the raw turns already reachable from here -- in <output>.build/*/raw, or teams/*/raw on an archive that has not migrated".to_string()
    } else {
        "the way back is a rebuild, and it cannot be run here: this archive has no raw turns in either place, which is what a combined export looks like -- they are in the ingest archive it was built from, wherever that is".to_string()
    };
    format!(
        "superseded by a complete {SCHEMA_3_BOOTSTRAP_PATH} beside {SCHEMA_2_BOOTSTRAP_PATH}; no build writes it any more ({SCHEMA_2_CAPABILITY} is False, and write_timeline_shards refuses to run while it is). Whether this includes the transcript search corpus that shares the tree depends on whether the schema 3 here publishes one; schema-2-search-corpus says which, and subtracts itself from this category when it does not. WARNING, read before --delete: this removes the last fallback. {chain}. A schema-3 generation the reader declines then leaves the archive unreadable rather than slow, and it declines the whole generation over a single shard-shaped file under {SCHEMA_3_PRESENTATION_ROOT}/ that the catalogue does not name -- exactly what an interrupted build leaves behind. Then {rebuild}; the way back from a mistake in the next ten minutes is {TRASH_ROOT}/, until `gc --empty-trash`"
    )
}

fn schema_3_search_corpus(root: &Path) -> Option<(i64, i64)> {
    let document = bootstrap(root, SCHEMA_3_BOOTSTRAP_PATH, 3, "timeline-v3-bootstrap")?;
    let streams = document.get("streams")?.as_object()?;
    let section = streams.get("search")?.as_object()?;
    let shards = section.get("shards")?.as_array()?;
    if shards.is_empty() {
        return None;
    }
    let teams: BTreeSet<String> = shards
        .iter()
        .filter_map(|raw| {
            raw.as_object()
                .and_then(|obj| obj.get("team"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    Some((shards.len() as i64, teams.len() as i64))
}

fn schema_2_search_corpus_category(
    root: &Path,
    named: Option<&BTreeSet<String>>,
    unknowable: &str,
    successor: Option<(i64, i64)>,
) -> GcCategory {
    let Some(named) = named else {
        return GcCategory {
            name: "schema-2-search-corpus".into(),
            reclaimable: false,
            reason: unknowable.to_string(),
            files: Vec::new(),
        };
    };
    if named.is_empty() {
        return GcCategory {
            name: "schema-2-search-corpus".into(),
            reclaimable: false,
            reason: format!(
                "nothing to hold: {SCHEMA_2_BOOTSTRAP_PATH} names no transcript search corpus, so there is nothing here that schema 3 has not superseded"
            ),
            files: Vec::new(),
        };
    }
    if let Some((shards, teams)) = successor {
        return GcCategory {
            name: "schema-2-search-corpus".into(),
            reclaimable: false,
            reason: format!(
                "nothing to hold: {SCHEMA_3_BOOTSTRAP_PATH} publishes a transcript search corpus of its own -- {shards} search shard(s) across {teams} team(s), with the trigram prefilter and the prompt/response sidecar beside them -- and query.TimelineQuery._iter_search_records reads it, opening {SCHEMA_2_BOOTSTRAP_PATH} only for an archive that has no such streams. These objects are therefore a duplicate like the rest of the generation and are accounted for by superseded-schema-2, under whatever condition that category is subject to"
            ),
            files: Vec::new(),
        };
    }
    let relatives: Vec<String> = named.iter().cloned().collect();
    let files = files_with_sizes(root, &relatives);
    let objects = files
        .iter()
        .filter(|item| {
            item.relative_path
                .starts_with(&format!("{SCHEMA_2_OBJECT_ROOT}/"))
        })
        .count();
    GcCategory {
        name: "schema-2-search-corpus".into(),
        reclaimable: false,
        reason: format!(
            "held: not superseded in this archive. {SCHEMA_2_BOOTSTRAP_PATH} and the {objects} content-addressed day shards and linkage sidecars its `search` section names under {SCHEMA_2_OBJECT_ROOT}/ are the transcript search corpus, and the {SCHEMA_3_BOOTSTRAP_PATH} here publishes no `search` stream to replace them -- so query.TimelineQuery._iter_search_records reads this generation even when schema 3 answers everything else, which is what `timeline search` and `timeline show` run on. Reclaiming these would remove a capability rather than a duplicate. A rebuild against this archive publishes the schema-3 corpus and costs no tokens; then they are offered with the rest of the generation"
        ),
        files,
    }
}

fn schema_2_categories(root: &Path) -> (GcCategory, GcCategory, GcCategory, GcCategory) {
    let on_disk: HashMap<String, i64> = walk_files(root, SCHEMA_2_OBJECT_ROOT)
        .into_iter()
        .filter(|(relative, _)| relative.rsplit('/').next().is_some_and(object_digest_shape))
        .collect();
    let mut generation = schema_2_generation_files(root);
    let search_paths = schema_2_search_paths(root);
    let unknowable = format!(
        "held: {SCHEMA_2_BOOTSTRAP_PATH} is present and cannot be parsed, so the objects the transcript search corpus needs cannot be told apart from the presentation objects schema 3 supersedes -- they share one content-addressed directory and only the catalogue distinguishes them. Nothing in this generation is offered until it reads"
    );
    let mut refusal = schema_2_retirement_refusal(root);
    if refusal.is_none() && search_paths.is_none() {
        refusal = Some(unknowable.clone());
    }
    let search = schema_2_search_corpus_category(
        root,
        search_paths.as_ref(),
        &unknowable,
        schema_3_search_corpus(root),
    );
    let held_by_search: BTreeSet<&str> = search
        .files
        .iter()
        .map(|item| item.relative_path.as_str())
        .collect();
    generation.retain(|item| !held_by_search.contains(item.relative_path.as_str()));
    let on_disk: HashMap<String, i64> = on_disk
        .into_iter()
        .filter(|(relative, _)| !held_by_search.contains(relative.as_str()))
        .collect();
    if refusal.is_none() && generation.is_empty() {
        refusal = Some(if !held_by_search.is_empty() {
            format!(
                "nothing to collect: the presentation half of this generation is gone and what remains under {SCHEMA_2_PRESENTATION_ROOT}/ is the transcript search corpus, held above"
            )
        } else {
            format!(
                "nothing to collect: this archive has no {SCHEMA_2_BOOTSTRAP_PATH} and no {SCHEMA_2_PRESENTATION_ROOT}/ tree"
            )
        });
    }
    if refusal.is_none() {
        let subsumed = "subsumed by superseded-schema-2: the presentation half of the generation is being reclaimed in this pass, so which of its objects the manifest still names decides nothing".to_string();
        return (
            GcCategory {
                name: "superseded-schema-2".into(),
                reclaimable: true,
                reason: schema_2_reclaim_reason(root),
                files: generation,
            },
            search,
            GcCategory {
                name: "schema-2-orphan-objects".into(),
                reclaimable: false,
                reason: subsumed,
                files: Vec::new(),
            },
            GcCategory {
                name: "schema-2-retained-objects".into(),
                reclaimable: false,
                reason: "subsumed by superseded-schema-2: the grace period keeps these fetchable for a browser that already loaded the previous bootstrap, and no browser reads this format any more -- which is the precondition the whole generation went on".into(),
                files: Vec::new(),
            },
        );
    }
    let refusal = refusal.unwrap();
    let (orphans, retained_category) = match schema_2_manifest_sets(root) {
        None => (
            GcCategory {
                name: "schema-2-orphan-objects".into(),
                reclaimable: false,
                reason: format!(
                    "held: {SCHEMA_2_MANIFEST} is absent or unrecognised, so nothing on disk can be shown to be unreachable"
                ),
                files: Vec::new(),
            },
            GcCategory {
                name: "schema-2-retained-objects".into(),
                reclaimable: false,
                reason: format!("held: {SCHEMA_2_MANIFEST} is absent or unrecognised"),
                files: Vec::new(),
            },
        ),
        Some((current, retained)) => {
            let orphan_files: Vec<GcFile> = {
                let mut keys: Vec<&String> = on_disk
                    .keys()
                    .filter(|relative| {
                        !current.contains(*relative) && !retained.contains(*relative)
                    })
                    .collect();
                keys.sort();
                keys.into_iter()
                    .map(|relative| GcFile {
                        relative_path: relative.clone(),
                        bytes: on_disk[relative],
                    })
                    .collect()
            };
            let retained_files: Vec<GcFile> = {
                let mut keys: Vec<&String> = retained
                    .iter()
                    .filter(|relative| on_disk.contains_key(*relative))
                    .collect();
                keys.sort();
                keys.into_iter()
                    .map(|relative| GcFile {
                        relative_path: relative.clone(),
                        bytes: on_disk[relative],
                    })
                    .collect()
            };
            (
                GcCategory {
                    name: "schema-2-orphan-objects".into(),
                    reclaimable: true,
                    reason: format!(
                        "named by neither current_objects nor retained_objects in {SCHEMA_2_MANIFEST}"
                    ),
                    files: orphan_files,
                },
                GcCategory {
                    name: "schema-2-retained-objects".into(),
                    reclaimable: false,
                    reason: "held: the previous generation's objects, kept alive one more generation so a browser that already loaded the previous bootstrap can still fetch what it names; the next build that supersedes them reclaims them".into(),
                    files: retained_files,
                },
            )
        }
    };
    let claimed: BTreeSet<&str> = orphans
        .files
        .iter()
        .chain(retained_category.files.iter())
        .map(|item| item.relative_path.as_str())
        .collect();
    (
        GcCategory {
            name: "superseded-schema-2".into(),
            reclaimable: false,
            reason: refusal,
            files: generation
                .into_iter()
                .filter(|item| !claimed.contains(item.relative_path.as_str()))
                .collect(),
        },
        search,
        orphans,
        retained_category,
    )
}

fn schema_3_named_paths(root: &Path) -> Option<BTreeSet<String>> {
    let document = bootstrap(root, SCHEMA_3_BOOTSTRAP_PATH, 3, "timeline-v3-bootstrap")?;
    let streams = document.get("streams")?.as_object()?;
    let mut named: BTreeSet<String> = [SCHEMA_3_BOOTSTRAP_PATH.to_string()].into_iter().collect();
    let mut stream_names: Vec<&String> = streams.keys().collect();
    stream_names.sort();
    for stream in stream_names {
        let section = streams.get(stream)?.as_object()?;
        let shards = section.get("shards")?.as_array()?;
        for raw in shards {
            let entry = raw.as_object()?;
            named.insert(entry.get("path")?.as_str()?.to_string());
            named.insert(entry.get("index_path")?.as_str()?.to_string());
        }
    }
    Some(named)
}

fn schema_3_category(root: &Path) -> GcCategory {
    let named = schema_3_named_paths(root);
    let on_disk = walk_files(root, SCHEMA_3_PRESENTATION_ROOT);
    let Some(named) = named else {
        return GcCategory {
            name: "schema-3-orphan-shards".into(),
            reclaimable: false,
            reason: format!(
                "held: {SCHEMA_3_BOOTSTRAP_PATH} is absent or unreadable, so no shard can be shown to be unreachable"
            ),
            files: Vec::new(),
        };
    };
    let orphans: Vec<GcFile> = on_disk
        .iter()
        .filter(|(relative, _)| !named.contains(relative))
        .map(|(relative, size)| GcFile {
            relative_path: relative.clone(),
            bytes: *size,
        })
        .collect();
    if orphans.is_empty() {
        let (complete, declined) = schema_3_completeness(root);
        return GcCategory {
            name: "schema-3-orphan-shards".into(),
            reclaimable: false,
            reason: format!(
                "nothing under {SCHEMA_3_PRESENTATION_ROOT}/ is unaccounted for; the tree matches {SCHEMA_3_BOOTSTRAP_PATH}{}",
                if complete {
                    String::new()
                } else {
                    format!(", though the generation is incomplete ({declined})")
                }
            ),
            files: Vec::new(),
        };
    }
    GcCategory {
        name: "schema-3-orphan-shards".into(),
        reclaimable: false,
        reason: format!(
            "held: under {SCHEMA_3_PRESENTATION_ROOT}/ and named by no entry in {SCHEMA_3_BOOTSTRAP_PATH}. A completed build leaves this set empty -- it removes its own strays -- so these are the residue of a build that was interrupted before it published its bootstrap, and there is no way from here to tell a retired team's shard from a live team's. Until they are gone the reader declines schema 3 entirely and this archive is being read through the slower generation behind it. Re-run the build; it will clear them and this category goes back to empty"
        ),
        files: orphans,
    }
}

fn source_snapshot_category(root: &Path) -> GcCategory {
    let teams = root.join("teams");
    let mut relatives: Vec<String> = Vec::new();
    if teams.is_dir() && !is_symlink(&teams) {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&teams)
            .map(|read| read.flatten().map(|entry| entry.path()).collect())
            .unwrap_or_default();
        entries.sort();
        for team in entries {
            if team.is_dir() && !is_symlink(&team) {
                if let Some(name) = team.file_name().and_then(|item| item.to_str()) {
                    relatives.push(format!("teams/{name}/{SOURCE_SNAPSHOT_DIRECTORY}"));
                }
            }
        }
    }
    let mut found: Vec<GcFile> = Vec::new();
    for relative in &relatives {
        found.extend(
            walk_files(root, relative)
                .into_iter()
                .map(|(path, size)| GcFile {
                    relative_path: path,
                    bytes: size,
                }),
        );
    }
    GcCategory {
        name: "ingest-source-snapshots".into(),
        reclaimable: false,
        reason: "held: vendor input rather than published output, and its deletion has a purpose-built gate -- run `audit-losslessness --require-lossless` and act on that, not on this. To get these bytes out of the published tree without deleting anything, run `migrate-snapshots`".into(),
        files: found,
    }
}

fn trash_category(root: &Path) -> GcCategory {
    let mut held: Vec<GcFile> = Vec::new();
    for trash_root in [TRASH_ROOT, LEGACY_TRASH_ROOT] {
        held.extend(
            walk_files(root, trash_root)
                .into_iter()
                .map(|(path, size)| GcFile {
                    relative_path: path,
                    bytes: size,
                }),
        );
    }
    GcCategory {
        name: "trash".into(),
        reclaimable: false,
        reason: format!(
            "held: already swept and reversible; `gc --empty-trash` deletes {TRASH_ROOT}/ for good"
        ),
        files: held,
    }
}

fn empty_directories(root: &Path, pending: &BTreeSet<String>) -> Vec<String> {
    let data = root.join("data");
    if !data.is_dir() || is_symlink(&data) {
        return Vec::new();
    }
    let mut collected: BTreeSet<String> = BTreeSet::new();
    let mut empty: Vec<String> = Vec::new();
    fn visit(
        dir: &Path,
        root: &Path,
        data: &Path,
        pending: &BTreeSet<String>,
        collected: &mut BTreeSet<String>,
        empty: &mut Vec<String>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut subdirs: Vec<PathBuf> = Vec::new();
        let mut names: Vec<String> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && !is_symlink(&path) {
                subdirs.push(path);
            } else {
                names.push(
                    path.strip_prefix(root)
                        .map(|item| item.display().to_string().replace('\\', "/"))
                        .unwrap_or_default(),
                );
            }
        }
        subdirs.sort();
        for sub in &subdirs {
            visit(sub, root, data, pending, collected, empty);
        }
        if dir == data || is_symlink(dir) {
            return;
        }
        if names.iter().any(|name| !pending.contains(name)) {
            return;
        }
        let sub_paths: Vec<String> = subdirs
            .iter()
            .map(|item| item.display().to_string().replace('\\', "/"))
            .collect();
        if sub_paths.iter().any(|item| !collected.contains(item)) {
            return;
        }
        collected.insert(dir.display().to_string().replace('\\', "/"));
        empty.push(
            dir.strip_prefix(root)
                .map(|item| item.display().to_string().replace('\\', "/"))
                .unwrap_or_default(),
        );
    }
    visit(&data, root, &data, pending, &mut collected, &mut empty);
    empty
}

/// Classify the archive without touching it.
pub fn plan_collection(root: &Path) -> GcReport {
    let (mut total_bytes, mut total_files) = (0i64, 0i64);
    for (_, size) in walk_files(root, "") {
        total_bytes += size;
        total_files += 1;
    }
    let (superseded_schema_2, search_corpus, orphan_objects, retained_objects) =
        schema_2_categories(root);
    let categories = vec![
        schema_1_category(root),
        superseded_schema_2,
        search_corpus,
        orphan_objects,
        schema_3_category(root),
        retained_objects,
        source_snapshot_category(root),
        trash_category(root),
    ];
    let pending: BTreeSet<String> = categories
        .iter()
        .filter(|category| category.reclaimable)
        .flat_map(|category| category.files.iter().map(|item| item.relative_path.clone()))
        .collect();
    GcReport {
        root: std::fs::canonicalize(root)
            .unwrap_or_else(|_| root.to_path_buf())
            .display()
            .to_string(),
        action: "dry-run".into(),
        total_bytes,
        total_files,
        removed_directories: empty_directories(root, &pending),
        categories,
        trash_relative_path: TRASH_ROOT.into(),
        trash_generation: None,
        moved: Vec::new(),
        emptied_bytes: 0,
    }
}

fn generation_directory(root: &Path, stamp: &str) -> Result<String, String> {
    for attempt in 1..1000 {
        let candidate = if attempt == 1 {
            stamp.to_string()
        } else {
            format!("{stamp}-{attempt}")
        };
        let relative = format!("{TRASH_ROOT}/{candidate}");
        if !root.join(&relative).exists() {
            return Ok(relative);
        }
    }
    Err(format!(
        "cannot open a trash generation under {TRASH_ROOT}/"
    ))
}

fn move_to_trash(root: &Path, generation: &str, relative: &str) -> Result<(), String> {
    let source = archive_path(root, relative)?;
    let target = archive_path(root, &format!("{generation}/{TRASH_PAYLOAD}/{relative}"))?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|error| crate::json::os_error(&error, parent))?;
    }
    std::fs::rename(&source, &target).map_err(|error| {
        #[cfg(unix)]
        {
            if error.raw_os_error() == Some(libc::EXDEV) {
                return format!(
                    "cannot move {relative} into {TRASH_ROOT}/ because they are on different filesystems; the trash has to sit on the archive's own device for the first pass to be both fast and reversible"
                );
            }
        }
        format!("cannot reclaim {relative}: {error}")
    })
}

fn receipt(
    root: &Path,
    generation: &str,
    report: &GcReport,
    files: &[(String, GcFile)],
    complete: bool,
) -> Result<(), String> {
    let entries: Vec<Value> = files
        .iter()
        .map(|(category, item)| {
            let mut entry = serde_json::Map::new();
            entry.insert("path".into(), Value::String(item.relative_path.clone()));
            entry.insert("bytes".into(), item.bytes.into());
            entry.insert("category".into(), Value::String(category.clone()));
            entry.insert(
                "restore_to".into(),
                Value::String(item.relative_path.clone()),
            );
            Value::Object(entry)
        })
        .collect();
    let reasons: Vec<Value> = report
        .categories
        .iter()
        .filter(|item| item.reclaimable)
        .map(|item| {
            let mut entry = serde_json::Map::new();
            entry.insert("category".into(), Value::String(item.name.clone()));
            entry.insert("reason".into(), Value::String(item.reason.clone()));
            Value::Object(entry)
        })
        .collect();
    let mut document = serde_json::Map::new();
    document.insert("schema_version".into(), 1.into());
    document.insert(
        "kind".into(),
        Value::String("archive-gc-trash-receipt".into()),
    );
    document.insert("generation".into(), Value::String(generation.into()));
    document.insert("archive".into(), Value::String(report.root.clone()));
    document.insert("complete".into(), Value::Bool(complete));
    document.insert("reasons".into(), Value::Array(reasons));
    document.insert("files".into(), Value::Array(entries));
    document.insert(
        "restore".into(),
        Value::String(format!(
            "from the archive root, run: cp -a {generation}/{TRASH_PAYLOAD}/. ./ -- the payload tree mirrors the archive, so every file lands back where it was taken from"
        )),
    );
    if !complete {
        document.insert(
            "note".into(),
            Value::String(
                "written before the sweep started. Every file listed is either in this generation's payload tree or still at its original path in the archive; the restore command above is correct in both cases. If the sweep finished, this file was replaced by one with complete=true.".into(),
            ),
        );
    }
    write_text_if_changed(
        &archive_path(root, &format!("{generation}/{RECEIPT_NAME}"))?,
        &dump_canonical(&Value::Object(document)),
    )?;
    Ok(())
}

fn remove_tree(path: &Path) -> i64 {
    let mut freed = 0i64;
    fn visit(dir: &Path, freed: &mut i64) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut items: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        items.sort();
        // Files first within this level; directories recursed bottom-up
        // by the caller's post-order walk.
        for item in &items {
            if is_symlink(item) || !item.is_dir() {
                if let Ok(meta) = item.symlink_metadata() {
                    *freed += meta.len() as i64;
                }
                let _ = std::fs::remove_file(item);
            }
        }
        for item in &items {
            if item.is_dir() && !is_symlink(item) {
                visit(item, freed);
                let _ = std::fs::remove_dir(item);
            }
        }
    }
    visit(path, &mut freed);
    let _ = std::fs::remove_dir(path);
    freed
}

/// Report, and -- only when asked -- sweep or empty the trash.
pub fn collect(root: &Path, delete: bool, empty_trash: bool) -> Result<GcReport, String> {
    if delete && empty_trash {
        return Err(
            "--delete and --empty-trash are separate passes on purpose: the first is reversible and the second is not, so no single invocation does both".into(),
        );
    }
    if !archive_marker_path(root).is_file() {
        return Err(format!("not a wrkviz archive: {}", root.display()));
    }
    let _lock = archive_writer_lock(root)?;
    let report = plan_collection(root);
    if empty_trash {
        let mut freed = 0i64;
        for trash_name in [TRASH_ROOT, LEGACY_TRASH_ROOT] {
            let trash = root.join(trash_name);
            if trash.is_dir() && !is_symlink(&trash) {
                freed += remove_tree(&trash);
            }
        }
        let trash_count = report.category("trash").count();
        return Ok(GcReport {
            root: report.root.clone(),
            action: "emptied".into(),
            total_bytes: report.total_bytes - freed,
            total_files: report.total_files - trash_count,
            categories: report
                .categories
                .iter()
                .map(|item| {
                    if item.name == "trash" {
                        GcCategory {
                            name: item.name.clone(),
                            reclaimable: false,
                            reason: "emptied".into(),
                            files: Vec::new(),
                        }
                    } else {
                        item.clone()
                    }
                })
                .collect(),
            trash_relative_path: TRASH_ROOT.into(),
            trash_generation: None,
            moved: Vec::new(),
            removed_directories: Vec::new(),
            emptied_bytes: freed,
        });
    }
    if !delete {
        return Ok(report);
    }
    let planned: Vec<(String, GcFile)> = report
        .categories
        .iter()
        .filter(|category| category.reclaimable)
        .flat_map(|category| {
            category
                .files
                .iter()
                .map(|item| (category.name.clone(), item.clone()))
        })
        .collect();
    let now = chrono::Utc::now();
    let stamp = now.format("%Y%m%dT%H%M%SZ").to_string();
    let generation = generation_directory(root, &stamp)?;
    if !planned.is_empty() {
        receipt(root, &generation, &report, &planned, false)?;
    }
    let mut moved: Vec<(String, GcFile)> = Vec::new();
    for (category_name, item) in &planned {
        move_to_trash(root, &generation, &item.relative_path)?;
        moved.push((category_name.clone(), item.clone()));
    }
    let mut removed: Vec<String> = Vec::new();
    for relative in &report.removed_directories {
        let candidate = archive_path(root, relative)?;
        if std::fs::remove_dir(&candidate).is_ok() {
            removed.push(relative.clone());
        }
    }
    let swept = GcReport {
        root: report.root.clone(),
        action: "swept".into(),
        total_bytes: report.total_bytes,
        total_files: report.total_files,
        categories: report
            .categories
            .iter()
            .map(|item| {
                if item.reclaimable {
                    GcCategory {
                        name: item.name.clone(),
                        reclaimable: true,
                        reason: item.reason.clone(),
                        files: Vec::new(),
                    }
                } else {
                    item.clone()
                }
            })
            .collect(),
        trash_relative_path: TRASH_ROOT.into(),
        trash_generation: Some(generation.clone()),
        moved: moved.iter().map(|(_, item)| item.clone()).collect(),
        removed_directories: removed,
        emptied_bytes: 0,
    };
    if !moved.is_empty() {
        receipt(root, &generation, &report, &moved, true)?;
    }
    Ok(swept)
}

fn mib(value: i64) -> String {
    let rendered = format!("{:.1}", value as f64 / (1024.0 * 1024.0));
    // Python f"{value:,.1f}": thousands separators in the integer part.
    let (int_part, frac) = rendered.split_once('.').unwrap_or((&rendered, "0"));
    let negative = int_part.starts_with('-');
    let digits = int_part.trim_start_matches('-');
    let mut grouped = String::new();
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    format!("{}{grouped}.{frac} MiB", if negative { "-" } else { "" })
}

fn grouped_int(value: i64) -> String {
    let digits = value.abs().to_string();
    let mut grouped = String::new();
    for (index, ch) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    format!("{}{grouped}", if value < 0 { "-" } else { "" })
}

/// Render a report for a human or for a machine.
pub fn format_gc_report(report: &GcReport, output_format: &str) -> Result<String, String> {
    if output_format == "json" {
        return Ok(format!("{}\n", dump_canonical(&report.to_json_obj(true))));
    }
    if output_format != "text" {
        return Err(format!(
            "unsupported gc report format {}",
            py_repr_str(output_format)
        ));
    }
    let mut lines: Vec<String> = vec![
        format!("archive: {}", report.root),
        format!(
            "  {} files, {}",
            grouped_int(report.total_files),
            mib(report.total_bytes)
        ),
        String::new(),
    ];
    for category in &report.categories {
        let state = if category.reclaimable {
            "reclaimable"
        } else {
            "held"
        };
        lines.push(format!(
            "{}: {} files, {} [{state}]",
            category.name,
            grouped_int(category.count()),
            mib(category.bytes())
        ));
        lines.push(format!("  {}", category.reason));
    }
    lines.push(String::new());
    lines.push(format!(
        "reclaimable now: {} files, {}",
        grouped_int(report.reclaimable_files()),
        mib(report.reclaimable_bytes())
    ));
    if report.action == "dry-run" {
        if !report.removed_directories.is_empty() {
            lines.push(format!(
                "empty directories to remove: {}",
                grouped_int(report.removed_directories.len() as i64)
            ));
        }
        lines.push(String::new());
        lines.push(format!(
            "this was a dry run; nothing was moved or deleted. To reclaim, add --delete: every file above is moved into {}/<generation>/ with its archive path preserved, and a receipt beside it says how to put it back. Emptying that trash for good is a separate command, `gc --empty-trash`.",
            report.trash_relative_path
        ));
    } else if report.action == "swept" {
        let generation = report.trash_generation.as_deref().unwrap_or_default();
        lines.push(format!(
            "moved {} file(s) into {generation}/{TRASH_PAYLOAD}/",
            grouped_int(report.moved.len() as i64)
        ));
        if !report.removed_directories.is_empty() {
            lines.push(format!(
                "removed {} empty director(ies)",
                grouped_int(report.removed_directories.len() as i64)
            ));
        }
        lines.push(format!(
            "to undo, from the archive root: cp -a {generation}/{TRASH_PAYLOAD}/. ./"
        ));
        lines.push("to make it permanent: gc --empty-trash".into());
    } else {
        lines.push(format!(
            "emptied the trash: {} freed permanently",
            mib(report.emptied_bytes)
        ));
    }
    Ok(format!("{}\n", lines.join("\n")))
}
