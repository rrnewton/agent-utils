//! Registered summarizer contracts and context-coverage provenance,
//! at parity with `py/wrkviz/summary_registry.py`. The prompt-version
//! strings keep the tool's former name deliberately: they are written
//! into paid artifacts and act as cache keys.

use serde_json::Value;

use crate::json::{as_array, as_int, as_object, as_string};

/// Summary style for one agent phase.
pub const PHASE_STYLE: &str = "phase";
/// Summary style for technical calendar rollups.
pub const TECHNICAL_ROLLUP_STYLE: &str = "technical-rollup";
/// Summary style for plain-language calendar rollups.
pub const PLAIN_LANGUAGE_ROLLUP_STYLE: &str = "plain-language-rollup";
/// Summary style for the project overview.
pub const PROJECT_OVERVIEW_STYLE: &str = "project-overview";
/// Summary style for glossary definitions.
pub const GLOSSARY_DEFINITION_STYLE: &str = "glossary-definition";
/// Summary style for hindsight agent names.
pub const AGENT_LIFETIME_STYLE: &str = "agent-lifetime";

/// One prompt/output contract revision.
#[derive(Clone, Debug, PartialEq)]
pub struct SummarizerChange {
    /// Numeric version.
    pub version: i64,
    /// Prompt version string.
    pub prompt_version: &'static str,
    /// Output schema version.
    pub output_schema_version: i64,
    /// Change summary.
    pub change: &'static str,
}

impl SummarizerChange {
    /// Deterministic registry metadata.
    pub fn to_json_obj(&self) -> Value {
        let mut out = serde_json::Map::new();
        out.insert("version".into(), self.version.into());
        out.insert(
            "prompt_version".into(),
            Value::String(self.prompt_version.into()),
        );
        out.insert(
            "output_schema_version".into(),
            self.output_schema_version.into(),
        );
        out.insert("change".into(), Value::String(self.change.into()));
        Value::Object(out)
    }
}

const fn ch(
    version: i64,
    prompt_version: &'static str,
    output_schema_version: i64,
    change: &'static str,
) -> SummarizerChange {
    SummarizerChange {
        version,
        prompt_version,
        output_schema_version,
        change,
    }
}

/// The selected contract and version ledger for one model task.
#[derive(Clone, Debug)]
pub struct SummarizerSpec {
    /// Durable summarizer id.
    pub summarizer_id: &'static str,
    /// Summary style.
    pub summary_style: &'static str,
    /// Current numeric version.
    pub current_version: i64,
    /// Current prompt version.
    pub prompt_version: &'static str,
    /// Current output schema version.
    pub output_schema_version: i64,
    /// Description.
    pub description: &'static str,
    /// Input field descriptions.
    pub input_fields: &'static [&'static str],
    /// Output field descriptions.
    pub output_fields: &'static [&'static str],
    /// Granularities.
    pub granularities: &'static [&'static str],
    /// Changelog.
    pub changelog: &'static [SummarizerChange],
    /// Lifecycle.
    pub lifecycle: &'static str,
}

impl SummarizerSpec {
    /// Deterministic registry metadata.
    pub fn to_json_obj(&self) -> Value {
        let strings = |items: &[&str]| -> Value {
            Value::Array(
                items
                    .iter()
                    .map(|item| Value::String((*item).into()))
                    .collect(),
            )
        };
        let mut out = serde_json::Map::new();
        out.insert(
            "summarizer_id".into(),
            Value::String(self.summarizer_id.into()),
        );
        out.insert(
            "summary_style".into(),
            Value::String(self.summary_style.into()),
        );
        out.insert("current_version".into(), self.current_version.into());
        out.insert(
            "prompt_version".into(),
            Value::String(self.prompt_version.into()),
        );
        out.insert(
            "output_schema_version".into(),
            self.output_schema_version.into(),
        );
        out.insert("description".into(), Value::String(self.description.into()));
        out.insert("input_fields".into(), strings(self.input_fields));
        out.insert("output_fields".into(), strings(self.output_fields));
        out.insert("granularities".into(), strings(self.granularities));
        out.insert(
            "changelog".into(),
            Value::Array(
                self.changelog
                    .iter()
                    .map(SummarizerChange::to_json_obj)
                    .collect(),
            ),
        );
        out.insert("lifecycle".into(), Value::String(self.lifecycle.into()));
        Value::Object(out)
    }
}

