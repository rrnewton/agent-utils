//! The single qualifying predicate and per-commit verdict.
//!
//! Rows are [`PyValue`] objects. Every check is typed: a boolean is never
//! an integer, a float is never a count, and string comparisons are exact.
//! Deployment-specific values (accepted green classes, base-evidence field
//! names, the expected builder name, counted repositories) arrive through
//! [`Policy`] data, never as deployment literals in the clause logic.
//!
//! # Skipped clauses
//!
//! The authority predicate includes clauses that need I/O, an external
//! reader, or deployment tables this pure core does not own. They are not
//! evaluated here; each entry names the direction of divergence
//! ("looser" accepts what the authority can refuse, "stricter" refuses
//! what the authority can accept):
//!
//! - Admission verdict, admission floor-evidence validation, and the
//!   disjoint-main / queue-base proofs (git and subprocess I/O, plus
//!   deployment policy strings): skipped, looser.
//! - Release payload-table identity and the pairless-builder cutoff,
//!   schema, and repository rules: approximated by checking a string
//!   builder field (and build-mode data) against
//!   [`Policy::expected_builder`] when present and accepting an absent
//!   builder field, looser.
//! - Known-gap ledger exception to `passed == executed` (hash-verified
//!   diagnostics resolved through git): skipped, stricter, because a
//!   count shortfall always refuses here.
//! - Producer epoch and known-producer enforcement: skipped, looser once
//!   a deployment declares an epoch; inactive while no epoch is set.
//! - Cell-results verdict and the full-validation completeness gate
//!   (deployment field names and artifact checks): skipped, looser.
//!   Schema-10 native evidence is likewise skipped, looser: the
//!   authority reads it from an external checker over the evidence
//!   archive, never from a row field, so no in-row presence check can
//!   stand in for it.
//! - Base-boundary currency (git containment and lag) beside the base
//!   fields: skipped, looser; only field presence and shape are checked.
//! - Canonical typed-completeness beyond the row predicate (per-gate
//!   clean passes, exit code, tree and host presence, count identities):
//!   outside this predicate, looser relative to that layer.
//! - Duplicate-key refusal inside an admission claim: rows here are
//!   dict views, so duplicate written pairs are not re-examined by this
//!   predicate, looser for claim-carrying rows.
//! - Legacy coverage-claim classification below schema 5, and cell
//!   schema classification for unsupported schemas 6 to 9: not
//!   distinguished, looser.
//! - In [`disposition`], typed per-gate failure classes and the
//!   future-schema needs-rerun guard: treated as untyped, stricter,
//!   because such reds can latch as failed here where the authority
//!   would withhold them.
//! - In [`assess`], composition records and the green-flaky supersede
//!   escape from the genuine-failure latch: not implemented, stricter.
//!
//! One clause is implemented rather than skipped, as an intended oracle
//! parity rule: the `legacy` clause (see [`legacy_applies`]). A passing
//! row with no cell-results evidence and no full-coverage claim is a
//! legacy count-based row; the authority qualifies it by its counts
//! (its cell-evidence verdict for such a row is a non-blocking
//! grandfathered unknown), so this predicate judges it by the same
//! counts and skips the coverage and base-evidence clauses for that
//! row only. Rows carrying cell evidence keep the strict path,
//! unchanged; the clause neither loosens nor tightens any other row.

use crate::event::is_lower_hex40;
use crate::pyjson::PyValue;
use std::collections::BTreeSet;

/// Deployment policy for the predicate. Every deployment-specific value
/// the clauses consult is data here, so one binary serves any ledger.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    /// Green-class names a derived class must be one of to qualify.
    pub accepted_green_classes: Vec<String>,
    /// Inclusive upper bound for a row's typed `failures` count.
    pub failures_max: u64,
    /// Minimum schema version at which the count-capable contract
    /// (declared gate accounting and executed-count proof) applies.
    pub counts_schema: i64,
    /// Whether a nonzero `filtered_tests` count refuses a row.
    pub gate_filtered_tests: bool,
    /// Row field names that must each carry a 40-lowercase-hex string
    /// from schema 5 on. Deployments may extend this list.
    pub base_fields: Vec<String>,
    /// Builder name a string builder field must equal when present.
    pub expected_builder: String,
    /// Repositories whose rows [`assess`] counts. `None` counts every
    /// repository; a row with an absent repository always counts.
    pub counted_repos: Option<BTreeSet<String>>,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            accepted_green_classes: vec!["hard".to_owned()],
            failures_max: 0,
            counts_schema: 5,
            gate_filtered_tests: false,
            base_fields: vec!["base_sha".to_owned(), "base_tree".to_owned()],
            expected_builder: "cargo".to_owned(),
            counted_repos: None,
        }
    }
}

/// Per-commit verdict from [`assess`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// At least one qualifying row and no genuine failure on record.
    Qualified,
    /// Rows exist for the commit but none qualifies, or a genuine
    /// failure latches the commit.
    NotQualified,
    /// No counted row exists for the commit at all.
    NoResult,
}

