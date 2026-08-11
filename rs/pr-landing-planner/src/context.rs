//! Caller-owned landing context bound to exact fetched head and base identities.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::model::{CiState, PolicyClass, PrNode, ValidationEvidence};

/// Label prefix carrying an assigned-agent identifier.
pub const AGENT_PREFIX: &str = "agent:";
/// Label prefix carrying a [`PolicyClass`].
pub const POLICY_PREFIX: &str = "landing-policy:";
/// Review lanes required by the adversarial-review protocol.
pub const REQUIRED_REVIEW_LANES: [&str; 2] = ["codex", "claude"];

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
    /// Caller-verified review lane to exact reviewed-head SHA receipts.
    pub review_pass_heads: BTreeMap<String, String>,
    /// Optional policy classification override.
    pub policy_class: Option<PolicyClass>,
}

fn is_exact_sha(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
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
        if matches!(
            evidence,
            Some(ValidationEvidence::LocallyValidated | ValidationEvidence::CleanValidateRecord)
        ) && (head_sha.is_empty() || base_sha.is_empty())
        {
            return Err(format!(
                "PR #{pr} {} evidence requires exact 'head_sha' and 'base_sha'; revalidate and record both fetched identities",
                evidence.expect("matched local evidence").as_str()
            ));
        }
        let mut review_pass_heads = BTreeMap::new();
        if let Some(raw_heads) = item.get("review_pass_heads") {
            let heads = raw_heads.as_object().ok_or_else(|| {
                format!("PR #{pr} review_pass_heads must be an object mapping lane to exact SHA")
            })?;
            for (lane, raw_sha) in heads {
                if !REQUIRED_REVIEW_LANES.contains(&lane.as_str()) {
                    return Err(format!("PR #{pr} has unknown review lane {lane:?}"));
                }
                let sha = raw_sha.as_str().unwrap_or("");
                if !is_exact_sha(sha) {
                    return Err(format!(
                        "PR #{pr} review_pass_heads[{lane:?}] must be an exact 40-character lowercase hex SHA"
                    ));
                }
                review_pass_heads.insert(lane.clone(), sha.to_owned());
            }
        }
        contexts.push(LandingContext {
            pr,
            head_sha,
            base_sha,
            assigned_agent: string("assigned_agent"),
            validation_evidence: evidence,
            review_pass_heads,
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
    } else {
        // A locally-validated label is only a cache hint. It deliberately maps to
        // None; only caller context bound to the fetched head and base may produce
        // LocallyValidated.
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
            node.review_pass_heads
                .clone_from(&context.review_pass_heads);
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
    use crate::graph::{held_reasons, review_binding};
    use crate::model::ReviewBinding;
    use serde_json::json;

    const REVIEWED_HEAD: &str = "92e1e0d0af65e50cd2991d4deaa25f726832fbf4";
    const REBASED_HEAD: &str = "0fc9f61edc01d6425def2efb0ed82f01410c7fcc";
    const CHANGED_HEAD: &str = "1111111111111111111111111111111111111111";

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
    fn bare_label_is_cache_only_but_dereferenced_context_is_evidence() {
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
        // Negative bracket: one bare cache label produces zero validation evidence.
        assert_eq!(enriched.validation_evidence, ValidationEvidence::None);

        let context = parse_landing_context(&json!({"prs":[{
            "pr":1,
            "head_sha":"head",
            "base_sha":"base",
            "validation_evidence":"locally-validated"
        }]}))
        .unwrap();
        let dereferenced = apply_landing_context(vec![node(1, "head", &[])], &context)
            .unwrap()
            .remove(0);
        // Positive bracket: one caller-dereferenced exact-identity record is accepted.
        assert_eq!(
            dereferenced.validation_evidence,
            ValidationEvidence::LocallyValidated
        );

        let missing_identity = json!({"prs":[{"pr":1,"validation_evidence":"locally-validated"}]});
        assert!(parse_landing_context(&missing_identity)
            .unwrap_err()
            .contains("locally-validated evidence requires"));

        let duplicate = node(2, "head", &["agent:one", "agent:two"]);
        assert!(apply_landing_context(vec![duplicate], &[])
            .unwrap_err()
            .contains("multiple agent labels"));
    }

    #[test]
    fn review_passes_bind_exact_head_and_head_moves_fail_closed() {
        let labels = [
            "post-facto-human-review",
            "passed-review-codex",
            "passed-review-claude",
        ];
        let exact = parse_landing_context(&json!({"prs":[{
            "pr":394,
            "review_pass_heads":{
                "codex":REBASED_HEAD,
                "claude":REBASED_HEAD
            }
        }]}))
        .unwrap();
        let exact_node = apply_landing_context(vec![node(394, REBASED_HEAD, &labels)], &exact)
            .unwrap()
            .remove(0);
        assert_eq!(
            review_binding(&exact_node),
            (ReviewBinding::ExactHead, vec![])
        );
        assert!(held_reasons(&[exact_node], &[]).is_empty());

        let stale = parse_landing_context(&json!({"prs":[{
            "pr":394,
            "review_pass_heads":{
                "codex":REBASED_HEAD,
                "claude":REVIEWED_HEAD
            }
        }]}))
        .unwrap();
        let stale_node = apply_landing_context(vec![node(394, REBASED_HEAD, &labels)], &stale)
            .unwrap()
            .remove(0);
        let (binding, reasons) = review_binding(&stale_node);
        assert_eq!(binding, ReviewBinding::Stale);
        assert_eq!(
            reasons,
            vec![format!(
                "review-pass-stale:claude:reviewed={REVIEWED_HEAD}:current={REBASED_HEAD}"
            )]
        );
        assert_eq!(held_reasons(&[stale_node], &[])[0].reasons, reasons);

        let changed_node = apply_landing_context(vec![node(394, CHANGED_HEAD, &labels)], &stale)
            .unwrap()
            .remove(0);
        assert_eq!(review_binding(&changed_node).0, ReviewBinding::Stale);
    }

    #[test]
    fn review_pass_labels_without_receipts_are_unbound_and_bad_receipts_refuse() {
        let labels = [
            "post-facto-human-review",
            "passed-review-codex",
            "passed-review-claude",
        ];
        let unbound = apply_landing_context(vec![node(394, REBASED_HEAD, &labels)], &[])
            .unwrap()
            .remove(0);
        assert_eq!(review_binding(&unbound).0, ReviewBinding::Unbound);

        let malformed = json!({"prs":[{
            "pr":394,
            "review_pass_heads":{"claude":"0fc9f61e"}
        }]});
        assert!(parse_landing_context(&malformed)
            .unwrap_err()
            .contains("exact 40-character lowercase hex SHA"));
        let unknown = json!({"prs":[{
            "pr":394,
            "review_pass_heads":{"other":REBASED_HEAD}
        }]});
        assert!(parse_landing_context(&unknown)
            .unwrap_err()
            .contains("unknown review lane"));
    }
}
