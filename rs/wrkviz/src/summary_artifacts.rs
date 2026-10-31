//! Immutable provenance for model-backed artifacts, at parity with
//! `py/wrkviz/summary_artifacts.py`. The envelope format string keeps
//! the tool's former name deliberately: it is stored in paid artifacts.

use serde_json::Value;

use crate::json::{as_array, as_int, as_object, as_string};
use crate::summary_registry::{
    summarizer_change_for_prompt, summarizer_for_id, ContextCoverage, SummarizerSpec,
};
use crate::token_usage::content_hash_json;

/// Stored envelope format (former tool name, deliberately).
pub const ARTIFACT_ENVELOPE_FORMAT: &str = "agent-team-timeline-model-artifact";
/// Stored envelope version.
pub const ARTIFACT_ENVELOPE_VERSION: i64 = 1;

fn identity_json(
    summarizer_id: &str,
    summarizer_version: i64,
    prompt_version: &str,
    output_schema_version: i64,
    logical_key: &str,
    team_slug: &str,
    start_ms: i64,
    end_ms: i64,
    input_hash: &str,
    backend: &str,
    model: &str,
    reasoning_effort: Option<&str>,
    service_tier: Option<&str>,
) -> Value {
    let opt = |value: Option<&str>| value.map_or(Value::Null, |text| Value::String(text.into()));
    let mut out = serde_json::Map::new();
    out.insert("summarizer_id".into(), Value::String(summarizer_id.into()));
    out.insert("summarizer_version".into(), summarizer_version.into());
    out.insert("prompt_version".into(), Value::String(prompt_version.into()));
    out.insert("output_schema_version".into(), output_schema_version.into());
    out.insert("logical_key".into(), Value::String(logical_key.into()));
    out.insert("team_slug".into(), Value::String(team_slug.into()));
    out.insert("start_ms".into(), start_ms.into());
    out.insert("end_ms".into(), end_ms.into());
    out.insert("input_hash".into(), Value::String(input_hash.into()));
    out.insert("backend".into(), Value::String(backend.into()));
    out.insert("model".into(), Value::String(model.into()));
    out.insert("reasoning_effort".into(), opt(reasoning_effort));
    out.insert("service_tier".into(), opt(service_tier));
    Value::Object(out)
}

fn artifact_id(identity: &Value) -> String {
    format!("summary-{}", content_hash_json(identity))
}

/// Version, context, dependency, model, and receipt identity for one result.
#[derive(Clone, Debug, PartialEq)]
pub struct SummaryArtifactProvenance {
    /// Content-addressed artifact id.
    pub artifact_id: String,
    /// Summarizer id.
    pub summarizer_id: String,
    /// Summarizer version.
    pub summarizer_version: i64,
    /// Prompt version.
    pub prompt_version: String,
    /// Output schema version.
    pub output_schema_version: i64,
    /// Logical key.
    pub logical_key: String,
    /// Team slug.
    pub team_slug: String,
    /// Start ms.
    pub start_ms: i64,
    /// End ms.
    pub end_ms: i64,
    /// Input hash.
    pub input_hash: String,
    /// Backend.
    pub backend: String,
    /// Model.
    pub model: String,
    /// Reasoning effort.
    pub reasoning_effort: Option<String>,
    /// Service tier.
    pub service_tier: Option<String>,
    /// Generated at.
    pub generated_at: String,
    /// Usage receipt id.
    pub usage_receipt_id: Option<String>,
    /// Context coverage.
    pub context_coverage: ContextCoverage,
    /// Dependency artifact keys.
    pub dependency_keys: Vec<String>,
    /// Legacy storage flag.
    pub legacy_storage: bool,
}