/// A [`Verdict`] with a stable short reason string.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Assessment {
    /// The verdict.
    pub verdict: Verdict,
    /// Stable short reason for the verdict.
    pub reason: String,
}

impl Assessment {
    /// Whether the verdict is [`Verdict::Qualified`].
    pub fn qualified(&self) -> bool {
        matches!(self.verdict, Verdict::Qualified)
    }
}

/// Fallback gate expectation for a full run without a declared count.
const FULL_GATES_FALLBACK: i64 = 5;

/// First schema whose gate expectation must be declared, not inferred.
const DECLARE_REQUIRED_SCHEMA: i64 = 5;

/// First schema held to the per-node coverage verdict.
const COVERAGE_SCHEMA_MIN: i64 = 5;

/// First schema held to carrying base-evidence fields.
const BASE_SCHEMA_MIN: i64 = 5;

fn refuse(reason: &str) -> (bool, String) {
    (false, reason.to_owned())
}

/// Dict-view lookup with JSON `null` treated as absent, matching the
/// typed-row view where a null count field is no count field.
fn present<'a>(row: &'a PyValue, key: &str) -> Option<&'a PyValue> {
    match row.get(key) {
        None | Some(PyValue::Null) => None,
        Some(value) => Some(value),
    }
}

fn str_of<'a>(row: &'a PyValue, key: &str) -> Option<&'a str> {
    row.get(key).and_then(PyValue::as_str)
}

/// Typed schema version: absent or null is 0, a non-integer or a
/// negative integer is invalid (`None` here, a refusal to callers).
fn schema_of(row: &PyValue) -> Option<i64> {
    match row.get("schema_version") {
        None | Some(PyValue::Null) => Some(0),
        Some(value) => {
            let parsed = value.as_i64()?;
            if parsed < 0 {
                None
            } else {
                Some(parsed)
            }
        }
    }
}

/// Whether an integer value is at least one (arbitrary precision,
/// never a boolean or float).
fn int_at_least_one(value: Option<&PyValue>) -> bool {
    match value {
        Some(PyValue::Int(text)) => !text.starts_with('-') && text != "0",
        _ => false,
    }
}

fn executed_ok(row: &PyValue) -> bool {
    int_at_least_one(present(row, "executed_tests"))
}

fn as_f64(value: &PyValue) -> Option<f64> {
    match value {
        PyValue::Float(number) => Some(*number),
        PyValue::Int(text) => text.parse::<f64>().ok(),
        _ => None,
    }
}

fn is_reconstructed(row: &PyValue) -> bool {
    str_of(row, "source") == Some("reconstructed")
}

/// Generic release-builder identity, approximated as recorded in the
/// module documentation: a string builder field must equal the policy
/// builder, build-mode data must name that same builder, and absent
/// fields accept.
fn builder_ok(row: &PyValue, policy: &Policy) -> bool {
    if let Some(value) = row.get("release_builder") {
        match value {
            PyValue::Str(name) if *name == policy.expected_builder => {}
            _ => return false,
        }
    }
    if let Some(value) = row.get("build_mode") {
        match value {
            PyValue::Str(name) if *name == policy.expected_builder => {}
            _ => return false,
        }
    }
    if let Some(value) = row.get("prepared_cohort_modes") {
        match value {
            PyValue::Object(pairs)
                if pairs.iter().all(|(_, mode)| {
                    matches!(mode, PyValue::Str(name) if *name == policy.expected_builder)
                }) => {}
            _ => return false,
        }
    }
    true
}

struct GateCounts {
    ran: Option<i64>,
    expected: Option<i64>,
    declared: bool,
    malformed: bool,
}

/// Gate ran/expected counts: `gates_run` falling back to `checks`, and
/// `gates_expected` falling back to the inferred full-run expectation
/// only for a full, non-reconstructed run at schema 3 or later.
fn gate_counts(row: &PyValue, schema: i64) -> GateCounts {
    let mut malformed = false;
    let ran = match present(row, "gates_run").or_else(|| present(row, "checks")) {
        Some(value) => match value.as_i64() {
            Some(number) => Some(number),
            None => {
                malformed = true;
                None
            }
        },
        None => None,
    };
    let (expected, declared) = match present(row, "gates_expected") {
        Some(value) => match value.as_i64() {
            Some(number) => (Some(number), true),
            None => {
                malformed = true;
                (None, false)
            }
        },
        None => {
            if str_of(row, "profile") == Some("full") && schema >= 3 && !is_reconstructed(row) {
                (Some(FULL_GATES_FALLBACK), false)
            } else {
                (None, false)
            }
        }
    };
    GateCounts {
        ran,
        expected,
        declared,
        malformed,
    }
}

