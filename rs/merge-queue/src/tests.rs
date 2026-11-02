//! Validation as a set of individual tests rather than one boolean.
//!
//! A test is a step ID with an optional case ID (`step` or `step::case`). Every validation the
//! queue runs, and every claim a submitter makes, names the exact selection it covered, so a
//! focused run is visibly focused and leaves a widening obligation behind it.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

/// Separator between a step ID and a case ID in the textual form of a [`TestId`].
pub const CASE_SEPARATOR: &str = "::";

/// One unit of validation: a runner step, optionally narrowed to one case inside it.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TestId {
    /// Runner step identifier, such as a DAG node name.
    pub step: String,
    /// Case inside the step, such as one test function; `None` means the whole step.
    pub case: Option<String>,
}

impl TestId {
    /// A whole step.
    pub fn step(step: impl Into<String>) -> Self {
        Self {
            step: step.into(),
            case: None,
        }
    }

    /// Parse `step` or `step::case`. Empty input and an empty step are rejected.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let (step, case) = match text.split_once(CASE_SEPARATOR) {
            Some((step, case)) => (step, Some(case)),
            None => (text, None),
        };
        if step.is_empty() {
            return Err(format!("invalid test id {text:?}: empty step"));
        }
        if case.is_some_and(str::is_empty) {
            return Err(format!("invalid test id {text:?}: empty case"));
        }
        Ok(Self {
            step: step.to_owned(),
            case: case.map(str::to_owned),
        })
    }

    /// The step this test belongs to, as a whole-step ID.
    pub fn whole_step(&self) -> Self {
        Self::step(self.step.clone())
    }
}

impl fmt::Display for TestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.case {
            Some(case) => write!(f, "{}{CASE_SEPARATOR}{case}", self.step),
            None => f.write_str(&self.step),
        }
    }
}

impl Serialize for TestId {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for TestId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// Which tests a validation covers. `All` is one selection among others, not a special mode.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind", content = "tests")]
pub enum Selection {
    /// The runner's complete suite.
    All,
    /// Exactly these tests.
    Tests(BTreeSet<TestId>),
}

impl Default for Selection {
    /// The empty selection.
    fn default() -> Self {
        Self::Tests(BTreeSet::new())
    }
}

impl Selection {
    /// A selection of the given tests.
    pub fn of(tests: impl IntoIterator<Item = TestId>) -> Self {
        Self::Tests(tests.into_iter().collect())
    }

    /// Whether this selection is the complete suite.
    pub const fn is_all(&self) -> bool {
        matches!(self, Self::All)
    }

    /// Whether this selection names no tests at all.
    pub fn is_empty(&self) -> bool {
        matches!(self, Self::Tests(tests) if tests.is_empty())
    }

    /// Union of two selections; anything unioned with `All` is `All`.
    pub fn union(&self, other: &Self) -> Self {
        match (self, other) {
            (Self::All, _) | (_, Self::All) => Self::All,
            (Self::Tests(a), Self::Tests(b)) => Self::Tests(a.union(b).cloned().collect()),
        }
    }

    /// Intersection of two selections; `All` is the identity.
    pub fn intersect(&self, other: &Self) -> Self {
        match (self, other) {
            (Self::All, x) | (x, Self::All) => x.clone(),
            (Self::Tests(a), Self::Tests(b)) => Self::Tests(
                a.iter()
                    .chain(b)
                    .filter(|test| covers(a, test) && covers(b, test))
                    .cloned()
                    .collect(),
            ),
        }
    }

    /// Whether this selection covers `test`: listed, or its whole step is listed.
    pub fn covers(&self, test: &TestId) -> bool {
        match self {
            Self::All => true,
            Self::Tests(tests) => covers(tests, test),
        }
    }

    /// The listed tests, or `None` for `All`.
    pub const fn tests(&self) -> Option<&BTreeSet<TestId>> {
        match self {
            Self::All => None,
            Self::Tests(tests) => Some(tests),
        }
    }

