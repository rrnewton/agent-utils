//! Token-usage records at parity with `py/wrkviz/token_usage.py`.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::json::{as_array, as_int, as_object, as_string, dump_compact, read_json};
use crate::transcript_io::write_json_if_changed;

/// Usage schema version.
pub const USAGE_SCHEMA_VERSION: i64 = 2;
const LEGACY_USAGE_SCHEMA_VERSION: i64 = 1;
/// Default Codex service tier.
pub const DEFAULT_SERVICE_TIER: &str = "default";

/// Resolve the effective service tier.
pub fn resolve_service_tier(
    backend: &str,
    service_tier: Option<&str>,
) -> Result<Option<String>, String> {
    if let Some(tier) = service_tier {
        if tier.trim().is_empty() {
            return Err("service tier must not be empty".to_string());
        }
    }
    if backend == "codex" {
        return Ok(Some(
            service_tier.unwrap_or(DEFAULT_SERVICE_TIER).to_string(),
        ));
    }
    if service_tier.is_some() {
        return Err("service tier is only supported by the codex backend".to_string());
    }
    Ok(None)
}

/// SHA-256 of the canonical JSON text.
pub fn content_hash(text: &str) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Content hash of a JSON value (canonical dumps).
pub fn content_hash_json(value: &Value) -> String {
    content_hash(&crate::json::dump_canonical(value))
}

fn obj_get<'a>(obj: &'a serde_json::Map<String, Value>, key: &str) -> &'a Value {
    obj.get(key).unwrap_or(&Value::Null)
}

/// The stable token breakdown of one model invocation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TokenUsage {
    /// Input tokens.
    pub input_tokens: i64,
    /// Cached input tokens.
    pub cached_input_tokens: i64,
    /// Cache-write input tokens.
    pub cache_write_input_tokens: i64,
    /// Output tokens.
    pub output_tokens: i64,
    /// Reasoning output tokens.
    pub reasoning_output_tokens: i64,
}

impl TokenUsage {
    /// Validate and construct.
    pub fn new(
        input_tokens: i64,
        cached_input_tokens: i64,
        cache_write_input_tokens: i64,
        output_tokens: i64,
        reasoning_output_tokens: i64,
    ) -> Result<Self, String> {
        let usage = Self {
            input_tokens,
            cached_input_tokens,
            cache_write_input_tokens,
            output_tokens,
            reasoning_output_tokens,
        };
        if [
            input_tokens,
            cached_input_tokens,
            cache_write_input_tokens,
            output_tokens,
            reasoning_output_tokens,
        ]
        .iter()
        .any(|value| *value < 0)
        {
            return Err("token counts must be non-negative".to_string());
        }
        if cached_input_tokens > input_tokens {
            return Err("cached input tokens exceed input tokens".to_string());
        }
        if reasoning_output_tokens > output_tokens {
            return Err("reasoning output tokens exceed output tokens".to_string());
        }
        Ok(usage)
    }

    /// Total tokens (input plus output).
    pub fn total_tokens(&self) -> i64 {
        self.input_tokens + self.output_tokens
    }

    /// Add two usages.
    pub fn add(&self, other: &Self) -> Self {
        Self {
            input_tokens: self.input_tokens + other.input_tokens,
            cached_input_tokens: self.cached_input_tokens + other.cached_input_tokens,
            cache_write_input_tokens: self.cache_write_input_tokens
                + other.cache_write_input_tokens,
            output_tokens: self.output_tokens + other.output_tokens,
            reasoning_output_tokens: self.reasoning_output_tokens + other.reasoning_output_tokens,
        }
    }

    /// Persisted form.
    pub fn to_json(&self) -> Value {
        let mut out = serde_json::Map::new();
        out.insert("input_tokens".into(), self.input_tokens.into());
        out.insert(
            "cached_input_tokens".into(),
            self.cached_input_tokens.into(),
        );
        out.insert(
            "cache_write_input_tokens".into(),
            self.cache_write_input_tokens.into(),
        );
        out.insert("output_tokens".into(), self.output_tokens.into());
        out.insert(
            "reasoning_output_tokens".into(),
            self.reasoning_output_tokens.into(),
        );
        out.insert("total_tokens".into(), self.total_tokens().into());
        Value::Object(out)
    }