/// Validated intentional-skip names. An absent field means no skips.
/// Each entry must be an object with a non-blank unique `name` and the
/// single accepted reason; anything else refuses the accounting.
fn intentional_skips(row: &PyValue) -> Result<Vec<String>, String> {
    let items = match row.get("intentional_skipped_nodes") {
        None => return Ok(Vec::new()),
        Some(PyValue::Array(items)) => items,
        Some(_) => return Err("intentional-skips-not-a-list".to_owned()),
    };
    let mut names: Vec<String> = Vec::new();
    for entry in items {
        if !entry.is_object() {
            return Err("intentional-skip-not-an-object".to_owned());
        }
        let name = match entry.get("name").and_then(PyValue::as_str) {
            Some(text) if !text.trim().is_empty() => text.to_owned(),
            _ => return Err("intentional-skip-unnamed".to_owned()),
        };
        if entry.get("reason").and_then(PyValue::as_str) != Some("empty-manifest-bucket") {
            return Err("intentional-skip-reason".to_owned());
        }
        if names.contains(&name) {
            return Err("intentional-skip-duplicate".to_owned());
        }
        names.push(name);
    }
    Ok(names)
}

fn explicit_empty_list(row: &PyValue, key: &str) -> bool {
    matches!(row.get(key), Some(PyValue::Array(items)) if items.is_empty())
}

/// Green-side gate completeness. An inferred expectation never proves
/// completeness once declaration is required, typed intentional skips
/// account without passing, and an over-run without skips is complete.
fn gates_complete(row: &PyValue, schema: i64) -> (bool, String) {
    let counts = gate_counts(row, schema);
    if counts.malformed {
        return refuse("gate-counts-not-int");
    }
    let Some(ran) = counts.ran else {
        return refuse("gates-run-missing");
    };
    let Some(expected) = counts.expected else {
        return refuse("gate-contract-absent");
    };
    if expected <= 0 {
        return refuse("gate-expected-not-positive");
    }
    if !counts.declared && schema >= DECLARE_REQUIRED_SCHEMA {
        return refuse("gate-expected-inferred");
    }
    let skips = match intentional_skips(row) {
        Ok(names) => names,
        Err(reason) => return (false, reason),
    };
    if !skips.is_empty() {
        if !explicit_empty_list(row, "dependency_skipped_nodes")
            || !explicit_empty_list(row, "unaccounted_nodes")
        {
            return refuse("skip-accounting-incomplete");
        }
        if row.get("skipped_nodes").and_then(PyValue::as_i64) != Some(skips.len() as i64) {
            return refuse("skipped-nodes-mismatch");
        }
        if ran + skips.len() as i64 != expected {
            return refuse("gates-short-run");
        }
        return (true, "gates-complete".to_owned());
    }
    if ran < expected {
        return refuse("gates-short-run");
    }
    (true, "gates-complete".to_owned())
}

/// Per-node coverage verdict. `None` means satisfied: a positive typed
/// planned count, a typed executed count when the key is present, and
/// both shortfall lists present, string-typed, and empty. An omitted
/// list is unknown, never empty-success.
fn coverage_refusal(row: &PyValue) -> Option<&'static str> {
    let coverage = match row.get("coverage") {
        Some(value) if value.is_object() => value,
        _ => return Some("coverage-missing"),
    };
    match coverage.get("planned_test_nodes").and_then(PyValue::as_u64) {
        Some(planned) if planned > 0 => {}
        _ => return Some("coverage-unavailable"),
    }
    if coverage.contains_key("executed_test_nodes")
        && coverage
            .get("executed_test_nodes")
            .and_then(PyValue::as_u64)
            .is_none()
    {
        return Some("coverage-unavailable");
    }
    for field in ["zero_executed_nodes", "absent_nodes"] {
        match coverage.get(field) {
            Some(PyValue::Array(items))
                if items.iter().all(|item| matches!(item, PyValue::Str(_))) =>
            {
                if !items.is_empty() {
                    return Some("coverage-unsatisfied");
                }
            }
            _ => return Some("coverage-unavailable"),
        }
    }
    None
}

/// Derived green class, or `None` for a refused derivation. The class
/// is derived from provenance fields only; a present `green_class`
/// label is a cache that must agree, never authority, and an explicit
/// null provenance field refuses where absence would default.
fn derive_green_class(row: &PyValue) -> Option<String> {
    let commit = str_of(row, "commit")?;
    if commit.is_empty() || commit == "unknown" {
        return None;
    }
    for field in ["validated_head_sha", "inherited_from", "green_class"] {
        if matches!(row.get(field), Some(PyValue::Null)) {
            return None;
        }
    }
    let validated = match row.get("validated_head_sha") {
        None => commit,
        Some(PyValue::Str(text)) => text.as_str(),
        Some(_) => return None,
    };
    let derived = if validated == commit {
        if row.contains_key("inherited_from") {
            return None;
        }
        "hard".to_owned()
    } else {
        let inherited = row.get("inherited_from")?;
        if !inherited.is_object() {
            return None;
        }
        let delta_kind = str_of(inherited, "delta_kind")?;
        if !matches!(
            delta_kind,
            "rebase-only" | "rebase-plus-upstream" | "new-branch-commits"
        ) {
            return None;
        }
        let branch_commits = inherited.get("branch_commits").and_then(PyValue::as_u64)?;
        let force_full_paths: Vec<&str> = match inherited.get("force_full_paths") {
            None => Vec::new(),
            Some(PyValue::Array(items)) => {
                let mut paths = Vec::new();
                for item in items {
                    paths.push(item.as_str()?);
                }
                paths
            }
            Some(_) => return None,
        };
        let upstream_commits = inherited
            .get("upstream_commits")
            .and_then(PyValue::as_u64)?;
        if delta_kind == "new-branch-commits" || branch_commits > 0 {
            "not-green".to_owned()
        } else if delta_kind == "rebase-only" {
            if upstream_commits != 0 {
                return None;
            }
            "soft-rebase-only".to_owned()
        } else {
            if upstream_commits == 0 {
                return None;
            }
            if force_full_paths.is_empty() {
                "soft-upstream-delta".to_owned()
            } else {
                "soft-force-full-touched".to_owned()
            }
        }
    };
    if let Some(label) = row.get("green_class") {
        if label.as_str() != Some(derived.as_str()) {
            return None;
        }
    }
    Some(derived)
}