/// Phase summary contract.
pub static PHASE_SUMMARIZER: SummarizerSpec = SummarizerSpec {
    summarizer_id: "phase-work-summary",
    summary_style: PHASE_STYLE,
    current_version: 2,
    prompt_version: "agent-team-timeline-summary-v2",
    output_schema_version: 1,
    description: "Phrase, hover paragraph, and chronological work bullets for one agent phase.",
    input_fields: &[
        "bounded phase transcript",
        "ancestor transcript scroll-back",
        "chronologically available glossary",
        "phase activity statistics",
    ],
    output_fields: &["phrase", "paragraph", "work_summary"],
    granularities: &["phase"],
    changelog: &[
        ch(
            1,
            "agent-team-timeline-summary-v1",
            1,
            "Initial substantive phase summary with three display resolutions.",
        ),
        ch(
            2,
            "agent-team-timeline-summary-v2",
            1,
            "Use only supported semantic concepts; exclude mechanical glossary candidates.",
        ),
    ],
    lifecycle: "active",
};

/// Technical rollup contract.
pub static TECHNICAL_ROLLUP_SUMMARIZER: SummarizerSpec = SummarizerSpec {
    summarizer_id: "technical-rollup",
    summary_style: TECHNICAL_ROLLUP_STYLE,
    current_version: 3,
    prompt_version: "agent-team-timeline-technical-rollup-v3",
    output_schema_version: 1,
    description: "Content-led technical rollup over lower-level cached summaries.",
    input_fields: &[
        "lower-level summaries or uncovered phase summaries",
        "up to ten prior same-level technical summaries",
        "chronologically available glossary",
        "aggregate activity statistics",
    ],
    output_fields: &["phrase", "paragraph", "work_summary"],
    granularities: &["hourly", "daily", "weekly", "monthly", "quarterly"],
    changelog: &[
        ch(
            1,
            "agent-team-timeline-technical-rollup-v1",
            1,
            "Initial calendar super-summary.",
        ),
        ch(
            2,
            "agent-team-timeline-technical-rollup-v2",
            1,
            "Require content before work-management identifiers and expand opaque shorthand.",
        ),
        ch(
            3,
            "agent-team-timeline-technical-rollup-v3",
            1,
            "Use only supported semantic concepts; exclude mechanical glossary candidates.",
        ),
    ],
    lifecycle: "active",
};

/// Plain-language rollup contract.
pub static PLAIN_LANGUAGE_ROLLUP_SUMMARIZER: SummarizerSpec = SummarizerSpec {
    summarizer_id: "plain-language-rollup",
    summary_style: PLAIN_LANGUAGE_ROLLUP_STYLE,
    current_version: 4,
    prompt_version: "agent-team-timeline-plain-rollup-v4",
    output_schema_version: 1,
    description: "Newcomer-oriented rewrite grounded in the same-period technical summary.",
    input_fields: &[
        "same-period technical summary as the factual source of truth",
        "lower-level plain-language summaries or uncovered phase summaries",
        "up to ten prior same-level plain-language summaries",
        "project overview",
        "supported chronologically available glossary definitions",
        "aggregate activity statistics",
    ],
    output_fields: &["phrase", "paragraph", "work_summary"],
    granularities: &["hourly", "daily", "weekly", "monthly", "quarterly"],
    changelog: &[
        ch(
            1,
            "agent-team-timeline-plain-rollup-v1",
            1,
            "Initial separate plain-language calendar summary.",
        ),
        ch(
            2,
            "agent-team-timeline-plain-rollup-v2",
            1,
            "Ground newcomer explanations in the project overview and verified glossary.",
        ),
        ch(
            3,
            "agent-team-timeline-plain-rollup-v3",
            1,
            "Use only supported semantic concepts; exclude mechanical glossary candidates.",
        ),
        ch(
            4,
            "agent-team-timeline-plain-rollup-v4",
            1,
            "Ground status, outcomes, counts, and scope in the same-period technical summary.",
        ),
    ],
    lifecycle: "active",
};

/// Project overview contract.
pub static PROJECT_OVERVIEW_SUMMARIZER: SummarizerSpec = SummarizerSpec {
    summarizer_id: "project-overview",
    summary_style: PROJECT_OVERVIEW_STYLE,
    current_version: 2,
    prompt_version: "agent-team-timeline-project-overview-v2",
    output_schema_version: 1,
    description: "Durable newcomer overview from bounded early coordinator evidence.",
    input_fields: &["up to 48,000 characters of early root transcript"],
    output_fields: &["support_status", "paragraph"],
    granularities: &["project"],
    changelog: &[
        ch(
            1,
            "agent-team-timeline-project-overview-v1",
            1,
            "Initial project knowledge summary.",
        ),
        ch(
            2,
            "agent-team-timeline-project-overview-v2",
            1,
            "Freeze evidence frontiers and require explicit insufficient-evidence results.",
        ),
    ],
    lifecycle: "active",
};