impl SummaryArtifactProvenance {
    #[allow(clippy::too_many_arguments)]
    fn validate(
        artifact_id_value: &str,
        summarizer_id: &str,
        summarizer_version: i64,
        prompt_version: &str,
        output_schema_version: i64,
        logical_key: &str,
        team_slug: &str,
        start_ms: i64,
        end_ms: i64,
        input_hash: &str,
        backend: &str,
        model: &str,
        reasoning_effort: Option<&str>,
        service_tier: Option<&str>,
    ) -> Result<(), String> {
        let spec = summarizer_for_id(summarizer_id)?;
        let change = summarizer_change_for_prompt(spec, prompt_version)?;
        if change.version != summarizer_version {
            return Err("summarizer version does not match prompt version".to_string());
        }
        if change.output_schema_version != output_schema_version {
            return Err("output schema does not match summarizer version".to_string());
        }
        if end_ms < start_ms {
            return Err("summary artifact ends before it starts".to_string());
        }
        if logical_key.is_empty() || team_slug.is_empty() || input_hash.is_empty() {
            return Err("summary artifact identity fields must not be empty".to_string());
        }
        let identity = identity_json(
            summarizer_id,
            summarizer_version,
            prompt_version,
            output_schema_version,
            logical_key,
            team_slug,
            start_ms,
            end_ms,
            input_hash,
            backend,
            model,
            reasoning_effort,
            service_tier,
        );
        if artifact_id_value != artifact_id(&identity) {
            return Err("summary artifact ID does not match its identity".to_string());
        }
        Ok(())
    }

    /// The fields that determine immutable artifact identity.
    pub fn identity_json_obj(&self) -> Value {
        identity_json(
            &self.summarizer_id,
            self.summarizer_version,
            &self.prompt_version,
            self.output_schema_version,
            &self.logical_key,
            &self.team_slug,
            self.start_ms,
            self.end_ms,
            &self.input_hash,
            &self.backend,
            &self.model,
            self.reasoning_effort.as_deref(),
            self.service_tier.as_deref(),
        )
    }

    /// Complete persisted provenance.
    pub fn to_json_obj(&self) -> Value {
        let opt = |value: &Option<String>| value.clone().map_or(Value::Null, Value::String);
        let mut out = match self.identity_json_obj() {
            Value::Object(map) => map,
            _ => unreachable!(),
        };
        out.insert("artifact_id".into(), Value::String(self.artifact_id.clone()));
        out.insert("generated_at".into(), Value::String(self.generated_at.clone()));
        out.insert("usage_receipt_id".into(), opt(&self.usage_receipt_id));
        out.insert("context_coverage".into(), self.context_coverage.to_json_obj());
        out.insert(
            "dependency_keys".into(),
            Value::Array(
                self.dependency_keys
                    .iter()
                    .map(|item| Value::String(item.clone()))
                    .collect(),
            ),
        );
        out.insert("legacy_storage".into(), Value::Bool(self.legacy_storage));
        Value::Object(out)
    }

    /// Strictly parse and identity-validate persisted provenance.
    pub fn from_json_obj(value: Option<&Value>, where_: &str) -> Result<Self, String> {
        let value = value.ok_or_else(|| format!("{where_}: expected an object"))?;
        let obj = as_object(value, where_)?;
        let expected = [
            "artifact_id",
            "summarizer_id",
            "summarizer_version",
            "prompt_version",
            "output_schema_version",
            "logical_key",
            "team_slug",
            "start_ms",
            "end_ms",
            "input_hash",
            "backend",
            "model",
            "reasoning_effort",
            "service_tier",
            "generated_at",
            "usage_receipt_id",
            "context_coverage",
            "dependency_keys",
            "legacy_storage",
        ];
        if obj.len() != expected.len() || !expected.iter().all(|key| obj.contains_key(*key)) {
            return Err(format!("{where_}: invalid summary provenance fields"));
        }
        let get = |key: &str| obj.get(key).unwrap();
        let opt_string = |key: &str| -> Result<Option<String>, String> {
            match get(key) {
                Value::Null => Ok(None),
                value => Ok(Some(as_string(value, &format!("{where_}.{key}"))?.to_string())),
            }
        };
        let legacy_storage = match get("legacy_storage") {
            Value::Bool(flag) => *flag,
            _ => return Err(format!("{where_}.legacy_storage: expected a boolean")),
        };
        let raw_deps = as_array(get("dependency_keys"), &format!("{where_}.dependency_keys"))?;
        let mut dependency_keys = Vec::new();
        for (index, item) in raw_deps.iter().enumerate() {
            dependency_keys.push(
                as_string(item, &format!("{where_}.dependency_keys[{index}]"))?.to_string(),
            );
        }
        let provenance = Self {
            artifact_id: as_string(get("artifact_id"), &format!("{where_}.artifact_id"))?
                .to_string(),
            summarizer_id: as_string(get("summarizer_id"), &format!("{where_}.summarizer_id"))?
                .to_string(),
            summarizer_version: as_int(
                get("summarizer_version"),
                &format!("{where_}.summarizer_version"),
            )?,
            prompt_version: as_string(get("prompt_version"), &format!("{where_}.prompt_version"))?
                .to_string(),
            output_schema_version: as_int(
                get("output_schema_version"),
                &format!("{where_}.output_schema_version"),
            )?,
            logical_key: as_string(get("logical_key"), &format!("{where_}.logical_key"))?
                .to_string(),
            team_slug: as_string(get("team_slug"), &format!("{where_}.team_slug"))?.to_string(),
            start_ms: as_int(get("start_ms"), &format!("{where_}.start_ms"))?,
            end_ms: as_int(get("end_ms"), &format!("{where_}.end_ms"))?,
            input_hash: as_string(get("input_hash"), &format!("{where_}.input_hash"))?.to_string(),
            backend: as_string(get("backend"), &format!("{where_}.backend"))?.to_string(),
            model: as_string(get("model"), &format!("{where_}.model"))?.to_string(),
            reasoning_effort: opt_string("reasoning_effort")?,
            service_tier: opt_string("service_tier")?,
            generated_at: as_string(get("generated_at"), &format!("{where_}.generated_at"))?
                .to_string(),
            usage_receipt_id: opt_string("usage_receipt_id")?,
            context_coverage: ContextCoverage::from_json_obj(
                Some(get("context_coverage")),
                &format!("{where_}.context_coverage"),
            )?,
            dependency_keys,
            legacy_storage,
        };
        Self::validate(
            &provenance.artifact_id,
            &provenance.summarizer_id,
            provenance.summarizer_version,
            &provenance.prompt_version,
            provenance.output_schema_version,
            &provenance.logical_key,
            &provenance.team_slug,
            provenance.start_ms,
            provenance.end_ms,
            &provenance.input_hash,
            &provenance.backend,
            &provenance.model,
            provenance.reasoning_effort.as_deref(),
            provenance.service_tier.as_deref(),
        )?;
        Ok(provenance)
    }
}