    /// Strictly parse persisted counters.
    pub fn from_json(value: Option<&Value>, where_: &str) -> Result<Self, String> {
        let value = value.ok_or_else(|| format!("{where_}: expected an object"))?;
        let obj = as_object(value, where_)?;
        let expected = [
            "input_tokens",
            "cached_input_tokens",
            "cache_write_input_tokens",
            "output_tokens",
            "reasoning_output_tokens",
            "total_tokens",
        ];
        if obj.len() != expected.len() || !expected.iter().all(|key| obj.contains_key(*key)) {
            let sorted: Vec<String> = {
                let mut items: Vec<&str> = expected.to_vec();
                items.sort_unstable();
                items
                    .iter()
                    .map(|item| crate::json::py_repr_str(item))
                    .collect()
            };
            return Err(format!(
                "{where_}: expected exactly [{}]",
                sorted.join(", ")
            ));
        }
        let usage = Self::new(
            as_int(
                obj_get(obj, "input_tokens"),
                &format!("{where_}.input_tokens"),
            )?,
            as_int(
                obj_get(obj, "cached_input_tokens"),
                &format!("{where_}.cached_input_tokens"),
            )?,
            as_int(
                obj_get(obj, "cache_write_input_tokens"),
                &format!("{where_}.cache_write_input_tokens"),
            )?,
            as_int(
                obj_get(obj, "output_tokens"),
                &format!("{where_}.output_tokens"),
            )?,
            as_int(
                obj_get(obj, "reasoning_output_tokens"),
                &format!("{where_}.reasoning_output_tokens"),
            )?,
        )?;
        let recorded = as_int(
            obj_get(obj, "total_tokens"),
            &format!("{where_}.total_tokens"),
        )?;
        if recorded != usage.total_tokens() {
            return Err(format!("{where_}.total_tokens: inconsistent token total"));
        }
        Ok(usage)
    }
}

impl std::ops::AddAssign for TokenUsage {
    fn add_assign(&mut self, other: Self) {
        *self = self.add(&other);
    }
}

/// Immutable provenance and usage for one backend batch call.
#[derive(Clone, Debug, PartialEq)]
pub struct BatchUsageReceipt {
    /// Content-addressed receipt id.
    pub receipt_id: String,
    /// Backend.
    pub backend: String,
    /// Model.
    pub model: String,
    /// Reasoning effort.
    pub reasoning_effort: Option<String>,
    /// Service tier.
    pub service_tier: Option<String>,
    /// Status.
    pub status: String,
    /// Started at.
    pub started_at: String,
    /// Completed at.
    pub completed_at: String,
    /// Input hashes.
    pub input_hashes: Vec<String>,
    /// Usage, if reported.
    pub usage: Option<TokenUsage>,
    /// Error text.
    pub error: Option<String>,
    /// Schema version.
    pub schema_version: i64,
}

#[allow(clippy::too_many_arguments)]
impl BatchUsageReceipt {
    fn validate(&self) -> Result<(), String> {
        if !matches!(
            self.schema_version,
            LEGACY_USAGE_SCHEMA_VERSION | USAGE_SCHEMA_VERSION
        ) {
            return Err(format!(
                "unsupported usage schema version {}",
                self.schema_version
            ));
        }
        if let Some(tier) = &self.service_tier {
            if tier.trim().is_empty() {
                return Err("service tier must not be empty".to_string());
            }
        }
        if self.schema_version == LEGACY_USAGE_SCHEMA_VERSION && self.service_tier.is_some() {
            return Err("schema v1 batch receipts cannot contain a service tier".to_string());
        }
        Ok(())
    }

