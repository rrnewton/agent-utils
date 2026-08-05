//! Caller-owned landing context bound to exact fetched head and base identities.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::model::{CiState, PolicyClass, PrNode, ValidationEvidence};

/// Label prefix carrying an assigned-agent identifier.
pub const AGENT_PREFIX: &str = "agent:";
/// Label prefix carrying a [`PolicyClass`].
pub const POLICY_PREFIX: &str = "landing-policy:";
/// Informational label indicating local validation without authoritative identity evidence.
pub const LOCALLY_VALIDATED_LABEL: &str = "locally-validated";

#[derive(Clone, Debug, Eq, PartialEq)]
/// Caller-owned facts for one PR, optionally guarded by fetched commit identities.
pub struct LandingContext {
    /// Positive PR number.
    pub pr: i64,
    /// Exact fetched head SHA to which the context applies.
    pub head_sha: String,
    /// Exact fetched base SHA to which validation evidence applies.
    pub base_sha: String,
    /// Optional operator or agent assignment.
    pub assigned_agent: String,
    /// Optional validation-evidence override.
    pub validation_evidence: Option<ValidationEvidence>,
    /// Optional policy classification override.
    pub policy_class: Option<PolicyClass>,
}

/// Parse and validate a `{"prs": [...]}` landing-context document.
pub fn parse_landing_context(raw: &Value) -> Result<Vec<LandingContext>, String> {
    let obj = raw
        .as_object()
        .ok_or("landing context must be an object with a 'prs' array")?;
    let prs = obj
        .get("prs")
        .and_then(Value::as_array)
        .ok_or("landing context field 'prs' must be an array")?;
    let mut seen = BTreeSet::new();
    let mut contexts = Vec::new();
    for item in prs {
        let item = item
            .as_object()
            .ok_or("each landing context PR entry must be an object")?;
        let pr = item.get("pr").and_then(Value::as_i64).unwrap_or(0);
        if pr <= 0 {
            return Err("landing context PR entry needs a positive integer 'pr'".to_owned());
        }
        if !seen.insert(pr) {
            return Err(format!("landing context contains duplicate PR #{pr}"));
        }
        let string = |key: &str| {
            item.get(key)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned()
        };
        let evidence_raw = string("validation_evidence");
        let evidence = if evidence_raw.is_empty() {
            None
        } else {
            Some(ValidationEvidence::parse(&evidence_raw).ok_or_else(|| {
                format!("PR #{pr} has unknown validation_evidence {evidence_raw:?}")
            })?)
        };
        let policy_raw = string("policy_class");
        let policy = if policy_raw.is_empty() {
            None
        } else {
            Some(
                PolicyClass::parse(&policy_raw)
                    .ok_or_else(|| format!("PR #{pr} has unknown policy_class {policy_raw:?}"))?,
            )
        };
        let head_sha = string("head_sha");
        let base_sha = string("base_sha");
        if evidence == Some(ValidationEvidence::CleanValidateRecord)
            && (head_sha.is_empty() || base_sha.is_empty())
        {
            return Err(format!(
                "PR #{pr} clean-validate-record evidence requires exact 'head_sha' and 'base_sha'; revalidate and record both fetched identities"
            ));
        }
        contexts.push(LandingContext {
            pr,
            head_sha,
            base_sha,
            assigned_agent: string("assigned_agent"),
            validation_evidence: evidence,
            policy_class: policy,
        });
    }
    Ok(contexts)
}