/// Construct identity-checked provenance for a registered prompt version.
#[allow(clippy::too_many_arguments)]
pub fn make_summary_provenance(
    spec: &SummarizerSpec,
    logical_key: &str,
    team_slug: &str,
    start_ms: i64,
    end_ms: i64,
    input_hash: &str,
    backend: &str,
    model: &str,
    reasoning_effort: Option<&str>,
    service_tier: Option<&str>,
    generated_at: &str,
    usage_receipt_id: Option<&str>,
    context_coverage: ContextCoverage,
    dependency_keys: Vec<String>,
    legacy_storage: bool,
    prompt_version: Option<&str>,
) -> Result<SummaryArtifactProvenance, String> {
    let selected_prompt = prompt_version.unwrap_or(spec.prompt_version);
    let change = summarizer_change_for_prompt(spec, selected_prompt)?;
    let identity = identity_json(
        spec.summarizer_id,
        change.version,
        change.prompt_version,
        change.output_schema_version,
        logical_key,
        team_slug,
        start_ms,
        end_ms,
        input_hash,
        backend,
        model,
        reasoning_effort,
        service_tier,
    );
    let provenance = SummaryArtifactProvenance {
        artifact_id: artifact_id(&identity),
        summarizer_id: spec.summarizer_id.to_string(),
        summarizer_version: change.version,
        prompt_version: change.prompt_version.to_string(),
        output_schema_version: change.output_schema_version,
        logical_key: logical_key.to_string(),
        team_slug: team_slug.to_string(),
        start_ms,
        end_ms,
        input_hash: input_hash.to_string(),
        backend: backend.to_string(),
        model: model.to_string(),
        reasoning_effort: reasoning_effort.map(str::to_string),
        service_tier: service_tier.map(str::to_string),
        generated_at: generated_at.to_string(),
        usage_receipt_id: usage_receipt_id.map(str::to_string),
        context_coverage,
        dependency_keys,
        legacy_storage,
    };
    SummaryArtifactProvenance::validate(
        &provenance.artifact_id,
        &provenance.summarizer_id,
        provenance.summarizer_version,
        &provenance.prompt_version,
        provenance.output_schema_version,
        &provenance.logical_key,
        &provenance.team_slug,
        provenance.start_ms,
        provenance.end_ms,
        &provenance.input_hash,
        &provenance.backend,
        &provenance.model,
        provenance.reasoning_effort.as_deref(),
        provenance.service_tier.as_deref(),
    )?;
    Ok(provenance)
}