/// Glossary definition contract (historical-disabled).
pub static GLOSSARY_DEFINITION_SUMMARIZER: SummarizerSpec = SummarizerSpec {
    summarizer_id: "glossary-definition",
    summary_style: GLOSSARY_DEFINITION_STYLE,
    current_version: 2,
    prompt_version: "agent-team-timeline-glossary-definition-v2",
    output_schema_version: 1,
    description: "Evidence-bounded newcomer definition for one deterministic glossary term.",
    input_fields: &[
        "exact glossary spelling",
        "up to six retained source occurrences",
        "frozen project overview",
    ],
    output_fields: &["support_status", "definition"],
    granularities: &["term"],
    changelog: &[
        ch(
            1,
            "agent-team-timeline-glossary-definition-v1",
            1,
            "Initial model-backed glossary definition.",
        ),
        ch(
            2,
            "agent-team-timeline-glossary-definition-v2",
            1,
            "Bind definitions to frozen occurrences and reject unsupported inference.",
        ),
    ],
    lifecycle: "historical-disabled",
};

/// Agent lifetime (hindsight naming) contract.
pub static AGENT_LIFETIME_SUMMARIZER: SummarizerSpec = SummarizerSpec {
    summarizer_id: "agent-lifetime",
    summary_style: AGENT_LIFETIME_STYLE,
    current_version: 2,
    prompt_version: "agent-team-timeline-agent-name-v2",
    output_schema_version: 2,
    description: "Hindsight short name and lifetime paragraph for one agent identity.",
    input_fields: &[
        "official path, coordinator nickname, role, depth, and parent path",
        "ancestor transcript scroll-back",
        "all available phase work summaries for the agent lifetime",
    ],
    output_fields: &["short_name", "rationale", "lifetime_summary"],
    granularities: &["agent-lifetime"],
    changelog: &[
        ch(
            1,
            "agent-team-timeline-agent-name-v1",
            1,
            "Initial hindsight short name.",
        ),
        ch(
            2,
            "agent-team-timeline-agent-name-v2",
            2,
            "Add the substantive lifetime summary used by agent hover cards.",
        ),
    ],
    lifecycle: "active",
};

/// All registered summarizers, in registry order.
pub fn summarizer_registry() -> Vec<&'static SummarizerSpec> {
    vec![
        &PHASE_SUMMARIZER,
        &AGENT_LIFETIME_SUMMARIZER,
        &PROJECT_OVERVIEW_SUMMARIZER,
        &GLOSSARY_DEFINITION_SUMMARIZER,
        &TECHNICAL_ROLLUP_SUMMARIZER,
        &PLAIN_LANGUAGE_ROLLUP_SUMMARIZER,
    ]
}

/// The registered contract for one summary style.
pub fn summarizer_for_style(style: &str) -> Result<&'static SummarizerSpec, String> {
    summarizer_registry()
        .into_iter()
        .find(|spec| spec.summary_style == style)
        .ok_or_else(|| {
            format!(
                "unregistered summary style {}",
                crate::json::py_repr_str(style)
            )
        })
}

/// The registered contract for one durable summarizer id.
pub fn summarizer_for_id(id: &str) -> Result<&'static SummarizerSpec, String> {
    summarizer_registry()
        .into_iter()
        .find(|spec| spec.summarizer_id == id)
        .ok_or_else(|| {
            format!(
                "unregistered summarizer ID {}",
                crate::json::py_repr_str(id)
            )
        })
}

/// Resolve a prompt identifier to its registered contract revision.
pub fn summarizer_change_for_prompt<'a>(
    spec: &'a SummarizerSpec,
    prompt_version: &str,
) -> Result<&'a SummarizerChange, String> {
    spec.changelog
        .iter()
        .find(|change| change.prompt_version == prompt_version)
        .ok_or_else(|| {
            format!(
                "summarizer {} has no prompt version {}",
                crate::json::py_repr_str(spec.summarizer_id),
                crate::json::py_repr_str(prompt_version)
            )
        })
}

/// The complete deterministic registry document.
pub fn registry_json_obj() -> Value {
    let mut out = serde_json::Map::new();
    out.insert("schema_version".into(), 2.into());
    out.insert(
        "summarizers".into(),
        Value::Array(
            summarizer_registry()
                .iter()
                .map(|spec| spec.to_json_obj())
                .collect(),
        ),
    );
    Value::Object(out)
}