    /// Short human summary such as `all` or `3 tests`.
    pub fn summary(&self) -> String {
        match self {
            Self::All => "all".to_owned(),
            Self::Tests(tests) if tests.len() == 1 => "1 test".to_owned(),
            Self::Tests(tests) => format!("{} tests", tests.len()),
        }
    }
}

fn covers(tests: &BTreeSet<TestId>, test: &TestId) -> bool {
    tests.contains(test) || (test.case.is_some() && tests.contains(&test.whole_step()))
}

/// Whether a validation covered the complete suite or a policy-chosen subset of it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    /// The complete suite ran.
    Complete,
    /// A subset ran; the rest is owed as a widening obligation.
    Focused,
}

impl Scope {
    /// The scope a selection implies.
    pub const fn of(selection: &Selection) -> Self {
        if selection.is_all() {
            Self::Complete
        } else {
            Self::Focused
        }
    }

    /// Stable machine-facing value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Focused => "focused",
        }
    }
}

/// Result of one test.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    /// The test passed.
    Pass,
    /// The test ran and failed.
    Fail,
    /// The runner reported the test skipped; it proves nothing either way.
    Skip,
    /// The test could not run or report (timeout, crash).
    Error,
    /// The test was selected but the runner never reported it. Blocks landing (silence is not
    /// success), but does not reproduce a failure in a culprit search: a test that does not
    /// exist yet at an earlier commit cannot fail there.
    Missing,
}

impl Outcome {
    /// Whether this outcome blocks landing.
    pub const fn is_bad(self) -> bool {
        matches!(self, Self::Fail | Self::Error | Self::Missing)
    }

    /// Parse a runner-reported outcome. Accepts common synonyms.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "pass" | "passed" | "ok" | "success" => Some(Self::Pass),
            "fail" | "failed" | "failure" => Some(Self::Fail),
            "skip" | "skipped" => Some(Self::Skip),
            "error" | "errored" | "timeout" | "timed-out" | "cancelled" => Some(Self::Error),
            _ => None,
        }
    }
}

/// One reported test result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TestResult {
    /// Test identifier.
    pub id: TestId,
    /// Outcome.
    pub outcome: Outcome,
    /// Wall time in milliseconds, when the runner reported it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// Per-test results of one validation run over one exact commit.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct TestResults {
    /// Results keyed by test.
    pub results: BTreeMap<TestId, Outcome>,
    /// Durations where known.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub durations_ms: BTreeMap<TestId, u64>,
    /// True when the runner gave no per-test attribution, so every selected test inherited the
    /// command's exit status. Bisection over such a result reruns the whole selection.
    #[serde(default)]
    pub unattributed: bool,
}

impl TestResults {
    /// Record one result; a worse outcome for the same test wins.
    pub fn record(&mut self, result: TestResult) {
        let slot = self
            .results
            .entry(result.id.clone())
            .or_insert(result.outcome);
        if result.outcome > *slot {
            *slot = result.outcome;
        }
        if let Some(ms) = result.duration_ms {
            self.durations_ms.insert(result.id, ms);
        }
    }