/// Whether the `legacy` clause applies to `row`: a passing row with
/// no cell-results evidence (`cell_results` absent or null) and no
/// full-coverage claim (`full_coverage` absent or null). Such a row is
/// a legacy count-based row, qualified by its counts; see the module
/// documentation. An explicit `full_coverage` value (including `false`)
/// is a claim and defeats the clause, as does any carried cell
/// evidence.
fn legacy_applies(row: &PyValue) -> bool {
    str_of(row, "result") == Some("pass")
        && present(row, "cell_results").is_none()
        && present(row, "full_coverage").is_none()
}

/// Whether one row qualifies as a full green for exact `sha`, with a
/// stable short reason. Clauses run in authority order and the first
/// refusal names the reason; see the module documentation for the
/// clauses this pure predicate skips.
pub fn row_qualifies(row: &PyValue, sha: &str, policy: &Policy) -> (bool, String) {
    match row.get("commit") {
        None | Some(PyValue::Null) => return refuse("no-commit"),
        Some(PyValue::Str(text)) if text.is_empty() || text == "unknown" => {
            return refuse("no-commit");
        }
        Some(PyValue::Str(text)) if text == sha => {}
        Some(_) => return refuse("commit-mismatch"),
    }
    if !matches!(row.get("commit_anchored"), Some(PyValue::Bool(true))) {
        return refuse("commit-not-anchored");
    }
    if !matches!(row.get("tree_dirty"), Some(PyValue::Bool(false))) {
        return refuse("tree-dirty");
    }
    if str_of(row, "profile") != Some("full") {
        return refuse("profile-not-full");
    }
    if str_of(row, "selection_mode") != Some("full") {
        return refuse("selection-not-full");
    }
    if !builder_ok(row, policy) {
        return refuse("release-builder-mismatch");
    }
    if str_of(row, "result") != Some("pass") {
        return refuse("result-not-pass");
    }
    let failures = match row.get("failures") {
        None | Some(PyValue::Null) => 0,
        Some(value) => match value.as_u64() {
            Some(count) => count,
            None => return refuse("failures-not-u64"),
        },
    };
    if failures > policy.failures_max {
        return refuse("failures-present");
    }
    if matches!(present(row, "executed_tests"), Some(value) if value.as_i64() == Some(0)) {
        return refuse("zero-executed-tests");
    }
    let Some(schema) = schema_of(row) else {
        return refuse("schema-version-invalid");
    };
    if schema >= 4 {
        for field in ["executed_tests", "passed_tests", "filtered_tests"] {
            if let Some(value) = present(row, field) {
                if value.as_i64().is_none() {
                    return refuse(if field == "filtered_tests" {
                        "filtered-tests-not-i64"
                    } else {
                        "count-not-i64"
                    });
                }
            }
        }
    }
    let executed_i64 = present(row, "executed_tests").and_then(PyValue::as_i64);
    let passed_i64 = present(row, "passed_tests").and_then(PyValue::as_i64);
    let passed_matches = executed_i64.is_some() && executed_i64 == passed_i64;
    if schema >= 4 && !passed_matches {
        return refuse("passed-not-equal-executed");
    }
    if schema < 4 && present(row, "passed_tests").is_some() && !passed_matches {
        return refuse("passed-not-equal-executed");
    }
    if policy.gate_filtered_tests
        && present(row, "filtered_tests").and_then(PyValue::as_i64) != Some(0)
    {
        return refuse("filtered-tests-present");
    }
    if schema >= policy.counts_schema {
        let (complete, reason) = gates_complete(row, schema);
        if !complete {
            return (false, reason);
        }
    }
    let legacy = legacy_applies(row);
    let count_capable = schema >= policy.counts_schema;
    let executed_present = present(row, "executed_tests").is_some();
    if count_capable || executed_present {
        // Count-capable rows and old-schema rows carrying counts must
        // prove nonzero execution; an old-schema row carrying only an
        // executed count proves it the same way. A row with neither
        // count cannot prove nonzero execution at all. (A passed-only
        // row already refused above as a passed/executed mismatch.)
        if !executed_ok(row) {
            return refuse("executed-tests-missing-or-zero");
        }
    } else {
        return refuse("no-execution-count");
    }
    if legacy && !count_capable {
        // The legacy count proof, exactly as the authority states it:
        // an old-schema row qualifies by counts only when it carries
        // both counts and proves nonzero execution by them.
        let counts_present = executed_present && present(row, "filtered_tests").is_some();
        if !counts_present {
            return refuse("no-execution-count");
        }
    }
    if !legacy && count_capable && schema >= COVERAGE_SCHEMA_MIN {
        if let Some(reason) = coverage_refusal(row) {
            return refuse(reason);
        }
    }
    if !legacy && schema >= BASE_SCHEMA_MIN {
        for field in &policy.base_fields {
            match str_of(row, field) {
                Some(text) if is_lower_hex40(text) => {}
                _ => return refuse("base-evidence-missing-or-invalid"),
            }
        }
    }
    if schema > 10 {
        return refuse("schema-unsupported");
    }
    let Some(green_class) = derive_green_class(row) else {
        return refuse("green-class-refused");
    };
    if !policy
        .accepted_green_classes
        .iter()
        .any(|accepted| accepted == &green_class)
    {
        return refuse("green-class-not-accepted");
    }
    (true, "qualifies".to_owned())
}