/// Availability of one optional context channel at generation time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextComponent {
    /// Channel name.
    pub name: String,
    /// Requested amount.
    pub requested: i64,
    /// Provided amount.
    pub provided: i64,
    /// Unit.
    pub unit: String,
}

impl ContextComponent {
    /// Construct, validating the Python invariants.
    pub fn new(name: &str, requested: i64, provided: i64, unit: &str) -> Result<Self, String> {
        if name.is_empty() || unit.is_empty() {
            return Err("context component name and unit must not be empty".to_string());
        }
        if requested < 0 || provided < 0 {
            return Err("context component counts must not be negative".to_string());
        }
        if provided > requested {
            return Err("provided context must not exceed requested context".to_string());
        }
        Ok(Self {
            name: name.to_string(),
            requested,
            provided,
            unit: unit.to_string(),
        })
    }

    /// Coverage in basis points.
    pub fn coverage_basis_points(&self) -> i64 {
        if self.requested == 0 {
            10_000
        } else {
            (self.provided * 10_000) / self.requested
        }
    }

    /// Persisted form.
    pub fn to_json_obj(&self) -> Value {
        let mut out = serde_json::Map::new();
        out.insert("name".into(), Value::String(self.name.clone()));
        out.insert("requested".into(), self.requested.into());
        out.insert("provided".into(), self.provided.into());
        out.insert("unit".into(), Value::String(self.unit.clone()));
        out.insert(
            "coverage_basis_points".into(),
            self.coverage_basis_points().into(),
        );
        Value::Object(out)
    }

    /// Strictly parse one persisted context channel.
    pub fn from_json_obj(value: Option<&Value>, where_: &str) -> Result<Self, String> {
        let value = value.ok_or_else(|| format!("{where_}: expected an object"))?;
        let obj = as_object(value, where_)?;
        let expected = [
            "name",
            "requested",
            "provided",
            "unit",
            "coverage_basis_points",
        ];
        if obj.len() != expected.len() || !expected.iter().all(|key| obj.contains_key(*key)) {
            return Err(format!("{where_}: invalid context component fields"));
        }
        let result = Self::new(
            as_string(obj.get("name").unwrap(), &format!("{where_}.name"))?,
            as_int(
                obj.get("requested").unwrap(),
                &format!("{where_}.requested"),
            )?,
            as_int(obj.get("provided").unwrap(), &format!("{where_}.provided"))?,
            as_string(obj.get("unit").unwrap(), &format!("{where_}.unit"))?,
        )?;
        let recorded = as_int(
            obj.get("coverage_basis_points").unwrap(),
            &format!("{where_}.coverage_basis_points"),
        )?;
        if recorded != result.coverage_basis_points() {
            return Err(format!(
                "{where_}: context coverage does not match raw counts"
            ));
        }
        Ok(result)
    }
}

/// Comparable context completeness plus frontier provenance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextCoverage {
    /// Measured channels.
    pub components: Vec<ContextComponent>,
    /// Frontier status.
    pub frontier_status: String,
    /// Predecessor logical keys.
    pub predecessor_keys: Vec<String>,
    /// Whether measurements are known.
    pub known: bool,
}

impl Default for ContextCoverage {
    fn default() -> Self {
        Self {
            components: Vec::new(),
            frontier_status: "not-applicable".to_string(),
            predecessor_keys: Vec::new(),
            known: true,
        }
    }
}

impl ContextCoverage {
    /// Construct, validating the Python invariants.
    pub fn new(
        components: Vec<ContextComponent>,
        frontier_status: &str,
        predecessor_keys: Vec<String>,
        known: bool,
    ) -> Result<Self, String> {
        if !matches!(
            frontier_status,
            "not-applicable"
                | "project-start"
                | "contiguous-extension"
                | "isolated-backfill"
                | "unknown-legacy"
        ) {
            return Err(format!(
                "unsupported frontier status {}",
                crate::json::py_repr_str(frontier_status)
            ));
        }
        let mut names: Vec<&str> = components.iter().map(|item| item.name.as_str()).collect();
        let total = names.len();
        names.sort_unstable();
        names.dedup();
        if names.len() != total {
            return Err("context component names must be unique".to_string());
        }
        if !known && !components.is_empty() {
            return Err("unknown context coverage cannot contain measured components".to_string());
        }
        Ok(Self {
            components,
            frontier_status: frontier_status.to_string(),
            predecessor_keys,
            known,
        })
    }