    /// Create a content-addressed receipt from one backend attempt.
    pub fn create(
        backend: &str,
        model: &str,
        reasoning_effort: Option<&str>,
        service_tier: Option<&str>,
        status: &str,
        started_at: &str,
        completed_at: &str,
        input_hashes: &[String],
        usage: Option<TokenUsage>,
        error: Option<&str>,
    ) -> Result<Self, String> {
        let mut identity = serde_json::Map::new();
        identity.insert("schema_version".into(), USAGE_SCHEMA_VERSION.into());
        identity.insert("backend".into(), Value::String(backend.into()));
        identity.insert("model".into(), Value::String(model.into()));
        identity.insert(
            "reasoning_effort".into(),
            reasoning_effort.map_or(Value::Null, |text| Value::String(text.into())),
        );
        identity.insert(
            "service_tier".into(),
            service_tier.map_or(Value::Null, |text| Value::String(text.into())),
        );
        identity.insert("status".into(), Value::String(status.into()));
        identity.insert("started_at".into(), Value::String(started_at.into()));
        identity.insert("completed_at".into(), Value::String(completed_at.into()));
        identity.insert(
            "input_hashes".into(),
            Value::Array(
                input_hashes
                    .iter()
                    .map(|item| Value::String(item.clone()))
                    .collect(),
            ),
        );
        identity.insert(
            "usage".into(),
            usage.map_or(Value::Null, |item| item.to_json()),
        );
        identity.insert(
            "error".into(),
            error.map_or(Value::Null, |text| Value::String(text.into())),
        );
        let receipt_id = content_hash_json(&Value::Object(identity));
        let receipt = Self {
            receipt_id,
            backend: backend.into(),
            model: model.into(),
            reasoning_effort: reasoning_effort.map(str::to_string),
            service_tier: service_tier.map(str::to_string),
            status: status.into(),
            started_at: started_at.into(),
            completed_at: completed_at.into(),
            input_hashes: input_hashes.to_vec(),
            usage,
            error: error.map(str::to_string),
            schema_version: USAGE_SCHEMA_VERSION,
        };
        receipt.validate()?;
        Ok(receipt)
    }

    /// Persisted form.
    pub fn to_json(&self) -> Value {
        let opt = |value: &Option<String>| value.clone().map_or(Value::Null, Value::String);
        let mut out = serde_json::Map::new();
        out.insert("schema_version".into(), self.schema_version.into());
        out.insert("receipt_id".into(), Value::String(self.receipt_id.clone()));
        out.insert("backend".into(), Value::String(self.backend.clone()));
        out.insert("model".into(), Value::String(self.model.clone()));
        out.insert("reasoning_effort".into(), opt(&self.reasoning_effort));
        out.insert("status".into(), Value::String(self.status.clone()));
        out.insert("started_at".into(), Value::String(self.started_at.clone()));
        out.insert(
            "completed_at".into(),
            Value::String(self.completed_at.clone()),
        );
        out.insert(
            "input_hashes".into(),
            Value::Array(
                self.input_hashes
                    .iter()
                    .map(|item| Value::String(item.clone()))
                    .collect(),
            ),
        );
        out.insert(
            "usage".into(),
            self.usage.map_or(Value::Null, |item| item.to_json()),
        );
        out.insert("error".into(), opt(&self.error));
        if self.schema_version == USAGE_SCHEMA_VERSION {
            out.insert("service_tier".into(), opt(&self.service_tier));
        }
        Value::Object(out)
    }