/// How one row bears on a commit's verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    /// The row qualifies as a full green for the commit.
    Qualifying,
    /// The row is a genuine clean-full failure that latches the commit.
    Failed,
    /// The row records no verdict-bearing outcome (a non-verdict
    /// result, a zero-test purported pass, an environment fault, an
    /// under-accounted run, or a red with no named failure).
    Withheld,
    /// The row does not bear on this commit's verdict.
    Ignored,
}

fn is_clean_full(row: &PyValue, sha: &str) -> bool {
    str_of(row, "commit") == Some(sha)
        && matches!(row.get("commit_anchored"), Some(PyValue::Bool(true)))
        && matches!(row.get("tree_dirty"), Some(PyValue::Bool(false)))
        && str_of(row, "profile") == Some("full")
        && str_of(row, "selection_mode") == Some("full")
}

fn gates_list(row: &PyValue) -> Vec<&PyValue> {
    match row.get("gates") {
        Some(PyValue::Array(items)) => items.iter().collect(),
        _ => Vec::new(),
    }
}

/// A red gate: not aborted, with its result (or kind, when no truthy
/// result is present) in the failure vocabulary.
fn gate_is_red(gate: &PyValue) -> bool {
    if matches!(gate.get("aborted"), Some(PyValue::Bool(true))) {
        return false;
    }
    let text = match gate.get("result") {
        Some(value) if value.truthy() => value.as_str().unwrap_or(""),
        _ => gate.get("kind").and_then(PyValue::as_str).unwrap_or(""),
    };
    matches!(text, "fail" | "failed" | "timeout")
}

/// Node-list length, with an absent or null list counting as empty.
/// `None` means the field is present but not a string list.
fn node_list_len(row: &PyValue, field: &str) -> Option<i64> {
    match row.get(field) {
        None | Some(PyValue::Null) => Some(0),
        Some(PyValue::Array(items)) if items.iter().all(|item| matches!(item, PyValue::Str(_))) => {
            Some(items.len() as i64)
        }
        Some(_) => None,
    }
}

/// Red-side gate accounting, deliberately different from the green
/// side: dependency skips earn credit beside intentional ones, and the
/// legacy inferred expectation still accounts for undeclared runs.
fn red_gate_accounting_complete(row: &PyValue, schema: i64, has_failed_gates: bool) -> bool {
    let counts = gate_counts(row, schema);
    if counts.malformed {
        return false;
    }
    let (Some(ran), Some(expected)) = (counts.ran, counts.expected) else {
        return false;
    };
    if expected <= 0 {
        return false;
    }
    if !counts.declared {
        return ran >= expected;
    }
    if !has_failed_gates {
        return gates_complete(row, schema).0;
    }
    let intentional = match intentional_skips(row) {
        Ok(names) => names.len() as i64,
        Err(_) => return false,
    };
    let Some(dependency) = node_list_len(row, "dependency_skipped_nodes") else {
        return false;
    };
    if node_list_len(row, "unaccounted_nodes") != Some(0) {
        return false;
    }
    let skipped = intentional + dependency;
    if skipped > 0 {
        ran + skipped == expected
    } else {
        ran >= expected
    }
}

/// Untyped environment-fault fallback: an all-127 command-not-found
/// storm with positive elapsed time, or a sub-second collapse in
/// which every gate is red.
fn env_fault(row: &PyValue, gates: &[&PyValue]) -> bool {
    let red: Vec<&PyValue> = gates
        .iter()
        .copied()
        .filter(|gate| gate_is_red(gate))
        .collect();
    let real_seconds = present(row, "real_seconds").and_then(as_f64);
    let storm = !red.is_empty()
        && red
            .iter()
            .all(|gate| gate.get("exit_code").and_then(PyValue::as_i64) == Some(127))
        && matches!(real_seconds, Some(seconds) if seconds > 0.0);
    let collapse = !gates.is_empty()
        && gates.iter().copied().all(gate_is_red)
        && matches!(real_seconds, Some(seconds) if seconds <= 1.0);
    storm || collapse
}