fn one_label_value(
    labels: &[String],
    prefix: &str,
    field: &str,
    pr: i64,
) -> Result<String, String> {
    let values: BTreeSet<_> = labels
        .iter()
        .filter_map(|label| label.strip_prefix(prefix))
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect();
    if values.len() > 1 {
        return Err(format!(
            "PR #{pr} has multiple {field} labels: {}",
            values.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    Ok(values.into_iter().next().unwrap_or_default())
}

fn apply_labels(mut node: PrNode) -> Result<PrNode, String> {
    node.assigned_agent = one_label_value(&node.labels, AGENT_PREFIX, "agent", node.number)?;
    let raw = one_label_value(&node.labels, POLICY_PREFIX, "landing-policy", node.number)?;
    node.policy_class = if raw.is_empty() {
        PolicyClass::Unclassified
    } else {
        PolicyClass::parse(&raw).ok_or_else(|| {
            format!(
                "PR #{} has unknown landing-policy label {raw:?}",
                node.number
            )
        })?
    };
    node.validation_evidence = if node.ci.raw_state == CiState::Passed && node.ci.gate_ok {
        ValidationEvidence::AuthoritativeCi
    } else if node
        .labels
        .iter()
        .any(|label| label == LOCALLY_VALIDATED_LABEL)
    {
        ValidationEvidence::LocallyValidated
    } else {
        ValidationEvidence::None
    };
    Ok(node)
}

/// Apply label-derived facts and caller context, rejecting unknown PRs or identity drift.
pub fn apply_landing_context(
    nodes: Vec<PrNode>,
    contexts: &[LandingContext],
) -> Result<Vec<PrNode>, String> {
    let by_context: BTreeMap<_, _> = contexts.iter().map(|ctx| (ctx.pr, ctx)).collect();
    let node_numbers: BTreeSet<_> = nodes.iter().map(|node| node.number).collect();
    let unknown: Vec<_> = by_context
        .keys()
        .filter(|number| !node_numbers.contains(number))
        .copied()
        .collect();
    if !unknown.is_empty() {
        return Err(format!(
            "landing context names PRs absent from this plan: {}",
            unknown
                .iter()
                .map(|number| format!("#{number}"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    nodes
        .into_iter()
        .map(|raw| {
            let mut node = apply_labels(raw)?;
            let Some(context) = by_context.get(&node.number) else {
                return Ok(node);
            };
            if !context.head_sha.is_empty() && context.head_sha != node.head_sha {
                return Err(format!(
                    "PR #{} landing context is stale: context={}, current={}",
                    node.number, context.head_sha, node.head_sha
                ));
            }
            if !context.base_sha.is_empty() && context.base_sha != node.base_sha {
                return Err(format!(
                    "PR #{} landing context base is stale: context={}, current={}; revalidate",
                    node.number, context.base_sha, node.base_sha
                ));
            }
            if !context.assigned_agent.is_empty() {
                node.assigned_agent.clone_from(&context.assigned_agent);
            }
            if let Some(evidence) = context.validation_evidence {
                node.validation_evidence = evidence;
            }
            if let Some(policy) = context.policy_class {
                node.policy_class = policy;
            }
            Ok(node)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(number: i64, head: &str, labels: &[&str]) -> PrNode {
        PrNode {
            number,
            head_sha: head.into(),
            base_sha: "base".into(),
            labels: labels.iter().map(|label| (*label).into()).collect(),
            ..PrNode::default()
        }
    }

    #[test]
    fn clean_records_are_exact_head_and_unknown_context_is_rejected() {
        let missing = json!({"prs":[{"pr":1,"validation_evidence":"clean-validate-record"}]});
        assert!(parse_landing_context(&missing)
            .unwrap_err()
            .contains("requires exact 'head_sha'"));
        let context = parse_landing_context(&json!({"prs":[{
            "pr":1,"head_sha":"stale","base_sha":"base",
            "validation_evidence":"clean-validate-record"
        }]}))
        .unwrap();
        assert!(
            apply_landing_context(vec![node(1, "current", &[])], &context)
                .unwrap_err()
                .contains("stale")
        );
        let unknown = parse_landing_context(&json!({"prs":[{"pr":9}]})).unwrap();
        assert!(
            apply_landing_context(vec![node(1, "current", &[])], &unknown)
                .unwrap_err()
                .contains("absent")
        );
    }

    #[test]
    fn labels_and_context_enrich_without_authorizing_local_hint() {
        let raw = node(
            1,
            "head",
            &[
                "locally-validated",
                "agent:one",
                "landing-policy:ci-hygiene",
            ],
        );
        let enriched = apply_landing_context(vec![raw], &[]).unwrap().remove(0);
        assert_eq!(enriched.assigned_agent, "one");
        assert_eq!(enriched.policy_class, PolicyClass::CiHygiene);
        assert_eq!(
            enriched.validation_evidence,
            ValidationEvidence::LocallyValidated
        );

        let duplicate = node(2, "head", &["agent:one", "agent:two"]);
        assert!(apply_landing_context(vec![duplicate], &[])
            .unwrap_err()
            .contains("multiple agent labels"));
    }
}