    /// Strictly parse and hash-validate one receipt.
    pub fn from_json(value: &Value, where_: &str) -> Result<Self, String> {
        let obj = as_object(value, where_)?;
        let version = as_int(
            obj_get(obj, "schema_version"),
            &format!("{where_}.schema_version"),
        )?;
        let mut expected = vec![
            "schema_version",
            "receipt_id",
            "backend",
            "model",
            "reasoning_effort",
            "status",
            "started_at",
            "completed_at",
            "input_hashes",
            "usage",
            "error",
        ];
        if version == USAGE_SCHEMA_VERSION {
            expected.push("service_tier");
        } else if version != LEGACY_USAGE_SCHEMA_VERSION {
            return Err(format!("{where_}: unsupported schema version {version}"));
        }
        if obj.len() != expected.len() || !expected.iter().all(|key| obj.contains_key(*key)) {
            let mut sorted = expected.clone();
            sorted.sort_unstable();
            let rendered: Vec<String> = sorted
                .iter()
                .map(|item| crate::json::py_repr_str(item))
                .collect();
            return Err(format!(
                "{where_}: expected exactly [{}]",
                rendered.join(", ")
            ));
        }
        let opt_string = |key: &str| -> Result<Option<String>, String> {
            match obj_get(obj, key) {
                Value::Null => Ok(None),
                value => Ok(Some(
                    as_string(value, &format!("{where_}.{key}"))?.to_string(),
                )),
            }
        };
        let raw_hashes = as_array(
            obj_get(obj, "input_hashes"),
            &format!("{where_}.input_hashes"),
        )?;
        let mut input_hashes = Vec::new();
        for (index, item) in raw_hashes.iter().enumerate() {
            input_hashes
                .push(as_string(item, &format!("{where_}.input_hashes[{index}]"))?.to_string());
        }
        let usage = match obj_get(obj, "usage") {
            Value::Null => None,
            value => Some(TokenUsage::from_json(
                Some(value),
                &format!("{where_}.usage"),
            )?),
        };
        let receipt = Self {
            receipt_id: as_string(obj_get(obj, "receipt_id"), &format!("{where_}.receipt_id"))?
                .to_string(),
            backend: as_string(obj_get(obj, "backend"), &format!("{where_}.backend"))?.to_string(),
            model: as_string(obj_get(obj, "model"), &format!("{where_}.model"))?.to_string(),
            reasoning_effort: opt_string("reasoning_effort")?,
            service_tier: if version == USAGE_SCHEMA_VERSION {
                opt_string("service_tier")?
            } else {
                None
            },
            status: as_string(obj_get(obj, "status"), &format!("{where_}.status"))?.to_string(),
            started_at: as_string(obj_get(obj, "started_at"), &format!("{where_}.started_at"))?
                .to_string(),
            completed_at: as_string(
                obj_get(obj, "completed_at"),
                &format!("{where_}.completed_at"),
            )?
            .to_string(),
            input_hashes,
            usage,
            error: opt_string("error")?,
            schema_version: version,
        };
        receipt.validate()?;
        let mut identity = match receipt.to_json() {
            Value::Object(map) => map,
            _ => unreachable!(),
        };
        identity.remove("receipt_id");
        if receipt.receipt_id != content_hash_json(&Value::Object(identity)) {
            return Err(format!("{where_}.receipt_id: content hash mismatch"));
        }
        Ok(receipt)
    }
}

/// Aggregate usage and receipt location for one cache operation.
#[derive(Clone, Debug)]
pub struct UsageRunAccounting {
    /// Receipt path.
    pub path: PathBuf,
    /// Newly spent usage.
    pub newly_spent_usage: TokenUsage,
    /// New receipts with unknown usage.
    pub newly_spent_unknown_receipts: i64,
    /// Usage behind returned artifacts.
    pub artifact_generation_usage: TokenUsage,
    /// Artifact receipts with unknown usage.
    pub artifact_generation_unknown_receipts: i64,
}