/// Classify one row for `sha` under `policy`.
///
/// Non-verdict results are read first so they stay observable even
/// though they can never pass, then a zero-test purported pass, then
/// the genuine-failure test. Typed per-gate failure classes are
/// treated as untyped (see the module documentation), so the
/// environment-fault fallback is the only typed-red escape here.
pub fn disposition(row: &PyValue, sha: &str, policy: &Policy) -> Disposition {
    let result = str_of(row, "result");
    if matches!(
        result,
        Some("truncated" | "needs-rerun" | "needs_rerun" | "no-result" | "no_result")
    ) {
        return if str_of(row, "commit") == Some(sha) {
            Disposition::Withheld
        } else {
            Disposition::Ignored
        };
    }
    if row_qualifies(row, sha, policy).0 {
        return Disposition::Qualifying;
    }
    let clean_full = is_clean_full(row, sha);
    if clean_full
        && result == Some("pass")
        && matches!(present(row, "executed_tests"), Some(value) if value.as_i64() == Some(0))
    {
        return Disposition::Withheld;
    }
    if !clean_full || !matches!(result, Some("fail" | "failed" | "timeout")) {
        return Disposition::Ignored;
    }
    let gates = gates_list(row);
    let has_failed_gates = gates.iter().copied().any(gate_is_red);
    if env_fault(row, &gates) {
        return Disposition::Withheld;
    }
    let schema = schema_of(row).unwrap_or(0);
    if !red_gate_accounting_complete(row, schema, has_failed_gates) {
        return Disposition::Withheld;
    }
    let has_real_failure = int_at_least_one(present(row, "failures")) || has_failed_gates;
    if !has_real_failure {
        return Disposition::Withheld;
    }
    Disposition::Failed
}

fn counted(row: &PyValue, policy: &Policy) -> bool {
    match &policy.counted_repos {
        None => true,
        Some(repos) => match row.get("repo") {
            None | Some(PyValue::Null) => true,
            Some(PyValue::Str(name)) => repos.contains(name),
            Some(_) => false,
        },
    }
}