    /// The legacy unknown-coverage value.
    pub fn unknown_legacy() -> Self {
        Self {
            components: Vec::new(),
            frontier_status: "unknown-legacy".to_string(),
            predecessor_keys: Vec::new(),
            known: false,
        }
    }

    /// Average coverage in basis points, if known.
    pub fn coverage_basis_points(&self) -> Option<i64> {
        if !self.known {
            return None;
        }
        if self.components.is_empty() {
            return Some(10_000);
        }
        Some(
            self.components
                .iter()
                .map(ContextComponent::coverage_basis_points)
                .sum::<i64>()
                / self.components.len() as i64,
        )
    }

    /// Whole-percent coverage.
    pub fn coverage_percent(&self) -> Option<i64> {
        self.coverage_basis_points()
            .map(|points| (points + 50) / 100)
    }

    /// Whole-percent missing.
    pub fn missing_percent(&self) -> Option<i64> {
        self.coverage_percent().map(|percent| 100 - percent)
    }

    /// Persisted form.
    pub fn to_json_obj(&self) -> Value {
        let opt = |value: Option<i64>| value.map_or(Value::Null, Into::into);
        let mut out = serde_json::Map::new();
        out.insert("known".into(), Value::Bool(self.known));
        out.insert(
            "coverage_basis_points".into(),
            opt(self.coverage_basis_points()),
        );
        out.insert("coverage_percent".into(), opt(self.coverage_percent()));
        out.insert("missing_percent".into(), opt(self.missing_percent()));
        out.insert(
            "frontier_status".into(),
            Value::String(self.frontier_status.clone()),
        );
        out.insert(
            "predecessor_keys".into(),
            Value::Array(
                self.predecessor_keys
                    .iter()
                    .map(|item| Value::String(item.clone()))
                    .collect(),
            ),
        );
        out.insert(
            "components".into(),
            Value::Array(
                self.components
                    .iter()
                    .map(ContextComponent::to_json_obj)
                    .collect(),
            ),
        );
        Value::Object(out)
    }

    /// Strictly parse persisted context coverage.
    pub fn from_json_obj(value: Option<&Value>, where_: &str) -> Result<Self, String> {
        let value = value.ok_or_else(|| format!("{where_}: expected an object"))?;
        let obj = as_object(value, where_)?;
        let expected = [
            "known",
            "coverage_basis_points",
            "coverage_percent",
            "missing_percent",
            "frontier_status",
            "predecessor_keys",
            "components",
        ];
        if obj.len() != expected.len() || !expected.iter().all(|key| obj.contains_key(*key)) {
            return Err(format!("{where_}: invalid context coverage fields"));
        }
        let known = match obj.get("known") {
            Some(Value::Bool(flag)) => *flag,
            _ => return Err(format!("{where_}.known: expected a boolean")),
        };
        let raw_components = as_array(
            obj.get("components").unwrap(),
            &format!("{where_}.components"),
        )?;
        let mut components = Vec::new();
        for (index, item) in raw_components.iter().enumerate() {
            components.push(ContextComponent::from_json_obj(
                Some(item),
                &format!("{where_}.components[{index}]"),
            )?);
        }
        let raw_predecessors = as_array(
            obj.get("predecessor_keys").unwrap(),
            &format!("{where_}.predecessor_keys"),
        )?;
        let mut predecessor_keys = Vec::new();
        for (index, item) in raw_predecessors.iter().enumerate() {
            predecessor_keys
                .push(as_string(item, &format!("{where_}.predecessor_keys[{index}]"))?.to_string());
        }
        let result = Self::new(
            components,
            as_string(
                obj.get("frontier_status").unwrap(),
                &format!("{where_}.frontier_status"),
            )?,
            predecessor_keys,
            known,
        )?;
        for (key, expected_value) in [
            ("coverage_basis_points", result.coverage_basis_points()),
            ("coverage_percent", result.coverage_percent()),
            ("missing_percent", result.missing_percent()),
        ] {
            let raw = obj.get(key).unwrap();
            match expected_value {
                None => {
                    if !raw.is_null() {
                        return Err(format!("{where_}.{key}: expected null"));
                    }
                }
                Some(expected) => {
                    if as_int(raw, &format!("{where_}.{key}"))? != expected {
                        return Err(format!("{where_}.{key}: derived value mismatch"));
                    }
                }
            }
        }
        Ok(result)
    }
}