/// Parse the final Codex `turn.completed` usage, if present.
pub fn parse_codex_jsonl_usage(text: &str) -> Result<Option<TokenUsage>, String> {
    let mut found = None;
    for (index, line) in text.lines().enumerate() {
        let candidate = line.trim();
        if !candidate.starts_with('{') {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(candidate) else {
            continue;
        };
        let Ok(event) = as_object(&value, "event") else {
            continue;
        };
        if event.get("type").and_then(Value::as_str) != Some("turn.completed") {
            continue;
        }
        let _ = index;
        let usage_value = event.get("usage").cloned().unwrap_or(Value::Null);
        let usage_obj = as_object(&usage_value, "turn.completed.usage")?;
        let optional = |key: &str| -> Result<i64, String> {
            if usage_obj.contains_key(key) {
                as_int(
                    usage_obj.get(key).unwrap(),
                    &format!("turn.completed.usage.{key}"),
                )
            } else {
                Ok(0)
            }
        };
        found = Some(TokenUsage::new(
            as_int(
                obj_get(usage_obj, "input_tokens"),
                "turn.completed.usage.input_tokens",
            )?,
            as_int(
                obj_get(usage_obj, "cached_input_tokens"),
                "turn.completed.usage.cached_input_tokens",
            )?,
            optional("cache_write_input_tokens")?,
            as_int(
                obj_get(usage_obj, "output_tokens"),
                "turn.completed.usage.output_tokens",
            )?,
            optional("reasoning_output_tokens")?,
        )?);
    }
    Ok(found)
}

/// Parse Claude `--output-format json` usage.
pub fn parse_claude_json_usage(text: &str) -> Result<TokenUsage, String> {
    let value: Value = serde_json::from_str(text)
        .map_err(|error| format!("claude JSON result is invalid: {error}"))?;
    let root = as_object(&value, "claude JSON result")?;
    let usage_value = root.get("usage").cloned().unwrap_or(Value::Null);
    let usage = as_object(&usage_value, "claude JSON result.usage")?;
    let direct = as_int(
        obj_get(usage, "input_tokens"),
        "claude JSON result.usage.input_tokens",
    )?;
    let cache_write = as_int(
        obj_get(usage, "cache_creation_input_tokens"),
        "claude JSON result.usage.cache_creation_input_tokens",
    )?;
    let cache_read = as_int(
        obj_get(usage, "cache_read_input_tokens"),
        "claude JSON result.usage.cache_read_input_tokens",
    )?;
    let output = as_int(
        obj_get(usage, "output_tokens"),
        "claude JSON result.usage.output_tokens",
    )?;
    TokenUsage::new(
        direct + cache_write + cache_read,
        cache_read,
        cache_write,
        output,
        0,
    )
}

/// Persist one batch receipt.
pub fn write_batch_receipt(root: &Path, receipt: &BatchUsageReceipt) -> Result<PathBuf, String> {
    let path = root
        .join("receipts")
        .join(format!("{}.json", receipt.receipt_id));
    write_json_if_changed(&path, &receipt.to_json())?;
    Ok(path)
}

/// Load one receipt, if present and valid.
pub fn load_batch_receipt(root: &Path, receipt_id: &str) -> Option<BatchUsageReceipt> {
    let path = root.join("receipts").join(format!("{receipt_id}.json"));
    let value = read_json(&path).ok()?;
    BatchUsageReceipt::from_json(&value, &path.display().to_string()).ok()
}

fn sum_receipt_usage(receipts: &[BatchUsageReceipt]) -> (TokenUsage, i64) {
    let mut total = TokenUsage::default();
    let mut unknown = 0i64;
    for receipt in receipts {
        match receipt.usage {
            None => unknown += 1,
            Some(usage) => total += usage,
        }
    }
    (total, unknown)
}

/// Persist an immutable invocation receipt and return its aggregates.
#[allow(clippy::too_many_arguments)]
pub fn write_usage_run_receipt(
    root: &Path,
    started_at: &str,
    completed_at: &str,
    backend: &str,
    model: &str,
    reasoning_effort: Option<&str>,
    service_tier: Option<&str>,
    job_count: usize,
    hits: usize,
    misses: usize,
    new_receipts: &[BatchUsageReceipt],
    artifact_receipts: &[BatchUsageReceipt],
    unreadable_artifact_receipt_ids: &[String],
    unknown_legacy_artifacts: i64,
) -> Result<UsageRunAccounting, String> {
    if let Some(tier) = service_tier {
        if tier.trim().is_empty() {
            return Err("service tier must not be empty".to_string());
        }
    }
    let (newly_spent_usage, newly_unknown) = sum_receipt_usage(new_receipts);
    let (artifact_usage, artifact_unknown_known) = sum_receipt_usage(artifact_receipts);
    let artifact_unknown = artifact_unknown_known + unreadable_artifact_receipt_ids.len() as i64;
    let mut batch_ids: Vec<String> = new_receipts
        .iter()
        .map(|item| item.receipt_id.clone())
        .collect();
    batch_ids.sort();
    let mut artifact_ids: Vec<String> = artifact_receipts
        .iter()
        .map(|item| item.receipt_id.clone())
        .chain(unreadable_artifact_receipt_ids.iter().cloned())
        .collect();
    artifact_ids.sort();
    let strings = |items: &[String]| -> Value {
        Value::Array(
            items
                .iter()
                .map(|item| Value::String(item.clone()))
                .collect(),
        )
    };
    let opt = |value: Option<&str>| value.map_or(Value::Null, |text| Value::String(text.into()));
    let mut identity = serde_json::Map::new();
    identity.insert("started_at".into(), Value::String(started_at.into()));
    identity.insert("completed_at".into(), Value::String(completed_at.into()));
    identity.insert("backend".into(), Value::String(backend.into()));
    identity.insert("model".into(), Value::String(model.into()));
    identity.insert("reasoning_effort".into(), opt(reasoning_effort));
    identity.insert("service_tier".into(), opt(service_tier));
    identity.insert("job_count".into(), (job_count as i64).into());
    identity.insert("cache_hits".into(), (hits as i64).into());
    identity.insert("cache_misses".into(), (misses as i64).into());
    identity.insert("batch_receipt_ids".into(), strings(&batch_ids));
    identity.insert("artifact_receipt_ids".into(), strings(&artifact_ids));
    let run_id = content_hash_json(&Value::Object(identity));
    let mut obj = serde_json::Map::new();
    obj.insert("schema_version".into(), USAGE_SCHEMA_VERSION.into());
    obj.insert("run_id".into(), Value::String(run_id.clone()));
    obj.insert("started_at".into(), Value::String(started_at.into()));
    obj.insert("completed_at".into(), Value::String(completed_at.into()));
    obj.insert("backend".into(), Value::String(backend.into()));
    obj.insert("model".into(), Value::String(model.into()));
    obj.insert("reasoning_effort".into(), opt(reasoning_effort));
    obj.insert("service_tier".into(), opt(service_tier));
    obj.insert("job_count".into(), (job_count as i64).into());
    obj.insert("cache_hits".into(), (hits as i64).into());
    obj.insert("cache_misses".into(), (misses as i64).into());
    obj.insert("backend_batches".into(), (batch_ids.len() as i64).into());
    obj.insert("batch_receipt_ids".into(), strings(&batch_ids));
    obj.insert("artifact_receipt_ids".into(), strings(&artifact_ids));
    obj.insert("newly_spent_usage".into(), newly_spent_usage.to_json());
    obj.insert("newly_spent_unknown_receipts".into(), newly_unknown.into());
    obj.insert("artifact_generation_usage".into(), artifact_usage.to_json());
    obj.insert(
        "artifact_generation_unknown_receipts".into(),
        artifact_unknown.into(),
    );
    obj.insert(
        "unknown_legacy_artifacts".into(),
        unknown_legacy_artifacts.into(),
    );
    let path = root.join("runs").join(format!("{run_id}.json"));
    write_json_if_changed(&path, &Value::Object(obj))?;
    Ok(UsageRunAccounting {
        path,
        newly_spent_usage,
        newly_spent_unknown_receipts: newly_unknown,
        artifact_generation_usage: artifact_usage,
        artifact_generation_unknown_receipts: artifact_unknown,
    })
}

/// Canonical compact dumps of a value (receipt identity in tests).
pub fn compact_json(value: &Value) -> String {
    dump_compact(value)
}