    /// Tests that actually failed or errored (not merely unreported).
    pub fn attributed_failures(&self) -> BTreeSet<TestId> {
        self.results
            .iter()
            .filter(|(_, outcome)| matches!(outcome, Outcome::Fail | Outcome::Error))
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Tests whose outcome blocks landing.
    pub fn failed(&self) -> BTreeSet<TestId> {
        self.results
            .iter()
            .filter(|(_, outcome)| outcome.is_bad())
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Whether every reported result is non-blocking and at least the selection was honoured.
    pub fn green(&self) -> bool {
        self.results.values().all(|outcome| !outcome.is_bad())
    }

    /// Counts by outcome, for reports.
    pub fn counts(&self) -> BTreeMap<Outcome, usize> {
        let mut counts = BTreeMap::new();
        for outcome in self.results.values() {
            *counts.entry(*outcome).or_default() += 1;
        }
        counts
    }

    /// Mark selected tests that the runner never reported as missing, so silence never passes.
    pub fn require(&mut self, selection: &Selection) {
        let Some(tests) = selection.tests() else {
            return;
        };
        for test in tests {
            let reported = self.results.contains_key(test)
                || self
                    .results
                    .keys()
                    .any(|seen| seen.step == test.step && test.case.is_none());
            if !reported {
                self.results.insert(test.clone(), Outcome::Missing);
            }
        }
    }
}

/// A claim or record of which tests validated a change, and how they were chosen.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ValidationClaim {
    /// Exact selection that ran.
    pub selection: Selection,
    /// Name of the selection policy that chose it (`all`, `path-routing`, `manual`, ...).
    pub policy: String,
    /// Complete or focused.
    pub scope: Scope,
    /// Commit the selection ran against, when known.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub commit: String,
}

impl ValidationClaim {
    /// Build a claim, deriving the scope from the selection.
    pub fn new(selection: Selection, policy: impl Into<String>, commit: impl Into<String>) -> Self {
        let scope = Scope::of(&selection);
        Self {
            selection,
            policy: policy.into(),
            scope,
            commit: commit.into(),
        }
    }
}

/// Parse a selection argument: `all`, a comma/space/newline separated list of test IDs, or
/// `@FILE` naming a file with one test ID per line (`#` starts a comment).
pub fn parse_selection(text: &str) -> Result<Selection, String> {
    let body = if let Some(path) = text.strip_prefix('@') {
        std::fs::read_to_string(path).map_err(|error| format!("cannot read {path}: {error}"))?
    } else {
        text.to_owned()
    };
    let mut tests = BTreeSet::new();
    for line in body.lines() {
        let line = line.split('#').next().unwrap_or_default();
        for word in line.split([',', ' ', '\t']).filter(|word| !word.is_empty()) {
            if word == "all" {
                return Ok(Selection::All);
            }
            tests.insert(TestId::parse(word)?);
        }
    }
    Ok(Selection::Tests(tests))
}

#[cfg(test)]
mod unit {
    use super::*;

    fn ids(list: &[&str]) -> Selection {
        Selection::of(list.iter().map(|id| TestId::parse(id).unwrap()))
    }

    #[test]
    fn test_ids_round_trip_with_cases() {
        let id = TestId::parse("rust.test::queue::bisect_halves").unwrap();
        assert_eq!(id.step, "rust.test");
        assert_eq!(id.case.as_deref(), Some("queue::bisect_halves"));
        assert_eq!(id.to_string(), "rust.test::queue::bisect_halves");
        assert!(TestId::parse("::x").is_err());
        assert!(TestId::parse("a::").is_err());
    }

    #[test]
    fn whole_step_covers_its_cases() {
        let sel = ids(&["lint"]);
        assert!(sel.covers(&TestId::parse("lint::file.rs").unwrap()));
        assert!(!sel.covers(&TestId::parse("unit").unwrap()));
        assert!(Selection::All.covers(&TestId::step("anything")));
    }

    #[test]
    fn union_and_intersection_treat_all_as_identity_and_absorber() {
        let a = ids(&["a", "b"]);
        let b = ids(&["b", "c::x"]);
        assert_eq!(a.union(&b), ids(&["a", "b", "c::x"]));
        assert_eq!(a.union(&Selection::All), Selection::All);
        assert_eq!(a.intersect(&Selection::All), a);
        assert_eq!(a.intersect(&b), ids(&["b"]));
        assert_eq!(ids(&["c"]).intersect(&b), ids(&["c::x"]));
    }

    #[test]
    fn silent_tests_become_errors() {
        let mut results = TestResults::default();
        results.record(TestResult {
            id: TestId::step("a"),
            outcome: Outcome::Pass,
            duration_ms: Some(5),
        });
        results.require(&ids(&["a", "b"]));
        assert_eq!(results.failed(), [TestId::step("b")].into());
        assert!(!results.green());
    }

    #[test]
    fn selection_argument_forms() {
        assert_eq!(parse_selection("all").unwrap(), Selection::All);
        assert_eq!(parse_selection("a, b c").unwrap(), ids(&["a", "b", "c"]));
        assert!(parse_selection("").unwrap().is_empty());
    }

    #[test]
    fn selection_serializes_with_kind_tag() {
        let json = serde_json::to_string(&ids(&["x::1"])).unwrap();
        assert_eq!(json, r#"{"kind":"tests","tests":["x::1"]}"#);
        let all: Selection = serde_json::from_str(r#"{"kind":"all"}"#).unwrap();
        assert!(all.is_all());
    }
}