/// Assess every counted row for exact `sha` under `policy`.
///
/// A genuine failure latches: any [`Disposition::Failed`] row makes
/// the commit not qualified however many qualifying rows stand beside
/// it, and no timing comparison washes a latch away. The authority's
/// composition and green-flaky supersede escapes from the latch are
/// not implemented here (stricter; see the module documentation).
pub fn assess(rows: &[PyValue], sha: &str, policy: &Policy) -> Assessment {
    let mut any_row = false;
    let mut any_failed = false;
    let mut any_qualifying = false;
    for row in rows {
        if !counted(row, policy) {
            continue;
        }
        if str_of(row, "commit") != Some(sha) {
            continue;
        }
        any_row = true;
        match disposition(row, sha, policy) {
            Disposition::Failed => any_failed = true,
            Disposition::Qualifying => any_qualifying = true,
            Disposition::Withheld | Disposition::Ignored => {}
        }
    }
    if any_failed {
        Assessment {
            verdict: Verdict::NotQualified,
            reason: "genuine-failure-on-record".to_owned(),
        }
    } else if any_qualifying {
        Assessment {
            verdict: Verdict::Qualified,
            reason: "qualifies".to_owned(),
        }
    } else if !any_row {
        Assessment {
            verdict: Verdict::NoResult,
            reason: "no-result".to_owned(),
        }
    } else {
        Assessment {
            verdict: Verdict::NotQualified,
            reason: "no-qualifying-row".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

    fn s(text: &str) -> PyValue {
        PyValue::Str(text.to_owned())
    }

    fn int(number: i64) -> PyValue {
        PyValue::Int(number.to_string())
    }

    fn hex40(digit: &str) -> PyValue {
        s(&digit.repeat(40))
    }

    /// One synthetic schema-5 row satisfying every implemented clause.
    fn happy() -> PyValue {
        PyValue::Object(vec![
            ("commit".to_owned(), s(SHA)),
            ("commit_anchored".to_owned(), PyValue::Bool(true)),
            ("tree_dirty".to_owned(), PyValue::Bool(false)),
            ("profile".to_owned(), s("full")),
            ("selection_mode".to_owned(), s("full")),
            ("result".to_owned(), s("pass")),
            ("failures".to_owned(), int(0)),
            ("executed_tests".to_owned(), int(10)),
            ("passed_tests".to_owned(), int(10)),
            ("filtered_tests".to_owned(), int(0)),
            ("schema_version".to_owned(), int(5)),
            ("gates_run".to_owned(), int(5)),
            ("gates_expected".to_owned(), int(5)),
            ("finished_at".to_owned(), s("2030-01-01T00:00:00Z")),
            ("base_sha".to_owned(), hex40("b")),
            ("base_tree".to_owned(), hex40("c")),
            (
                "coverage".to_owned(),
                PyValue::Object(vec![
                    ("planned_test_nodes".to_owned(), int(10)),
                    ("executed_test_nodes".to_owned(), int(10)),
                    ("zero_executed_nodes".to_owned(), PyValue::Array(vec![])),
                    ("absent_nodes".to_owned(), PyValue::Array(vec![])),
                ]),
            ),
        ])
    }

    fn set(row: &PyValue, key: &str, value: PyValue) -> PyValue {
        let mut pairs = match row {
            PyValue::Object(pairs) => pairs.clone(),
            _ => Vec::new(),
        };
        pairs.retain(|(name, _)| name != key);
        pairs.push((key.to_owned(), value));
        PyValue::Object(pairs)
    }

    fn remove(row: &PyValue, key: &str) -> PyValue {
        let mut pairs = match row {
            PyValue::Object(pairs) => pairs.clone(),
            _ => Vec::new(),
        };
        pairs.retain(|(name, _)| name != key);
        PyValue::Object(pairs)
    }

    fn qualifies(row: &PyValue) -> bool {
        row_qualifies(row, SHA, &Policy::default()).0
    }

    /// A clean-full red row with one red gate at the given exit code.
    fn red(result: &str, exit_code: i64, real_seconds: f64) -> PyValue {
        let gate = PyValue::Object(vec![
            ("name".to_owned(), s("gate-a")),
            ("result".to_owned(), s(result)),
            ("exit_code".to_owned(), int(exit_code)),
        ]);
        let row = set(&happy(), "result", s(result));
        let row = set(&row, "failures", int(1));
        let row = set(&row, "real_seconds", PyValue::Float(real_seconds));
        set(&row, "gates", PyValue::Array(vec![gate]))
    }

    #[test]
    fn happy_row_qualifies() {
        let (ok, reason) = row_qualifies(&happy(), SHA, &Policy::default());
        assert!(ok, "reason: {reason}");
        assert_eq!(reason, "qualifies");
        let outcome = assess(&[happy()], SHA, &Policy::default());
        assert_eq!(outcome.verdict, Verdict::Qualified);
        assert!(outcome.qualified());
    }

    #[test]
    fn schema10_row_qualifies_without_in_row_native_evidence() {
        // Schema-10 native evidence lives with the authority's external
        // checker, never in a row field; its absence must not refuse.
        let row = set(&happy(), "schema_version", int(10));
        let (ok, reason) = row_qualifies(&row, SHA, &Policy::default());
        assert!(ok, "reason: {reason}");
    }

    /// One legacy count-based row: passing, with declared gate counts
    /// and test counts, but no cell evidence, no coverage object, no
    /// base-evidence fields, and no full-coverage claim.
    fn legacy_row(schema: i64, executed: i64, passed: i64, filtered: i64) -> PyValue {
        let row = set(&happy(), "schema_version", int(schema));
        let row = set(&row, "executed_tests", int(executed));
        let row = set(&row, "passed_tests", int(passed));
        let row = set(&row, "filtered_tests", int(filtered));
        let row = remove(&row, "coverage");
        let row = remove(&row, "base_sha");
        remove(&row, "base_tree")
    }

    #[test]
    fn legacy_count_rows_qualify_by_counts() {
        // Several count shapes, mirroring the legacy population: small
        // and large executed counts, filtered tests present, and an
        // old-schema row that predates declared gate contracts.
        assert!(qualifies(&legacy_row(5, 10, 10, 0)));
        assert!(qualifies(&legacy_row(5, 740, 740, 350)));
        assert!(qualifies(&legacy_row(4, 1, 1, 0)));
        assert!(qualifies(&legacy_row(3, 427, 427, 12)));
        // Null evidence fields are unset, not claims.
        let row = set(&legacy_row(5, 10, 10, 0), "cell_results", PyValue::Null);
        let row = set(&row, "full_coverage", PyValue::Null);
        assert!(qualifies(&row));
    }

    #[test]
    fn legacy_clause_defeated_by_cell_evidence() {
        // Carried cell evidence keeps the strict path: without coverage
        // and base evidence this row refuses, evidence or not.
        let row = set(
            &legacy_row(5, 10, 10, 0),
            "cell_results",
            PyValue::Object(vec![]),
        );
        assert!(!qualifies(&row));
    }

    #[test]
    fn legacy_clause_defeated_by_full_coverage_claim() {
        let row = set(
            &legacy_row(5, 10, 10, 0),
            "full_coverage",
            PyValue::Bool(false),
        );
        assert!(!qualifies(&row));
    }

    #[test]
    fn legacy_clause_defeated_by_non_pass_result() {
        let row = set(&legacy_row(5, 10, 10, 0), "result", s("fail"));
        assert!(!qualifies(&row));
    }

    #[test]
    fn legacy_row_with_failing_counts_refused() {
        assert!(!qualifies(&legacy_row(5, 10, 9, 0)));
        assert!(!qualifies(&legacy_row(5, 0, 0, 0)));
        let short = set(&legacy_row(5, 10, 10, 0), "gates_run", int(4));
        assert!(!qualifies(&short));
        // An old-schema row carrying only one count proves nothing.
        let one_count = remove(&legacy_row(3, 10, 10, 0), "filtered_tests");
        let one_count = remove(&one_count, "passed_tests");
        assert!(!qualifies(&one_count));
    }

    #[test]
    fn uppercase_pass_refused() {
        assert!(!qualifies(&set(&happy(), "result", s("PASS"))));
    }

    #[test]
    fn prefix_sha_does_not_match() {
        let prefix = &SHA[..12];
        assert!(!row_qualifies(&happy(), prefix, &Policy::default()).0);
        let outcome = assess(&[happy()], prefix, &Policy::default());
        assert_eq!(outcome.verdict, Verdict::NoResult);
    }

    #[test]
    fn absent_profile_refused() {
        assert!(!qualifies(&remove(&happy(), "profile")));
    }

    #[test]
    fn float_executed_refused() {
        assert!(!qualifies(&set(
            &happy(),
            "executed_tests",
            PyValue::Float(10.0)
        )));
    }

    #[test]
    fn absent_passed_at_schema5_refused() {
        assert!(!qualifies(&remove(&happy(), "passed_tests")));
    }

    #[test]
    fn absent_coverage_refused() {
        // On the strict path (cell evidence carried), coverage is
        // required. A legacy count row without it is judged by the
        // legacy clause instead; see legacy_count_rows_qualify_by_counts.
        let row = set(&happy(), "cell_results", PyValue::Object(vec![]));
        assert!(!qualifies(&remove(&row, "coverage")));
    }

    #[test]
    fn inferred_gates_refused() {
        assert!(!qualifies(&remove(&happy(), "gates_expected")));
    }

    #[test]
    fn soft_green_class_refused() {
        let inherited = PyValue::Object(vec![
            ("delta_kind".to_owned(), s("rebase-only")),
            ("branch_commits".to_owned(), int(0)),
            ("upstream_commits".to_owned(), int(0)),
            ("force_full_paths".to_owned(), PyValue::Array(vec![])),
        ]);
        let row = set(&happy(), "validated_head_sha", hex40("d"));
        let row = set(&row, "inherited_from", inherited);
        assert!(!qualifies(&row));
    }

    #[test]
    fn over_run_gates_still_qualify() {
        assert!(qualifies(&set(&happy(), "gates_run", int(6))));
    }

    #[test]
    fn typed_skip_qualifies_without_counting_as_a_pass() {
        let skip = PyValue::Object(vec![
            ("name".to_owned(), s("node-a")),
            ("reason".to_owned(), s("empty-manifest-bucket")),
        ]);
        let row = set(&happy(), "gates_run", int(4));
        let row = set(
            &row,
            "intentional_skipped_nodes",
            PyValue::Array(vec![skip]),
        );
        let row = set(&row, "dependency_skipped_nodes", PyValue::Array(vec![]));
        let row = set(&row, "unaccounted_nodes", PyValue::Array(vec![]));
        let row = set(&row, "skipped_nodes", int(1));
        assert!(qualifies(&row));
    }

    #[test]
    fn env_fault_red_is_withheld_so_pass_still_qualifies() {
        let storm = red("fail", 127, 2.5);
        assert_eq!(
            disposition(&storm, SHA, &Policy::default()),
            Disposition::Withheld
        );
        let outcome = assess(&[storm, happy()], SHA, &Policy::default());
        assert_eq!(outcome.verdict, Verdict::Qualified);
    }

    #[test]
    fn newer_pass_does_not_wash_genuine_fail() {
        let genuine = red("fail", 1, 10.0);
        assert_eq!(
            disposition(&genuine, SHA, &Policy::default()),
            Disposition::Failed
        );
        let newer_pass = set(&happy(), "finished_at", s("2030-01-02T00:00:00Z"));
        let outcome = assess(&[genuine, newer_pass], SHA, &Policy::default());
        assert_eq!(outcome.verdict, Verdict::NotQualified);
        assert_eq!(outcome.reason, "genuine-failure-on-record");
        assert!(!outcome.qualified());
    }

    #[test]
    fn timeout_red_latches() {
        let timed_out = red("timeout", 1, 10.0);
        assert_eq!(
            disposition(&timed_out, SHA, &Policy::default()),
            Disposition::Failed
        );
        let outcome = assess(&[timed_out, happy()], SHA, &Policy::default());
        assert_eq!(outcome.verdict, Verdict::NotQualified);
    }

    #[test]
    fn no_result_is_not_not_qualified() {
        let empty = assess(&[], SHA, &Policy::default());
        assert_eq!(empty.verdict, Verdict::NoResult);
        let other_commit = set(&happy(), "commit", hex40("e"));
        let outcome = assess(&[other_commit], SHA, &Policy::default());
        assert_eq!(outcome.verdict, Verdict::NoResult);
        let subset_pass = set(&happy(), "profile", s("subset"));
        let outcome = assess(&[subset_pass], SHA, &Policy::default());
        assert_eq!(outcome.verdict, Verdict::NotQualified);
    }
}
