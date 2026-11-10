//! Structured per-step test results written by controlled test runners.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use serde_json::Map;
use serde_json::Value;

/// Environment variable naming the path where a controlled test runner may write its
/// structured test results.
///
/// The harness owns the path and sets it after any environment the run's own
/// configuration provides, so a run cannot redirect the evidence to a stale or shared
/// file.
pub const TEST_COUNTS_PATH_ENV: &str = "DAGRUN_TEST_COUNTS_PATH";

/// Suffix for the scheduler-owned recovery sibling of a structured-result path.
///
/// A producer uses this only after the primary report has been fully validated
/// but cannot be written or atomically published.  Keeping the recovery file in
/// the same directory preserves existing container bind mounts while the
/// scheduler's per-attempt nonce keeps it isolated from every other attempt.
pub const TEST_RESULTS_RECOVERY_SUFFIX: &str = ".publication-recovery";

/// Return the recovery sibling shared by controlled producers and the scheduler.
pub fn structured_test_results_recovery_path(path: &Path) -> PathBuf {
    let mut recovery = path.as_os_str().to_owned();
    recovery.push(TEST_RESULTS_RECOVERY_SUFFIX);
    PathBuf::from(recovery)
}

/// A JSON value decoded while every mapping entry is still observable.
///
/// `serde_json::Value` keeps only the last occurrence of a repeated object key. That behavior is
/// unsafe for result evidence: a later `"result":"pass"` or `"outcome":"passed"` could erase an
/// earlier failure before the strict field checks see it. This visitor rejects a repeated key at
/// every object depth before constructing the corresponding [`Value::Object`].
struct UniqueJsonValue(Value);

impl<'de> Deserialize<'de> for UniqueJsonValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(UniqueJsonValueVisitor)
    }
}

struct UniqueJsonValueVisitor;

impl<'de> Visitor<'de> for UniqueJsonValueVisitor {
    type Value = UniqueJsonValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value whose object keys are unique")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(UniqueJsonValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(UniqueJsonValue(Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(UniqueJsonValue(Value::Number(value.into())))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .map(UniqueJsonValue)
            .ok_or_else(|| E::custom("non-finite numbers are not supported"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(UniqueJsonValue(Value::String(value.to_owned())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(UniqueJsonValue(Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueJsonValue(Value::Null))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(UniqueJsonValue(Value::Null))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = sequence.next_element::<UniqueJsonValue>()? {
            values.push(value.0);
        }
        Ok(UniqueJsonValue(Value::Array(values)))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut values = Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if values.contains_key(&key) {
                return Err(de::Error::custom(format!("duplicate key {key:?}")));
            }
            let value = map.next_value::<UniqueJsonValue>()?;
            values.insert(key, value.0);
        }
        Ok(UniqueJsonValue(Value::Object(values)))
    }
}

fn parse_unique_json_value(bytes: &[u8]) -> Result<Value, String> {
    serde_json::from_slice::<UniqueJsonValue>(bytes)
        .map(|value| value.0)
        .map_err(|error| format!("structured-test-results-json: {error}"))
}

/// Current structured-result schema for this release line.
pub const CURRENT_SCHEMA: u64 = 2;
/// Explicit opt-in schema retaining complete classified attempt histories.
pub const CLASSIFIED_RESULTS_SCHEMA: u64 = 3;
/// Schema 2 remains readable while result producers migrate atomically.
pub const RETAINED_RESULTS_SCHEMA: u64 = 2;
/// Explicit opt-in schema: schema 3 plus non-blocking diagnostic failures.
///
/// A row may be a `diagnostic_fail`: the test failed, the producer declared in advance why its
/// failure does not decide the step, and the reason travels with the row. The scheduler reports
/// and counts such a row separately and does not fail the step on it. Every other non-pass is
/// still a blocking `fail`.
pub const DIAGNOSTIC_RESULTS_SCHEMA: u64 = 4;
/// Explicit opt-in schema: schema 4 plus first-class runtime skips.
///
/// A row may be a `skip`: the test started, reported that it could not run (for example a
/// required device is absent), and returned normally. The reason travels with the row. A skip
/// is neither a pass nor a failure: it counts in `executed_tests`, never in passed or failed,
/// and it never fails its step by itself. A step that must not skip declares that policy
/// outside this record. Only this schema may carry a skip; earlier writers refuse one rather
/// than fold it into a pass or a failure.
pub const SKIPPED_RESULTS_SCHEMA: u64 = 5;

/// Why the scheduler could not import a required structured result.
///
/// This describes the actual read or validation operation, not the producer's output.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TestResultsErrorKind {
    /// No report was present at the scheduler-owned path.
    Missing,
    /// Reading the path failed for a reason other than absence.
    ReadIo,
    /// The bytes did not satisfy the declared result schema or its invariants.
    InvalidReport,
}

impl TestResultsErrorKind {
    /// Stable spelling in retained event records.
    pub fn value(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::ReadIo => "read_io",
            Self::InvalidReport => "invalid_report",
        }
    }

    /// Decode a known kind. Unknown or absent retained values have no typed authority.
    pub fn from_value(value: &str) -> Option<Self> {
        match value {
            "missing" => Some(Self::Missing),
            "read_io" => Some(Self::ReadIo),
            "invalid_report" => Some(Self::InvalidReport),
            _ => None,
        }
    }
}

/// Failure to validate, write, or atomically publish a structured result.
#[derive(Debug)]
pub enum TestResultsWriteError {
    /// The report, schema, or destination path was invalid before I/O.
    Invalid(String),
    /// Writing the temporary file failed.
    Write {
        /// The temporary path actually written.
        path: PathBuf,
        /// The original filesystem error.
        source: io::Error,
    },
    /// Renaming the completed temporary file to its destination failed.
    Publish {
        /// The final destination path.
        path: PathBuf,
        /// The original filesystem error.
        source: io::Error,
    },
}

impl fmt::Display for TestResultsWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => formatter.write_str(message),
            Self::Write { path, source } => write!(
                formatter,
                "structured-test-results-write {}: {source}",
                path.display()
            ),
            Self::Publish { path, source } => write!(
                formatter,
                "structured-test-results-publish {}: {source}",
                path.display()
            ),
        }
    }
}

impl std::error::Error for TestResultsWriteError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Invalid(_) => None,
            Self::Write { source, .. } | Self::Publish { source, .. } => Some(source),
        }
    }
}

/// Classified terminal cause for one test-runner attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TestAttemptOutcome {
    /// Attempt completed successfully.
    Passed,
    /// The controlled test attempt reported an ordinary failure.
    Failed,
    /// Attempt exceeded its per-attempt CPU-time bound.
    CpuTimeout,
    /// Attempt exceeded its separate wall-clock backstop.
    WallTimeout,
    /// Attempt was stopped by external cancellation.
    Cancelled,
    /// Attempt could not produce trustworthy execution or accounting evidence.
    InfrastructureError,
    /// Attempt produced an explicit no-result classification that is neither a
    /// timeout nor an infrastructure/accounting failure.
    NoResult,
    /// The test started and reported that it could not run; it returned normally.
    Skipped,
}

impl TestAttemptOutcome {
    /// Stable schema spelling for this attempt outcome.
    pub fn value(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::CpuTimeout => "cpu_timeout",
            Self::WallTimeout => "wall_timeout",
            Self::Cancelled => "cancelled",
            Self::InfrastructureError => "infrastructure_error",
            Self::NoResult => "no_result",
            Self::Skipped => "skipped",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "passed" => Some(Self::Passed),
            "failed" => Some(Self::Failed),
            "cpu_timeout" => Some(Self::CpuTimeout),
            "wall_timeout" => Some(Self::WallTimeout),
            "cancelled" => Some(Self::Cancelled),
            "infrastructure_error" => Some(Self::InfrastructureError),
            "no_result" => Some(Self::NoResult),
            "skipped" => Some(Self::Skipped),
            _ => None,
        }
    }
}

/// One classified attempt retained in the scheduler-consumed result file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestAttemptResult {
    /// One-based attempt number within the named test.
    pub attempt: u64,
    /// Machine-readable terminal cause.
    pub outcome: TestAttemptOutcome,
    /// Required human-readable cause for non-passing attempts; absent for passes.
    pub detail: Option<String>,
}

impl TestAttemptResult {
    /// Construct and validate one classified attempt.
    pub fn new(
        attempt: u64,
        outcome: TestAttemptOutcome,
        detail: Option<String>,
    ) -> Result<Self, String> {
        if attempt == 0 {
            return Err("structured-test-results-attempt must be positive".into());
        }
        match (outcome, detail.as_deref()) {
            (TestAttemptOutcome::Passed, None) => {}
            (TestAttemptOutcome::Passed, Some(_)) => {
                return Err("structured-test-results passed attempt must not carry detail".into());
            }
            (_, Some(detail)) if !detail.is_empty() && detail.trim() == detail => {}
            _ => {
                return Err(
                    "structured-test-results non-passing attempt requires nonempty trimmed detail"
                        .into(),
                );
            }
        }
        Ok(Self {
            attempt,
            outcome,
            detail,
        })
    }
}

/// Terminal result of one named test, including how many attempts the test runner made.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestResult {
    /// Stable test identity emitted by the controlled test runner.
    pub id: String,
    /// Terminal verdict after the runner's own retries.
    pub passed: bool,
    /// Number of attempts made by the test runner; always at least one.
    pub attempts: u64,
    /// Per-attempt classified causes. `None` is retained schema 2.
    pub attempt_results: Option<Vec<TestAttemptResult>>,
    /// Why this FAILED test does not fail its step (schema 4 only).
    ///
    /// `Some` only on a non-passing row with classified attempts; the scheduler reports and
    /// counts it as a diagnostic failure instead of failing the step. `None` on every pass and on
    /// every blocking failure.
    pub diagnostic: Option<String>,
    /// Why this test reported at runtime that it could not run (schema 5 only).
    ///
    /// `Some` only on a `skip` row with classified attempts; the scheduler reports and
    /// counts it as a skip instead of a pass or a failure. `None` on every other row.
    /// Mutually exclusive with `diagnostic`.
    pub skip_reason: Option<String>,
}

impl TestResult {
    /// Construct one retained schema-2 terminal result.
    ///
    /// Opt-in schema-3 writers must use [Self::with_attempt_results].
    pub fn new(id: String, passed: bool, attempts: u64) -> Result<Self, String> {
        if id.is_empty() || id.trim() != id {
            return Err("structured-test-results-id must be nonempty and trimmed".into());
        }
        if attempts == 0 {
            return Err("structured-test-results-attempts must be positive".into());
        }
        Ok(Self {
            id,
            passed,
            attempts,
            attempt_results: None,
            diagnostic: None,
            skip_reason: None,
        })
    }

    /// Attach complete classified attempt evidence for opt-in schema 3.
    pub fn with_attempt_results(
        id: String,
        passed: bool,
        attempts: Vec<TestAttemptResult>,
    ) -> Result<Self, String> {
        let attempt_count = u64::try_from(attempts.len())
            .map_err(|_| "structured-test-results attempt count does not fit u64".to_string())?;
        let result = Self {
            id,
            passed,
            attempts: attempt_count,
            attempt_results: Some(attempts),
            diagnostic: None,
            skip_reason: None,
        };
        validate_result(&result, true)?;
        Ok(result)
    }

    /// A failed test whose failure the producer declared non-blocking, for opt-in schema 4.
    ///
    /// `reason` is the producer's standing explanation of why this test cannot decide the step
    /// (not the cause of this failure, which the final attempt's detail carries). The terminal
    /// attempt must be a non-pass: a passing test is an ordinary `pass` row whatever its policy.
    pub fn diagnostic_failure(
        id: String,
        attempts: Vec<TestAttemptResult>,
        reason: String,
    ) -> Result<Self, String> {
        let attempt_count = u64::try_from(attempts.len())
            .map_err(|_| "structured-test-results attempt count does not fit u64".to_string())?;
        let result = Self {
            id,
            passed: false,
            attempts: attempt_count,
            attempt_results: Some(attempts),
            diagnostic: Some(reason),
            skip_reason: None,
        };
        validate_result(&result, true)?;
        Ok(result)
    }

    /// A test that started and reported it could not run, for opt-in schema 5.
    ///
    /// `reason` is what the test itself reported (for example the absent resource).
    /// The terminal attempt must be `skipped`: a test that ran to a pass or a failure
    /// is an ordinary row whatever it printed along the way.
    pub fn skipped(
        id: String,
        attempts: Vec<TestAttemptResult>,
        reason: String,
    ) -> Result<Self, String> {
        let attempt_count = u64::try_from(attempts.len())
            .map_err(|_| "structured-test-results attempt count does not fit u64".to_string())?;
        let result = Self {
            id,
            passed: false,
            attempts: attempt_count,
            attempt_results: Some(attempts),
            diagnostic: None,
            skip_reason: Some(reason),
        };
        validate_result(&result, true)?;
        Ok(result)
    }

    /// A failure that fails the step: every non-pass that is neither a declared
    /// diagnostic nor a runtime skip.
    pub fn is_blocking_failure(&self) -> bool {
        !self.passed && self.diagnostic.is_none() && self.skip_reason.is_none()
    }

    /// A failure the producer declared non-blocking; reported and counted, never ignored.
    pub fn is_diagnostic_failure(&self) -> bool {
        !self.passed && self.diagnostic.is_some()
    }

    /// A runtime skip; reported and counted, never folded into pass or fail.
    pub fn is_skipped(&self) -> bool {
        self.skip_reason.is_some()
    }

    fn wire_result(&self) -> &'static str {
        match (
            self.passed,
            self.diagnostic.is_some(),
            self.skip_reason.is_some(),
        ) {
            (true, _, _) => "pass",
            (false, false, false) => "fail",
            (false, true, _) => "diagnostic_fail",
            (false, false, true) => "skip",
        }
    }
}

fn parse_result_row(
    row: &Map<String, Value>,
    index: usize,
    schema: u64,
) -> Result<TestResult, String> {
    let classified = schema >= CLASSIFIED_RESULTS_SCHEMA;
    let diagnostic_schema = schema == DIAGNOSTIC_RESULTS_SCHEMA;
    let skipped_schema = schema == SKIPPED_RESULTS_SCHEMA;
    let expected = if skipped_schema {
        &[
            "id",
            "result",
            "attempts",
            "attempt_results",
            "diagnostic_reason",
            "skip_reason",
        ][..]
    } else if diagnostic_schema {
        &[
            "id",
            "result",
            "attempts",
            "attempt_results",
            "diagnostic_reason",
        ][..]
    } else if classified {
        &["id", "result", "attempts", "attempt_results"][..]
    } else {
        &["id", "result", "attempts"][..]
    };
    exact_fields(row, expected)?;
    let id = row
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("structured-test-results-results[{index}].id must be a string"))?
        .to_string();
    let (passed, diagnostic_result, skip_result) = match row.get("result").and_then(Value::as_str) {
        Some("pass") => (true, false, false),
        Some("fail") => (false, false, false),
        // Only the schema that carries a reason may say a failure is non-blocking.
        Some("diagnostic_fail") if diagnostic_schema || skipped_schema => (false, true, false),
        // Only the schema that carries a skip reason may say a test skipped.
        Some("skip") if skipped_schema => (false, false, true),
        Some(value) => {
            return Err(format!(
                "structured-test-results-results[{index}].result has unknown value {value:?}"
            ));
        }
        None => {
            return Err(format!(
                "structured-test-results-results[{index}].result must be a string"
            ));
        }
    };
    let attempts = row.get("attempts").and_then(Value::as_u64).ok_or_else(|| {
        format!("structured-test-results-results[{index}].attempts must be an unsigned integer")
    })?;
    if !classified {
        return TestResult::new(id, passed, attempts);
    }
    let attempt_rows = match row.get("attempt_results") {
        Some(Value::Array(rows)) => rows,
        Some(Value::Null) => {
            return Err(format!(
                "structured-test-results-results[{index}].attempt_results must be an array for classified schema"
            ));
        }
        _ => {
            return Err(format!(
                "structured-test-results-results[{index}].attempt_results must be an array"
            ));
        }
    };
    let mut attempt_results = Vec::with_capacity(attempt_rows.len());
    for (attempt_index, attempt) in attempt_rows.iter().enumerate() {
        let attempt = attempt.as_object().ok_or_else(|| {
            format!(
                "structured-test-results-results[{index}].attempt_results[{attempt_index}] must be an object"
            )
        })?;
        exact_fields(attempt, &["attempt", "outcome", "detail"])?;
        let number = required_u64(attempt, "attempt")?;
        let value = attempt
            .get("outcome")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                format!(
                    "structured-test-results-results[{index}].attempt_results[{attempt_index}].outcome must be a string"
                )
            })?;
        let outcome = TestAttemptOutcome::parse(value).ok_or_else(|| {
            format!(
                "structured-test-results-results[{index}].attempt_results[{attempt_index}].outcome has unknown value {value:?}"
            )
        })?;
        let detail = match attempt.get("detail") {
            Some(Value::Null) => None,
            Some(Value::String(value)) => Some(value.clone()),
            _ => {
                return Err(format!(
                    "structured-test-results-results[{index}].attempt_results[{attempt_index}].detail must be a string or null"
                ));
            }
        };
        attempt_results.push(
            TestAttemptResult::new(number, outcome, detail).map_err(|error| {
                format!(
                "structured-test-results-results[{index}].attempt_results[{attempt_index}]: {error}"
            )
            })?,
        );
    }
    let diagnostic = if diagnostic_schema || skipped_schema {
        match (row.get("diagnostic_reason"), diagnostic_result) {
            (Some(Value::Null), false) => None,
            (Some(Value::String(reason)), true) => Some(reason.clone()),
            (_, true) => {
                return Err(format!(
                    "structured-test-results-results[{index}].diagnostic_reason must be a string on a diagnostic_fail row"
                ));
            }
            (_, false) => {
                return Err(format!(
                    "structured-test-results-results[{index}].diagnostic_reason must be null unless result is diagnostic_fail"
                ));
            }
        }
    } else {
        None
    };
    let skip_reason = if skipped_schema {
        match (row.get("skip_reason"), skip_result) {
            (Some(Value::Null), false) => None,
            (Some(Value::String(reason)), true) => Some(reason.clone()),
            (_, true) => {
                return Err(format!(
                    "structured-test-results-results[{index}].skip_reason must be a string on a skip row"
                ));
            }
            (_, false) => {
                return Err(format!(
                    "structured-test-results-results[{index}].skip_reason must be null unless result is skip"
                ));
            }
        }
    } else {
        None
    };
    let result = TestResult {
        id,
        passed,
        attempts,
        attempt_results: Some(attempt_results),
        diagnostic,
        skip_reason,
    };
    validate_result(&result, false)?;
    Ok(result)
}

/// Counts and terminal per-test results captured from one controlled test-runner step.
///
/// Schema 1 retained only the two counts. It remains readable with `results == None`,
/// schema 2 retained terminal rows without typed attempts; schema 3 is an explicit classified write path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestResults {
    /// Tests that executed, according to the controlled runner's aggregate report.
    pub executed_tests: u64,
    /// Tests excluded by selection, according to the same aggregate report.
    pub filtered_tests: u64,
    /// Per-test terminal results. `None` is retained schema 1, not an empty run.
    pub results: Option<Vec<TestResult>>,
    /// The run's selection, recorded once (schema 5 only).
    ///
    /// Omitted tests are implied by this record and are never enumerated per
    /// test: a test with no row was omitted for this run. `None` on every
    /// pre-schema-5 document and on schema-5 documents that do not record it.
    pub selection: Option<TestSelection>,
}

/// One run's selection: what was asked for, recorded once at run level.
///
/// Results stay sparse: only tests that started get per-test rows. A focused
/// rerun of one test makes every other test omitted for that run without
/// listing them; they are implied by `focused_tests` (or by `expression`
/// plus the `disabled_in_config` sets, whose source is hashed so the
/// omission set is reproducible from the run record alone).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestSelection {
    /// The selection or label expression, when the run used one.
    pub expression: Option<String>,
    /// Explicitly focused test ids, when the run listed tests by name.
    pub focused_tests: Vec<String>,
    /// Suites or label sets disabled in configuration for this run.
    pub disabled_in_config: Vec<String>,
    /// Where `disabled_in_config` came from (config path + content hash).
    pub disabled_in_config_source: Option<TestSelectionSource>,
}

/// Provenance of a selection's disabled-in-config sets.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestSelectionSource {
    /// Config file the disabled sets were read from.
    pub path: String,
    /// SHA-256 of that file's content at run time (64 lowercase hex).
    pub sha256: String,
}

fn validate_selection(selection: &TestSelection) -> Result<(), String> {
    if let Some(expression) = selection.expression.as_deref() {
        if expression.is_empty() || expression.trim() != expression {
            return Err(
                "structured-test-results selection.expression must be nonempty and trimmed".into(),
            );
        }
    }
    for (field, entries) in [
        ("focused_tests", &selection.focused_tests),
        ("disabled_in_config", &selection.disabled_in_config),
    ] {
        let mut seen = std::collections::BTreeSet::new();
        for entry in entries {
            if entry.is_empty() || entry.trim() != entry {
                return Err(format!(
                    "structured-test-results selection.{field} entries must be nonempty and trimmed"
                ));
            }
            if !seen.insert(entry) {
                return Err(format!(
                    "structured-test-results selection.{field} duplicates {entry:?}"
                ));
            }
        }
    }
    if let Some(source) = selection.disabled_in_config_source.as_ref() {
        if source.path.is_empty() || source.path.trim() != source.path {
            return Err("structured-test-results selection.disabled_in_config_source.path must be nonempty and trimmed".into());
        }
        let digest_ok = source.sha256.len() == 64
            && source
                .sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if !digest_ok {
            return Err("structured-test-results selection.disabled_in_config_source.sha256 must be 64 lowercase hex".into());
        }
    }
    Ok(())
}

fn selection_to_json(selection: &TestSelection) -> Value {
    serde_json::json!({
        "expression": selection.expression,
        "focused_tests": selection.focused_tests,
        "disabled_in_config": selection.disabled_in_config,
        "disabled_in_config_source": selection.disabled_in_config_source.as_ref().map(|source| serde_json::json!({
            "path": source.path,
            "sha256": source.sha256,
        })),
    })
}

fn parse_selection(value: &Value) -> Result<TestSelection, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "structured-test-results-selection must be an object".to_string())?;
    exact_fields(
        object,
        &[
            "expression",
            "focused_tests",
            "disabled_in_config",
            "disabled_in_config_source",
        ],
    )?;
    let strings = |field: &str| -> Result<Vec<String>, String> {
        let entries = object
            .get(field)
            .and_then(Value::as_array)
            .ok_or_else(|| format!("structured-test-results-selection.{field} must be an array"))?;
        entries
            .iter()
            .map(|entry| {
                entry.as_str().map(str::to_string).ok_or_else(|| {
                    format!("structured-test-results-selection.{field} must be strings")
                })
            })
            .collect()
    };
    let expression = match object.get("expression") {
        Some(Value::Null) => None,
        Some(Value::String(expression)) => Some(expression.clone()),
        _ => {
            return Err(
                "structured-test-results-selection.expression must be a string or null".into(),
            )
        }
    };
    let disabled_in_config_source = match object.get("disabled_in_config_source") {
        Some(Value::Null) => None,
        Some(source) => {
            let source = source
                .as_object()
                .ok_or_else(|| "structured-test-results-selection.disabled_in_config_source must be an object or null".to_string())?;
            exact_fields(source, &["path", "sha256"])?;
            Some(TestSelectionSource {
                path: source
                    .get("path")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "structured-test-results-selection.disabled_in_config_source.path must be a string".to_string())?
                    .to_string(),
                sha256: source
                    .get("sha256")
                    .and_then(Value::as_str)
                    .ok_or_else(|| "structured-test-results-selection.disabled_in_config_source.sha256 must be a string".to_string())?
                    .to_string(),
            })
        }
        None => {
            return Err(
                "structured-test-results-selection.disabled_in_config_source is required".into(),
            )
        }
    };
    let selection = TestSelection {
        expression,
        focused_tests: strings("focused_tests")?,
        disabled_in_config: strings("disabled_in_config")?,
        disabled_in_config_source,
    };
    validate_selection(&selection)?;
    Ok(selection)
}

impl TestResults {
    /// Construct schema-2 terminal results, preserving the default writer contract.
    pub fn current(
        executed_tests: u64,
        filtered_tests: u64,
        results: Vec<TestResult>,
    ) -> Result<Self, String> {
        validate_results(executed_tests, &results)?;
        for result in &results {
            require_legacy_result(result)?;
        }
        Ok(Self {
            executed_tests,
            filtered_tests,
            results: Some(results),
            selection: None,
        })
    }

    /// Construct opt-in schema-3 results with complete classified attempts.
    pub fn classified(
        executed_tests: u64,
        filtered_tests: u64,
        results: Vec<TestResult>,
    ) -> Result<Self, String> {
        validate_results(executed_tests, &results)?;
        for result in &results {
            validate_result(result, true)?;
            require_blocking_result(result)?;
        }
        Ok(Self {
            executed_tests,
            filtered_tests,
            results: Some(results),
            selection: None,
        })
    }

    /// Construct opt-in schema-4 results: classified attempts plus diagnostic failures.
    pub fn diagnostic(
        executed_tests: u64,
        filtered_tests: u64,
        results: Vec<TestResult>,
    ) -> Result<Self, String> {
        validate_results(executed_tests, &results)?;
        for result in &results {
            validate_result(result, true)?;
            require_no_skip_result(result)?;
        }
        Ok(Self {
            executed_tests,
            filtered_tests,
            results: Some(results),
            selection: None,
        })
    }

    /// Construct opt-in schema-5 results: schema 4 plus first-class runtime skips.
    pub fn skipped(
        executed_tests: u64,
        filtered_tests: u64,
        results: Vec<TestResult>,
    ) -> Result<Self, String> {
        validate_results(executed_tests, &results)?;
        for result in &results {
            validate_result(result, true)?;
        }
        Ok(Self {
            executed_tests,
            filtered_tests,
            results: Some(results),
            selection: None,
        })
    }

    /// Attach the run's selection record (schema 5 only).
    pub fn with_selection(mut self, selection: TestSelection) -> Result<Self, String> {
        validate_selection(&selection)?;
        self.selection = Some(selection);
        Ok(self)
    }

    /// Read count-only schema 1, default schema 2, classified schema 3, diagnostic
    /// schema 4, or skipped schema 5.
    pub fn from_json_slice(bytes: &[u8]) -> Result<Self, String> {
        let value = parse_unique_json_value(bytes)?;
        let object = value
            .as_object()
            .ok_or_else(|| "structured-test-results must be an object".to_string())?;
        let schema = required_u64(object, "schema")?;
        let executed_tests = required_u64(object, "executed_tests")?;
        let filtered_tests = required_u64(object, "filtered_tests")?;
        match schema {
            1 => {
                exact_fields(object, &["schema", "executed_tests", "filtered_tests"])?;
                Ok(Self {
                    executed_tests,
                    filtered_tests,
                    results: None,
                    selection: None,
                })
            }
            CURRENT_SCHEMA
            | CLASSIFIED_RESULTS_SCHEMA
            | DIAGNOSTIC_RESULTS_SCHEMA
            | SKIPPED_RESULTS_SCHEMA => {
                if schema == SKIPPED_RESULTS_SCHEMA {
                    // `selection` is optional run-level metadata; every
                    // other key must be exactly the schema-5 set.
                    let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
                    keys.retain(|key| *key != "selection");
                    keys.sort_unstable();
                    if keys != ["executed_tests", "filtered_tests", "results", "schema"] {
                        return Err(format!(
                            "structured-test-results has unsupported fields {keys:?}"
                        ));
                    }
                } else {
                    exact_fields(
                        object,
                        &["schema", "executed_tests", "filtered_tests", "results"],
                    )?;
                }
                let rows = object
                    .get("results")
                    .and_then(Value::as_array)
                    .ok_or_else(|| {
                        "structured-test-results-results must be an array".to_string()
                    })?;
                let mut results = Vec::with_capacity(rows.len());
                for (index, row) in rows.iter().enumerate() {
                    let row = row.as_object().ok_or_else(|| {
                        format!("structured-test-results-results[{index}] must be an object")
                    })?;
                    results.push(parse_result_row(row, index, schema).map_err(|error| {
                        format!("structured-test-results-results[{index}]: {error}")
                    })?);
                }
                validate_results(executed_tests, &results)?;
                let selection = if schema == SKIPPED_RESULTS_SCHEMA {
                    match object.get("selection") {
                        Some(Value::Null) => None,
                        Some(value) => Some(parse_selection(value)?),
                        None => None,
                    }
                } else {
                    None
                };
                Ok(Self {
                    executed_tests,
                    filtered_tests,
                    results: Some(results),
                    selection,
                })
            }
            other => Err(format!(
                "structured-test-results-schema: unsupported schema {other}"
            )),
        }
    }

    /// Read the exact wire schema declared by the owning DAG step.
    pub fn from_declared_schema_json_slice(
        bytes: &[u8],
        declared_schema: u64,
    ) -> Result<Self, String> {
        if declared_schema != CURRENT_SCHEMA
            && declared_schema != CLASSIFIED_RESULTS_SCHEMA
            && declared_schema != DIAGNOSTIC_RESULTS_SCHEMA
            && declared_schema != SKIPPED_RESULTS_SCHEMA
        {
            return Err(format!(
                "structured-test-results declaration has unsupported schema {declared_schema}; expected default schema {CURRENT_SCHEMA}, classified schema {CLASSIFIED_RESULTS_SCHEMA}, diagnostic schema {DIAGNOSTIC_RESULTS_SCHEMA} or skipped schema {SKIPPED_RESULTS_SCHEMA}"
            ));
        }
        let value = parse_unique_json_value(bytes)?;
        let object = value
            .as_object()
            .ok_or_else(|| "structured-test-results must be an object".to_string())?;
        let observed_schema = required_u64(object, "schema")?;
        if observed_schema != declared_schema {
            return Err(format!(
                "structured-test-results-schema: declaration requires schema {declared_schema}, got {observed_schema}"
            ));
        }
        Self::from_json_slice(bytes)
    }

    /// Serialize the current shape, refusing retained count-only evidence.
    pub fn to_current_json(&self) -> Result<Vec<u8>, String> {
        require_no_selection(self)?;
        let results = self.results.as_ref().ok_or_else(|| {
            "structured-test-results-schema: retained schema 1 has no current write path"
                .to_string()
        })?;
        validate_results(self.executed_tests, results)?;
        for result in results {
            require_legacy_result(result)?;
        }
        let rows = results
            .iter()
            .map(|result| {
                serde_json::json!({
                    "id": result.id,
                    "result": if result.passed { "pass" } else { "fail" },
                    "attempts": result.attempts,
                })
            })
            .collect::<Vec<_>>();
        serde_json::to_vec(&serde_json::json!({
            "schema": CURRENT_SCHEMA,
            "executed_tests": self.executed_tests,
            "filtered_tests": self.filtered_tests,
            "results": rows,
        }))
        .map_err(|error| format!("structured-test-results-json: {error}"))
    }

    /// Serialize complete opt-in schema-3 evidence without inventing causes.
    pub fn to_classified_json(&self) -> Result<Vec<u8>, String> {
        require_no_selection(self)?;
        let results = self.results.as_ref().ok_or_else(|| {
            "structured-test-results-schema: retained schema 1 has no current write path"
                .to_string()
        })?;
        validate_results(self.executed_tests, results)?;
        let rows = results
            .iter()
            .map(|result| -> Result<Value, String> {
                validate_result(result, true)?;
                require_blocking_result(result)?;
                let attempts = result.attempt_results.as_ref();
                Ok(serde_json::json!({
                    "id": result.id,
                    "result": if result.passed { "pass" } else { "fail" },
                    "attempts": result.attempts,
                    "attempt_results": attempts.map(|attempts| attempts.iter().map(|attempt| serde_json::json!({
                        "attempt": attempt.attempt,
                        "outcome": attempt.outcome.value(),
                        "detail": attempt.detail,
                    })).collect::<Vec<_>>()),
                }))
            })
            .collect::<Result<Vec<_>, _>>()?;
        serde_json::to_vec(&serde_json::json!({
            "schema": CLASSIFIED_RESULTS_SCHEMA,
            "executed_tests": self.executed_tests,
            "filtered_tests": self.filtered_tests,
            "results": rows,
        }))
        .map_err(|error| format!("structured-test-results-json: {error}"))
    }

    /// Serialize complete opt-in schema-4 evidence, diagnostic reasons included.
    pub fn to_diagnostic_json(&self) -> Result<Vec<u8>, String> {
        require_no_selection(self)?;
        let results = self.results.as_ref().ok_or_else(|| {
            "structured-test-results-schema: retained schema 1 has no current write path"
                .to_string()
        })?;
        validate_results(self.executed_tests, results)?;
        let rows = results
            .iter()
            .map(|result| -> Result<Value, String> {
                validate_result(result, true)?;
                require_no_skip_result(result)?;
                let attempts = result.attempt_results.as_ref();
                Ok(serde_json::json!({
                    "id": result.id,
                    "result": result.wire_result(),
                    "attempts": result.attempts,
                    "attempt_results": attempts.map(|attempts| attempts.iter().map(|attempt| serde_json::json!({
                        "attempt": attempt.attempt,
                        "outcome": attempt.outcome.value(),
                        "detail": attempt.detail,
                    })).collect::<Vec<_>>()),
                    "diagnostic_reason": result.diagnostic,
                }))
            })
            .collect::<Result<Vec<_>, _>>()?;
        serde_json::to_vec(&serde_json::json!({
            "schema": DIAGNOSTIC_RESULTS_SCHEMA,
            "executed_tests": self.executed_tests,
            "filtered_tests": self.filtered_tests,
            "results": rows,
        }))
        .map_err(|error| format!("structured-test-results-json: {error}"))
    }

    /// Serialize complete opt-in schema-5 evidence, skip reasons included.
    pub fn to_skipped_json(&self) -> Result<Vec<u8>, String> {
        let results = self.results.as_ref().ok_or_else(|| {
            "structured-test-results-schema: retained schema 1 has no current write path"
                .to_string()
        })?;
        validate_results(self.executed_tests, results)?;
        let rows = results
            .iter()
            .map(|result| -> Result<Value, String> {
                validate_result(result, true)?;
                let attempts = result.attempt_results.as_ref();
                Ok(serde_json::json!({
                    "id": result.id,
                    "result": result.wire_result(),
                    "attempts": result.attempts,
                    "attempt_results": attempts.map(|attempts| attempts.iter().map(|attempt| serde_json::json!({
                        "attempt": attempt.attempt,
                        "outcome": attempt.outcome.value(),
                        "detail": attempt.detail,
                    })).collect::<Vec<_>>()),
                    "diagnostic_reason": result.diagnostic,
                    "skip_reason": result.skip_reason,
                }))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(selection) = self.selection.as_ref() {
            validate_selection(selection)?;
        }
        serde_json::to_vec(&serde_json::json!({
            "schema": SKIPPED_RESULTS_SCHEMA,
            "executed_tests": self.executed_tests,
            "filtered_tests": self.filtered_tests,
            "results": rows,
            "selection": self.selection.as_ref().map(selection_to_json),
        }))
        .map_err(|error| format!("structured-test-results-json: {error}"))
    }

    /// Atomically publish the current shape at the scheduler-owned path.
    pub fn write_current(&self, path: &Path) -> Result<(), String> {
        self.write_current_typed(path)
            .map_err(|error| error.to_string())
    }

    /// Atomically publish explicitly classified schema-3 results.
    pub fn write_classified(&self, path: &Path) -> Result<(), String> {
        self.write_classified_typed(path)
            .map_err(|error| error.to_string())
    }

    /// Atomically publish schema 2, retaining validation versus filesystem failure.
    pub fn write_current_typed(&self, path: &Path) -> Result<(), TestResultsWriteError> {
        let bytes = self
            .to_current_json()
            .map_err(TestResultsWriteError::Invalid)?;
        self.write_bytes(path, bytes)
    }

    /// Atomically publish schema 3, retaining validation versus filesystem failure.
    pub fn write_classified_typed(&self, path: &Path) -> Result<(), TestResultsWriteError> {
        let bytes = self
            .to_classified_json()
            .map_err(TestResultsWriteError::Invalid)?;
        self.write_bytes(path, bytes)
    }

    /// Atomically publish schema 4, retaining validation versus filesystem failure.
    pub fn write_diagnostic_typed(&self, path: &Path) -> Result<(), TestResultsWriteError> {
        let bytes = self
            .to_diagnostic_json()
            .map_err(TestResultsWriteError::Invalid)?;
        self.write_bytes(path, bytes)
    }

    /// Atomically publish schema 5, retaining validation versus filesystem failure.
    pub fn write_skipped_typed(&self, path: &Path) -> Result<(), TestResultsWriteError> {
        let bytes = self
            .to_skipped_json()
            .map_err(TestResultsWriteError::Invalid)?;
        self.write_bytes(path, bytes)
    }

    fn write_bytes(&self, path: &Path, bytes: Vec<u8>) -> Result<(), TestResultsWriteError> {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| {
                TestResultsWriteError::Invalid(
                    "structured-test-results-path has no UTF-8 file name".to_string(),
                )
            })?;
        let temporary = path.with_file_name(format!(".{file_name}.tmp-{}", std::process::id()));
        fs::write(&temporary, bytes).map_err(|source| TestResultsWriteError::Write {
            path: temporary.clone(),
            source,
        })?;
        if let Err(source) = fs::rename(&temporary, path) {
            let _ = fs::remove_file(&temporary);
            return Err(TestResultsWriteError::Publish {
                path: path.to_path_buf(),
                source,
            });
        }
        Ok(())
    }
}

fn required_u64(object: &Map<String, Value>, field: &str) -> Result<u64, String> {
    object
        .get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("structured-test-results-{field} must be an unsigned integer"))
}

fn exact_fields(object: &Map<String, Value>, expected: &[&str]) -> Result<(), String> {
    let actual = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let expected = expected.iter().copied().collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(format!(
            "structured-test-results-fields: expected {expected:?}, found {actual:?}"
        ));
    }
    Ok(())
}

fn validate_results(executed_tests: u64, results: &[TestResult]) -> Result<(), String> {
    let terminal_rows = u64::try_from(results.len())
        .map_err(|_| "structured-test-results-results length does not fit u64".to_string())?;
    if terminal_rows != executed_tests {
        return Err(format!(
            "structured-test-results-results has {terminal_rows} terminal row(s), expected exactly {executed_tests} executed test(s)"
        ));
    }
    let mut ids = BTreeSet::new();
    for result in results {
        validate_result(result, false)?;
        if !ids.insert(result.id.as_str()) {
            return Err(format!(
                "structured-test-results-id is duplicated: {:?}",
                result.id
            ));
        }
    }

    Ok(())
}

fn require_legacy_result(result: &TestResult) -> Result<(), String> {
    if result.attempt_results.is_some() {
        return Err(
            "structured-test-results schema-2 writer refuses to discard classified attempts".into(),
        );
    }
    require_blocking_result(result)
}

/// Schemas 2 and 3 cannot say a failure is non-blocking. Writing a diagnostic failure as a plain
/// `fail` would silently change its meaning, so those writers refuse instead.
fn require_blocking_result(result: &TestResult) -> Result<(), String> {
    if result.diagnostic.is_some() {
        return Err(format!(
            "structured-test-results schema {RETAINED_RESULTS_SCHEMA}/{CLASSIFIED_RESULTS_SCHEMA} writer refuses to discard the diagnostic designation of {:?}; write schema {DIAGNOSTIC_RESULTS_SCHEMA}",
            result.id
        ));
    }
    require_no_skip_result(result)
}

/// Schemas before 5 cannot say a test skipped. Writing a skip as a plain `fail` (or
/// dropping the reason) would silently change its meaning, so those writers refuse instead.
fn require_no_selection(results: &TestResults) -> Result<(), String> {
    if results.selection.is_some() {
        return Err(format!(
            "structured-test-results writer refuses to discard the run selection; write schema {SKIPPED_RESULTS_SCHEMA}"
        ));
    }
    Ok(())
}

fn require_no_skip_result(result: &TestResult) -> Result<(), String> {
    if result.skip_reason.is_some() {
        return Err(format!(
            "structured-test-results writer refuses to discard the skip designation of {:?}; write schema {SKIPPED_RESULTS_SCHEMA}",
            result.id
        ));
    }
    Ok(())
}

fn validate_result(result: &TestResult, require_attempt_results: bool) -> Result<(), String> {
    TestResult::new(result.id.clone(), result.passed, result.attempts)?;
    if let Some(reason) = result.diagnostic.as_deref() {
        if result.passed {
            return Err(format!(
                "structured-test-results diagnostic designation on passing test {:?}; a pass is an ordinary pass row",
                result.id
            ));
        }
        if reason.is_empty() || reason.trim() != reason {
            return Err(format!(
                "structured-test-results diagnostic failure {:?} requires a nonempty trimmed reason",
                result.id
            ));
        }
        let Some(attempts) = result.attempt_results.as_ref() else {
            return Err(format!(
                "structured-test-results diagnostic failure {:?} lacks classified attempt_results",
                result.id
            ));
        };
        // Only a MEASURED failure may be excused. A run that was cancelled, could not produce
        // trustworthy evidence, or produced no result has no product verdict to call diagnostic.
        let terminal = attempts.last().map(|attempt| attempt.outcome);
        if !matches!(
            terminal,
            Some(
                TestAttemptOutcome::Failed
                    | TestAttemptOutcome::CpuTimeout
                    | TestAttemptOutcome::WallTimeout
            )
        ) {
            return Err(format!(
                "structured-test-results diagnostic failure {:?} must end in a measured failure (failed, cpu_timeout or wall_timeout), not {:?}",
                result.id, terminal
            ));
        }
    }
    if result.diagnostic.is_some() && result.skip_reason.is_some() {
        return Err(format!(
            "structured-test-results test {:?} is both a diagnostic failure and a skip; a row has exactly one terminal classification",
            result.id
        ));
    }
    if let Some(reason) = result.skip_reason.as_deref() {
        if result.passed {
            return Err(format!(
                "structured-test-results skip designation on passing test {:?}; a pass is an ordinary pass row",
                result.id
            ));
        }
        if reason.is_empty() || reason.trim() != reason {
            return Err(format!(
                "structured-test-results skip {:?} requires a nonempty trimmed reason",
                result.id
            ));
        }
        let Some(attempts) = result.attempt_results.as_ref() else {
            return Err(format!(
                "structured-test-results skip {:?} lacks classified attempt_results",
                result.id
            ));
        };
        let terminal = attempts.last().map(|attempt| attempt.outcome);
        if terminal != Some(TestAttemptOutcome::Skipped) {
            return Err(format!(
                "structured-test-results skip {:?} must end in a skipped attempt, not {:?}",
                result.id, terminal
            ));
        }
    }
    let Some(attempts) = result.attempt_results.as_ref() else {
        if require_attempt_results {
            return Err("structured-test-results classified row lacks attempt_results".into());
        }
        return Ok(());
    };
    if result.skip_reason.is_none()
        && attempts.last().map(|attempt| attempt.outcome) == Some(TestAttemptOutcome::Skipped)
    {
        return Err(format!(
            "structured-test-results test {:?} ends in a skipped attempt without a skip designation; a skip must carry its reason",
            result.id
        ));
    }
    let attempt_count = u64::try_from(attempts.len())
        .map_err(|_| "structured-test-results attempt count does not fit u64".to_string())?;
    if attempt_count != result.attempts {
        return Err(format!(
            "structured-test-results has {attempt_count} attempt result(s), expected {}",
            result.attempts
        ));
    }
    for (index, attempt) in attempts.iter().enumerate() {
        let expected = u64::try_from(index + 1)
            .map_err(|_| "structured-test-results attempt index does not fit u64".to_string())?;
        TestAttemptResult::new(attempt.attempt, attempt.outcome, attempt.detail.clone())?;
        if attempt.outcome == TestAttemptOutcome::Passed && index + 1 != attempts.len() {
            return Err(format!(
                "structured-test-results attempt {} passed before the terminal attempt",
                attempt.attempt
            ));
        }
        if attempt.attempt != expected {
            return Err(format!(
                "structured-test-results attempt sequence expected {expected}, found {}",
                attempt.attempt
            ));
        }
    }
    let terminal_passed = attempts
        .last()
        .is_some_and(|attempt| attempt.outcome == TestAttemptOutcome::Passed);
    if result.passed != terminal_passed {
        return Err(format!(
            "structured-test-results terminal pass {} disagrees with final attempt outcome {:?}",
            result.passed,
            attempts.last().map(|attempt| attempt.outcome)
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_shape_round_trips_and_keeps_retry_count() {
        let report = TestResults::current(
            3,
            7,
            vec![
                TestResult::new("suite$passes".into(), true, 1).unwrap(),
                TestResult::new("suite$recovers".into(), true, 2).unwrap(),
                TestResult::new("suite$fails".into(), false, 1).unwrap(),
            ],
        )
        .unwrap();
        let bytes = report.to_current_json().unwrap();
        assert_eq!(TestResults::from_json_slice(&bytes).unwrap(), report);
    }

    #[test]
    fn retained_counts_remain_readable_but_have_no_current_write_path() {
        let retained =
            TestResults::from_json_slice(br#"{"schema":1,"executed_tests":7,"filtered_tests":11}"#)
                .unwrap();
        assert_eq!(retained.results, None);
        assert!(retained
            .to_current_json()
            .unwrap_err()
            .contains("retained schema 1 has no current write path"));
    }

    #[test]
    fn malformed_current_results_fail_by_field_name() {
        let missing = br#"{"schema":2,"executed_tests":1,"filtered_tests":0,"results":[{"id":"suite$case","result":"pass"}]}"#;
        assert!(TestResults::from_json_slice(missing)
            .unwrap_err()
            .contains("structured-test-results-fields"));
        let unknown = br#"{"schema":2,"executed_tests":1,"filtered_tests":0,"results":[{"id":"suite$case","result":"maybe","attempts":1}]}"#;
        assert!(TestResults::from_json_slice(unknown)
            .unwrap_err()
            .contains(".result has unknown value"));
        let incomplete = br#"{"schema":2,"executed_tests":2,"filtered_tests":0,"results":[{"id":"suite$case","result":"pass","attempts":1}]}"#;
        assert!(TestResults::from_json_slice(incomplete)
            .unwrap_err()
            .contains("1 terminal row(s), expected exactly 2 executed test(s)"));
        let duplicate = br#"{"schema":2,"executed_tests":2,"filtered_tests":0,"results":[{"id":"suite$case","result":"pass","attempts":1},{"id":"suite$case","result":"fail","attempts":1}]}"#;
        assert!(TestResults::from_json_slice(duplicate)
            .unwrap_err()
            .contains("structured-test-results-id is duplicated"));
        let extra = br#"{"schema":2,"executed_tests":1,"filtered_tests":0,"results":[{"id":"suite$one","result":"pass","attempts":1},{"id":"suite$two","result":"pass","attempts":1}]}"#;
        assert!(TestResults::from_json_slice(extra)
            .unwrap_err()
            .contains("2 terminal row(s), expected exactly 1 executed test(s)"));
    }
}

#[cfg(test)]
mod publication_tests {
    use super::*;

    #[test]
    fn typed_publication_keeps_validation_atomicity_and_original_io_errors() {
        let dir = std::env::temp_dir().join(format!(
            "dagrun-typed-publication-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir(&dir).unwrap();
        let current = TestResults::current(
            1,
            2,
            vec![TestResult::new("suite$case".into(), false, 1).unwrap()],
        )
        .unwrap();
        let classified = TestResults::classified(
            1,
            2,
            vec![TestResult::with_attempt_results(
                "suite$case".into(),
                false,
                vec![
                    TestAttemptResult::new(1, TestAttemptOutcome::Failed, Some("exit 17".into()))
                        .unwrap(),
                ],
            )
            .unwrap()],
        )
        .unwrap();
        for (name, report, schema) in [("current", current, 2), ("classified", classified, 3)] {
            let typed_write = |report: &TestResults, path: &Path| match schema {
                2 => report.write_current_typed(path),
                _ => report.write_classified_typed(path),
            };
            let legacy_write = |report: &TestResults, path: &Path| match schema {
                2 => report.write_current(path),
                _ => report.write_classified(path),
            };
            let output = dir.join(name);
            typed_write(&report, &output).unwrap();
            let bytes = fs::read(&output).unwrap();
            assert_eq!(
                TestResults::from_declared_schema_json_slice(&bytes, schema).unwrap(),
                report
            );
            legacy_write(&report, &output).unwrap();
            assert_eq!(fs::read(&output).unwrap(), bytes);

            // A genuine fs::write failure: its parent does not exist.
            let missing = dir.join("missing").join(name);
            let error = typed_write(&report, &missing).unwrap_err();
            let TestResultsWriteError::Write { path, source } = &error else {
                panic!("expected the original write I/O error: {error:?}");
            };
            assert_eq!(source.kind(), io::ErrorKind::NotFound);
            assert_eq!(path.parent(), missing.parent());
            assert!(std::error::Error::source(&error).is_some());
            assert_eq!(
                legacy_write(&report, &missing).unwrap_err(),
                error.to_string()
            );
            assert!(!missing.exists());

            // A real atomic publication failure, independent of uid permission bypasses.
            let destination = dir.join(format!("{name}-directory"));
            fs::create_dir(&destination).unwrap();
            let error = typed_write(&report, &destination).unwrap_err();
            let TestResultsWriteError::Publish { path, source } = &error else {
                panic!("expected the original rename I/O error: {error:?}");
            };
            assert_eq!(path, &destination);
            assert!(source.raw_os_error().is_some());
            assert_eq!(
                legacy_write(&report, &destination).unwrap_err(),
                error.to_string()
            );
            assert!(destination.is_dir());
            assert_eq!(fs::read_dir(&destination).unwrap().count(), 0);
            let temporary = dir.join(format!(".{name}-directory.tmp-{}", std::process::id()));
            assert!(
                !temporary.exists(),
                "failed rename must retain existing cleanup"
            );

            // Invalid evidence must refuse BEFORE either inaccessible destination is touched.
            let mut invalid = report.clone();
            invalid.executed_tests = 2;
            let error = typed_write(&invalid, &missing).unwrap_err();
            assert!(matches!(&error, TestResultsWriteError::Invalid(_)));
            assert!(error
                .to_string()
                .contains("expected exactly 2 executed test(s)"));
            assert_eq!(
                legacy_write(&invalid, &missing).unwrap_err(),
                error.to_string()
            );
            assert!(std::error::Error::source(&error).is_none());
            assert_eq!(
                fs::read(&output).unwrap(),
                bytes,
                "prior evidence must survive"
            );
            let error = typed_write(&report, Path::new("")).unwrap_err();
            assert!(matches!(error, TestResultsWriteError::Invalid(_)));
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn retained_import_kinds_do_not_invent_authority_for_unknown_values() {
        for kind in [
            TestResultsErrorKind::Missing,
            TestResultsErrorKind::ReadIo,
            TestResultsErrorKind::InvalidReport,
        ] {
            assert_eq!(TestResultsErrorKind::from_value(kind.value()), Some(kind));
        }
        for unknown in ["", "future", "Missing", "missing; last output: read_io"] {
            assert_eq!(TestResultsErrorKind::from_value(unknown), None);
        }
    }
}

#[cfg(test)]
mod classified_tests {
    use super::*;

    #[test]
    fn current_shape_round_trips_and_keeps_retry_count() {
        let report = TestResults::classified(
            3,
            7,
            vec![
                TestResult::with_attempt_results(
                    "suite$passes".into(),
                    true,
                    vec![TestAttemptResult::new(1, TestAttemptOutcome::Passed, None).unwrap()],
                )
                .unwrap(),
                TestResult::with_attempt_results(
                    "suite$recovers".into(),
                    true,
                    vec![
                        TestAttemptResult::new(
                            1,
                            TestAttemptOutcome::CpuTimeout,
                            Some("cpu 22s".into()),
                        )
                        .unwrap(),
                        TestAttemptResult::new(2, TestAttemptOutcome::Passed, None).unwrap(),
                    ],
                )
                .unwrap(),
                TestResult::with_attempt_results(
                    "suite$fails".into(),
                    false,
                    vec![TestAttemptResult::new(
                        1,
                        TestAttemptOutcome::Failed,
                        Some("exit 7".into()),
                    )
                    .unwrap()],
                )
                .unwrap(),
            ],
        )
        .unwrap();
        let bytes = report.to_classified_json().unwrap();
        assert_eq!(TestResults::from_json_slice(&bytes).unwrap(), report);
    }

    #[test]
    fn retained_counts_remain_readable_but_have_no_current_write_path() {
        let retained =
            TestResults::from_json_slice(br#"{"schema":1,"executed_tests":7,"filtered_tests":11}"#)
                .unwrap();
        assert_eq!(retained.results, None);
        assert!(retained
            .to_classified_json()
            .unwrap_err()
            .contains("retained schema 1 has no current write path"));
    }

    #[test]
    fn malformed_current_results_fail_by_field_name() {
        let missing = br#"{"schema":3,"executed_tests":1,"filtered_tests":0,"results":[{"id":"suite$case","result":"pass","attempts":1}]}"#;
        assert!(TestResults::from_json_slice(missing)
            .unwrap_err()
            .contains("structured-test-results-fields"));
        let unknown = br#"{"schema":3,"executed_tests":1,"filtered_tests":0,"results":[{"id":"suite$case","result":"maybe","attempts":1,"attempt_results":[]}]}"#;
        assert!(TestResults::from_json_slice(unknown)
            .unwrap_err()
            .contains(".result has unknown value"));
        let incomplete = br#"{"schema":3,"executed_tests":2,"filtered_tests":0,"results":[{"id":"suite$case","result":"pass","attempts":1,"attempt_results":[{"attempt":1,"outcome":"passed","detail":null}]}]}"#;
        assert!(TestResults::from_json_slice(incomplete)
            .unwrap_err()
            .contains("1 terminal row(s), expected exactly 2 executed test(s)"));
        let duplicate = br#"{"schema":3,"executed_tests":2,"filtered_tests":0,"results":[{"id":"suite$case","result":"pass","attempts":1,"attempt_results":[{"attempt":1,"outcome":"passed","detail":null}]},{"id":"suite$case","result":"fail","attempts":1,"attempt_results":[{"attempt":1,"outcome":"failed","detail":"exit 1"}]}]}"#;
        assert!(TestResults::from_json_slice(duplicate)
            .unwrap_err()
            .contains("structured-test-results-id is duplicated"));
        let extra = br#"{"schema":3,"executed_tests":1,"filtered_tests":0,"results":[{"id":"suite$one","result":"pass","attempts":1,"attempt_results":[{"attempt":1,"outcome":"passed","detail":null}]},{"id":"suite$two","result":"pass","attempts":1,"attempt_results":[{"attempt":1,"outcome":"passed","detail":null}]}]}"#;
        assert!(TestResults::from_json_slice(extra)
            .unwrap_err()
            .contains("2 terminal row(s), expected exactly 1 executed test(s)"));
        let after_pass = br#"{"schema":3,"executed_tests":1,"filtered_tests":0,"results":[{"id":"suite$case","result":"fail","attempts":2,"attempt_results":[{"attempt":1,"outcome":"passed","detail":null},{"attempt":2,"outcome":"failed","detail":"impossible retry"}]}]}"#;
        assert!(TestResults::from_json_slice(after_pass)
            .unwrap_err()
            .contains("passed before the terminal attempt"));
        let malformed = br#"{"schema":3,"executed_tests":1,"filtered_tests":0,"results":[{"id":"suite$case","result":"fail","attempts":1,"attempt_results":[{"attempt":1,"outcome":"cpu_timeout","detail":null}]}]}"#;
        assert!(TestResults::from_json_slice(malformed)
            .unwrap_err()
            .contains("requires nonempty trimmed detail"));
    }

    #[test]
    fn current_schema_refuses_duplicate_keys_at_every_object_depth_in_both_orders() {
        let cases: &[(&str, &[u8])] = &[
            (
                "top-level retained then current schema",
                br#"{"schema":2,"schema":3,"executed_tests":1,"filtered_tests":0,"results":[{"id":"suite$case","result":"pass","attempts":1,"attempt_results":[{"attempt":1,"outcome":"passed","detail":null}]}]}"#,
            ),
            (
                "top-level current then retained schema",
                br#"{"schema":3,"schema":2,"executed_tests":1,"filtered_tests":0,"results":[{"id":"suite$case","result":"pass","attempts":1,"attempt_results":[{"attempt":1,"outcome":"passed","detail":null}]}]}"#,
            ),
            (
                "result failure overwritten by pass",
                br#"{"schema":3,"executed_tests":1,"filtered_tests":0,"results":[{"id":"suite$case","result":"fail","result":"pass","attempts":1,"attempt_results":[{"attempt":1,"outcome":"passed","detail":null}]}]}"#,
            ),
            (
                "result pass overwritten by failure",
                br#"{"schema":3,"executed_tests":1,"filtered_tests":0,"results":[{"id":"suite$case","result":"pass","result":"fail","attempts":1,"attempt_results":[{"attempt":1,"outcome":"failed","detail":"exit 7"}]}]}"#,
            ),
            (
                "attempt failure overwritten by pass",
                br#"{"schema":3,"executed_tests":1,"filtered_tests":0,"results":[{"id":"suite$case","result":"pass","attempts":1,"attempt_results":[{"attempt":1,"outcome":"failed","outcome":"passed","detail":null}]}]}"#,
            ),
            (
                "attempt pass overwritten by failure",
                br#"{"schema":3,"executed_tests":1,"filtered_tests":0,"results":[{"id":"suite$case","result":"fail","attempts":1,"attempt_results":[{"attempt":1,"outcome":"passed","outcome":"failed","detail":"exit 7"}]}]}"#,
            ),
        ];
        for (name, bytes) in cases {
            let error = TestResults::from_json_slice(bytes).unwrap_err();
            assert!(error.contains("duplicate key"), "{name}: {error}");
        }
    }

    #[test]
    fn strict_json_reader_still_refuses_trailing_input() {
        let bytes = br#"{"schema":3,"executed_tests":1,"filtered_tests":0,"results":[{"id":"suite$case","result":"pass","attempts":1,"attempt_results":[{"attempt":1,"outcome":"passed","detail":null}]}]} {}"#;
        let error = TestResults::from_json_slice(bytes).unwrap_err();
        assert!(
            error.contains("trailing characters"),
            "the duplicate-safe decoder must retain serde_json's trailing-input refusal: {error}"
        );
    }

    #[test]
    fn current_schema_refuses_missing_attempt_results() {
        let retained = TestResult::new("suite$case".into(), false, 1).unwrap();
        let error = TestResults::classified(1, 0, vec![retained.clone()]).unwrap_err();
        assert!(error.contains("classified row lacks attempt_results"));

        let report = TestResults {
            executed_tests: 1,
            filtered_tests: 0,
            results: Some(vec![retained]),
            selection: None,
        };
        assert!(report
            .to_classified_json()
            .unwrap_err()
            .contains("classified row lacks attempt_results"));
        let bytes = br#"{"schema":3,"executed_tests":1,"filtered_tests":0,"results":[{"id":"suite$case","result":"fail","attempts":1,"attempt_results":null}]}"#;
        assert!(TestResults::from_json_slice(bytes)
            .unwrap_err()
            .contains("must be an array for classified schema"));
    }
}

#[cfg(test)]
mod additive_tests {
    use super::*;

    fn classified_row() -> Value {
        serde_json::json!({"schema":3,"executed_tests":1,"filtered_tests":7,"results":[{
            "id":"suite$retry","result":"pass","attempts":2,"attempt_results":[
                {"attempt":1,"outcome":"cpu_timeout","detail":"used 22 seconds CPU"},
                {"attempt":2,"outcome":"passed","detail":null}
            ]
        }]})
    }

    #[test]
    fn default_schema_and_legacy_bytes_are_unchanged() {
        assert_eq!(CURRENT_SCHEMA, 2);
        assert_eq!(RETAINED_RESULTS_SCHEMA, 2);
        assert_eq!(CLASSIFIED_RESULTS_SCHEMA, 3);
        let report = TestResults::current(
            2,
            7,
            vec![
                TestResult::new("a".into(), true, 2).unwrap(),
                TestResult::new("b".into(), false, u64::MAX).unwrap(),
            ],
        )
        .unwrap();
        // Workspace dependencies can enable serde_json's preserve_order feature.
        // Both wire forms below match the pre-classified writer under the same
        // feature set. Select independently of the result serializer, then
        // require one exact byte sequence rather than accepting either form.
        let mut order_probe = serde_json::Map::new();
        order_probe.insert("z".into(), Value::Null);
        order_probe.insert("a".into(), Value::Null);
        let keys = order_probe.keys().map(String::as_str).collect::<Vec<_>>();
        let (expected, empty_legacy, empty_classified): (&[u8], &[u8], &[u8]) =
            match keys.as_slice() {
                ["a", "z"] => (
                    br#"{"executed_tests":2,"filtered_tests":7,"results":[{"attempts":2,"id":"a","result":"pass"},{"attempts":18446744073709551615,"id":"b","result":"fail"}],"schema":2}"#,
                    br#"{"executed_tests":0,"filtered_tests":7,"results":[],"schema":2}"#,
                    br#"{"executed_tests":0,"filtered_tests":7,"results":[],"schema":3}"#,
                ),
                ["z", "a"] => (
                    br#"{"schema":2,"executed_tests":2,"filtered_tests":7,"results":[{"id":"a","result":"pass","attempts":2},{"id":"b","result":"fail","attempts":18446744073709551615}]}"#,
                    br#"{"schema":2,"executed_tests":0,"filtered_tests":7,"results":[]}"#,
                    br#"{"schema":3,"executed_tests":0,"filtered_tests":7,"results":[]}"#,
                ),
                other => panic!("unknown serde_json map order: {other:?}"),
            };
        assert_eq!(report.to_current_json().unwrap(), expected);
        assert_eq!(TestResults::from_json_slice(expected).unwrap(), report);
        assert!(report
            .results
            .as_ref()
            .unwrap()
            .iter()
            .all(|row| row.attempt_results.is_none()));
        assert_eq!(
            TestResults::current(0, 7, vec![])
                .unwrap()
                .to_current_json()
                .unwrap(),
            empty_legacy
        );
        assert_eq!(
            TestResults::classified(0, 7, vec![])
                .unwrap()
                .to_classified_json()
                .unwrap(),
            empty_classified
        );
    }

    #[test]
    fn writers_and_declarations_refuse_implicit_conversion() {
        let legacy = br#"{"schema":2,"executed_tests":1,"filtered_tests":7,"results":[{"id":"suite$retry","result":"pass","attempts":2}]}"#;
        let typed = serde_json::to_vec(&classified_row()).unwrap();
        for (bytes, schema, other) in [(legacy.as_slice(), 2, 3), (typed.as_slice(), 3, 2)] {
            assert!(TestResults::from_declared_schema_json_slice(bytes, schema).is_ok());
            let error = TestResults::from_declared_schema_json_slice(bytes, other).unwrap_err();
            assert!(
                error.contains(&format!(
                    "declaration requires schema {other}, got {schema}"
                )),
                "{error}"
            );
        }
        let legacy = TestResults::from_json_slice(legacy).unwrap();
        assert!(legacy
            .to_classified_json()
            .unwrap_err()
            .contains("lacks attempt_results"));
        assert!(TestResults::classified(1, 7, legacy.results.unwrap()).is_err());
        let typed = TestResults::from_json_slice(&typed).unwrap();
        assert!(typed
            .to_current_json()
            .unwrap_err()
            .contains("refuses to discard"));
        assert!(TestResults::current(1, 7, typed.results.clone().unwrap()).is_err());
        assert_eq!(
            typed.executed_tests, 1,
            "attempts do not inflate the executed denominator"
        );
        assert_eq!(typed.results.unwrap()[0].attempts, 2);
        let counts = br#"{"schema":1,"executed_tests":9,"filtered_tests":11}"#;
        for declared in [1, 2, 3, 4] {
            assert!(TestResults::from_declared_schema_json_slice(counts, declared).is_err());
        }
    }

    #[test]
    fn each_nonpass_requires_real_detail_and_keeps_its_cause() {
        for outcome in [
            "failed",
            "cpu_timeout",
            "wall_timeout",
            "cancelled",
            "infrastructure_error",
            "no_result",
        ] {
            let mut wire = classified_row();
            wire["results"][0]["result"] = "fail".into();
            wire["results"][0]["attempt_results"][1]["outcome"] = outcome.into();
            wire["results"][0]["attempt_results"][1]["detail"] = "specific observed cause".into();
            let parsed = TestResults::from_json_slice(&serde_json::to_vec(&wire).unwrap()).unwrap();
            assert_eq!(
                parsed.results.unwrap()[0].attempt_results.as_ref().unwrap()[1]
                    .outcome
                    .value(),
                outcome
            );
            for bad in [
                Value::Null,
                "".into(),
                " ".into(),
                " detail".into(),
                "detail ".into(),
                7.into(),
                false.into(),
            ] {
                wire["results"][0]["attempt_results"][1]["detail"] = bad;
                assert!(
                    TestResults::from_json_slice(&serde_json::to_vec(&wire).unwrap()).is_err(),
                    "{wire}"
                );
            }
            wire["results"][0]["attempt_results"][1]
                .as_object_mut()
                .unwrap()
                .remove("detail");
            assert!(TestResults::from_json_slice(&serde_json::to_vec(&wire).unwrap()).is_err());
        }
    }

    #[test]
    fn inconsistent_or_ambiguous_attempt_histories_are_refused() {
        let mutate = |change: fn(&mut Value)| {
            let mut wire = classified_row();
            change(&mut wire);
            assert!(
                TestResults::from_json_slice(&serde_json::to_vec(&wire).unwrap()).is_err(),
                "{wire}"
            );
        };
        mutate(|w| w["results"][0]["attempt_results"][0]["outcome"] = "unknown".into());
        mutate(|w| w["results"][0]["attempt_results"][0]["extra"] = true.into());
        mutate(|w| w["results"][0]["attempt_results"][1]["detail"] = "pass with detail".into());
        mutate(|w| w["results"][0]["attempt_results"][1]["attempt"] = 1.into());
        mutate(|w| w["results"][0]["attempt_results"][1]["attempt"] = 3.into());
        mutate(|w| {
            w["results"][0]["attempt_results"]
                .as_array_mut()
                .unwrap()
                .reverse()
        });
        mutate(|w| w["results"][0]["attempts"] = 3.into());
        mutate(|w| w["results"][0]["attempt_results"] = serde_json::json!([]));
        mutate(|w| w["results"][0]["result"] = "fail".into());
        mutate(|w| {
            w["results"][0]["attempt_results"][0]["outcome"] = "passed".into();
            w["results"][0]["attempt_results"][0]["detail"] = Value::Null;
        });
        for number in [0.into(), (-1).into(), 1.5.into(), true.into()] {
            let mut wire = classified_row();
            wire["results"][0]["attempt_results"][0]["attempt"] = number;
            assert!(TestResults::from_json_slice(&serde_json::to_vec(&wire).unwrap()).is_err());
        }
        for shape in [
            Value::Null,
            false.into(),
            "missing rows".into(),
            serde_json::json!({}),
        ] {
            let mut wire = classified_row();
            wire["results"][0]["attempt_results"] = shape;
            assert!(TestResults::from_json_slice(&serde_json::to_vec(&wire).unwrap()).is_err());
        }
    }

    #[test]
    fn raw_duplicate_keys_refuse_identical_values_and_legacy_ambiguity() {
        for bytes in [
            br#"{"schema":2,"schema":2,"executed_tests":0,"filtered_tests":0,"results":[]}"#.as_slice(),
            br#"{"schema":1,"executed_tests":9,"executed_tests":1,"filtered_tests":0}"#.as_slice(),
            br#"{"schema":2,"executed_tests":1,"filtered_tests":0,"results":[{"id":"a","result":"fail","result":"pass","attempts":1}]}"#.as_slice(),
            br#"{"schema":2,"executed_tests":1,"filtered_tests":0,"results":[{"id":"a","result":"pass","result":"fail","attempts":1}]}"#.as_slice(),
            br#"{"schema":3,"executed_tests":1,"filtered_tests":0,"results":[{"id":"a","result":"fail","attempts":1,"attempt_results":[{"attempt":1,"outcome":"failed","detail":"first","detail":"second"}]}]}"#.as_slice(),
        ] {
            assert!(TestResults::from_json_slice(bytes).unwrap_err().contains("duplicate key"));
        }
    }
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;

    fn failed_attempt() -> Vec<TestAttemptResult> {
        vec![TestAttemptResult::new(1, TestAttemptOutcome::Failed, Some("exit 1".into())).unwrap()]
    }

    fn report() -> TestResults {
        TestResults::diagnostic(
            3,
            2,
            vec![
                TestResult::with_attempt_results(
                    "suite$pass".into(),
                    true,
                    vec![TestAttemptResult::new(1, TestAttemptOutcome::Passed, None).unwrap()],
                )
                .unwrap(),
                TestResult::with_attempt_results("suite$fail".into(), false, failed_attempt())
                    .unwrap(),
                TestResult::diagnostic_failure(
                    "suite$diag".into(),
                    failed_attempt(),
                    "bounded probe; a slow host is not a product loss".into(),
                )
                .unwrap(),
            ],
        )
        .unwrap()
    }

    fn skipped_report() -> TestResults {
        let mut rows = report().results.unwrap();
        rows.push(
            TestResult::skipped(
                "suite$skip".into(),
                vec![TestAttemptResult::new(
                    1,
                    TestAttemptOutcome::Skipped,
                    Some("/dev/kvm unavailable".into()),
                )
                .unwrap()],
                "/dev/kvm unavailable".into(),
            )
            .unwrap(),
        );
        TestResults::skipped(4, 2, rows).unwrap()
    }

    #[test]
    fn skipped_schema_round_trips_and_classifies_each_row() {
        let report = skipped_report();
        let bytes = report.to_skipped_json().unwrap();
        let parsed =
            TestResults::from_declared_schema_json_slice(&bytes, SKIPPED_RESULTS_SCHEMA).unwrap();
        assert_eq!(parsed, report);
        let rows = parsed.results.unwrap();
        let skip = &rows[3];
        assert!(skip.is_skipped());
        assert!(!skip.is_blocking_failure(), "a skip never fails its step");
        assert!(!skip.is_diagnostic_failure());
        assert!(!skip.passed, "a skip is not a pass");
        assert_eq!(skip.skip_reason.as_deref(), Some("/dev/kvm unavailable"));
        let wire: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(wire["schema"], 5);
        assert_eq!(wire["results"][3]["result"], "skip");
        assert_eq!(wire["results"][3]["skip_reason"], "/dev/kvm unavailable");
        assert_eq!(
            wire["results"][3]["attempt_results"][0]["outcome"],
            "skipped"
        );
    }

    #[test]
    fn selection_round_trips_only_in_schema_5() {
        let selection = TestSelection {
            expression: Some("test(kvm_)".into()),
            focused_tests: vec!["suite$skip".into()],
            disabled_in_config: vec!["gpu-suite".into()],
            disabled_in_config_source: Some(TestSelectionSource {
                path: "ci/test-selection.md".into(),
                sha256: "a".repeat(64),
            }),
        };
        let report = skipped_report().with_selection(selection.clone()).unwrap();
        let bytes = report.to_skipped_json().unwrap();
        let parsed =
            TestResults::from_declared_schema_json_slice(&bytes, SKIPPED_RESULTS_SCHEMA).unwrap();
        assert_eq!(parsed, report);
        assert_eq!(parsed.selection, Some(selection));
        let wire: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(wire["selection"]["expression"], "test(kvm_)");
        assert_eq!(
            wire["selection"]["disabled_in_config_source"]["sha256"],
            "a".repeat(64)
        );
        for error in [
            report.to_current_json().unwrap_err(),
            report.to_classified_json().unwrap_err(),
            report.to_diagnostic_json().unwrap_err(),
        ] {
            assert!(
                error.contains("refuses to discard the run selection"),
                "{error}"
            );
        }
        // Schema 4 cannot carry a selection at all.
        let schema_four =
            br#"{"schema":4,"executed_tests":1,"filtered_tests":0,"results":[],"selection":null}"#;
        assert!(TestResults::from_json_slice(schema_four).is_err());
    }

    #[test]
    fn malformed_selections_are_refused() {
        let mut duplicate = skipped_report();
        duplicate.selection = Some(TestSelection {
            expression: None,
            focused_tests: vec!["a".into(), "a".into()],
            disabled_in_config: vec![],
            disabled_in_config_source: None,
        });
        assert!(duplicate
            .to_skipped_json()
            .unwrap_err()
            .contains("duplicates"));
        let bad_digest = br#"{"schema":5,"executed_tests":0,"filtered_tests":0,"results":[],"selection":{"expression":null,"focused_tests":[],"disabled_in_config":[],"disabled_in_config_source":{"path":"ci/x.md","sha256":"short"}}}"#;
        assert!(TestResults::from_json_slice(bad_digest)
            .unwrap_err()
            .contains("64 lowercase hex"));
    }

    #[test]
    fn older_schemas_refuse_to_write_or_read_a_skip() {
        let report = skipped_report();
        assert!(report.to_current_json().is_err());
        for error in [
            report.to_classified_json().unwrap_err(),
            report.to_diagnostic_json().unwrap_err(),
            TestResults::diagnostic(4, 2, report.results.clone().unwrap()).unwrap_err(),
        ] {
            assert!(error.contains("refuses to discard"), "{error}");
        }
        // The schema-4 writer specifically refuses the skip designation.
        let only_skip = TestResults::skipped(
            1,
            0,
            vec![TestResult::skipped(
                "suite$only".into(),
                vec![TestAttemptResult::new(
                    1,
                    TestAttemptOutcome::Skipped,
                    Some("no device".into()),
                )
                .unwrap()],
                "no device".into(),
            )
            .unwrap()],
        )
        .unwrap();
        assert!(only_skip
            .to_diagnostic_json()
            .unwrap_err()
            .contains("refuses to discard the skip designation"));
        // A schema-4 document cannot spell skip at all.
        let bytes = br#"{"schema":4,"executed_tests":1,"filtered_tests":0,"results":[{"id":"a","result":"skip","attempts":1,"attempt_results":[{"attempt":1,"outcome":"skipped","detail":"no device"}],"diagnostic_reason":null}]}"#;
        assert!(TestResults::from_json_slice(bytes)
            .unwrap_err()
            .contains("unknown value \"skip\""));
        let declared_four = report.to_skipped_json().unwrap();
        assert!(TestResults::from_declared_schema_json_slice(&declared_four, 4).is_err());
    }

    #[test]
    fn inconsistent_skip_rows_are_refused() {
        // A skip whose terminal attempt actually failed.
        assert!(
            TestResult::skipped("suite$x".into(), failed_attempt(), "no device".into(),).is_err()
        );
        // A skip with an empty reason.
        assert!(TestResult::skipped(
            "suite$x".into(),
            vec![
                TestAttemptResult::new(1, TestAttemptOutcome::Skipped, Some("no device".into()),)
                    .unwrap()
            ],
            " ".into(),
        )
        .is_err());
        // A skipped attempt without the row designation, and a skip row whose
        // reason field disagrees with its result, both refuse on read.
        let missing_reason = br#"{"schema":5,"executed_tests":1,"filtered_tests":0,"results":[{"id":"a","result":"fail","attempts":1,"attempt_results":[{"attempt":1,"outcome":"skipped","detail":"no device"}],"diagnostic_reason":null,"skip_reason":null}]}"#;
        assert!(TestResults::from_json_slice(missing_reason)
            .unwrap_err()
            .contains("without a skip designation"));
        let stray_reason = br#"{"schema":5,"executed_tests":1,"filtered_tests":0,"results":[{"id":"a","result":"pass","attempts":1,"attempt_results":[{"attempt":1,"outcome":"passed","detail":null}],"diagnostic_reason":null,"skip_reason":"no device"}]}"#;
        assert!(TestResults::from_json_slice(stray_reason)
            .unwrap_err()
            .contains("must be null unless result is skip"));
    }

    #[test]
    fn diagnostic_schema_round_trips_and_classifies_each_row() {
        let report = report();
        let bytes = report.to_diagnostic_json().unwrap();
        let parsed = TestResults::from_declared_schema_json_slice(&bytes, 4).unwrap();
        assert_eq!(parsed, report);
        let rows = parsed.results.unwrap();
        let blocking = rows.iter().filter(|r| r.is_blocking_failure()).count();
        let diagnostic = rows.iter().filter(|r| r.is_diagnostic_failure()).count();
        assert_eq!((blocking, diagnostic), (1, 1));
        assert!(!rows[2].passed, "a diagnostic failure is still a failure");
        let wire: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(wire["results"][2]["result"], "diagnostic_fail");
        assert_eq!(
            wire["results"][2]["diagnostic_reason"],
            "bounded probe; a slow host is not a product loss"
        );
        assert_eq!(wire["results"][1]["result"], "fail");
        assert_eq!(wire["results"][1]["diagnostic_reason"], Value::Null);
    }

    #[test]
    fn older_schemas_refuse_to_write_or_read_a_diagnostic_failure() {
        let report = report();
        for error in [
            report.to_classified_json().unwrap_err(),
            TestResults::classified(3, 2, report.results.clone().unwrap()).unwrap_err(),
        ] {
            assert!(
                error.contains("refuses to discard the diagnostic designation"),
                "{error}"
            );
        }
        // The schema-2 writer refuses the designation itself, not only the missing attempts.
        let legacy = TestResult {
            attempt_results: None,
            ..report.results.as_ref().unwrap()[2].clone()
        };
        let error = require_legacy_result(&legacy).unwrap_err();
        assert!(
            error.contains("refuses to discard the diagnostic designation"),
            "{error}"
        );
        assert!(TestResults::current(1, 0, vec![legacy]).is_err());
        // A schema-3 document cannot spell diagnostic_fail at all.
        let bytes = br#"{"schema":3,"executed_tests":1,"filtered_tests":0,"results":[{"id":"a","result":"diagnostic_fail","attempts":1,"attempt_results":[{"attempt":1,"outcome":"failed","detail":"exit 1"}]}]}"#;
        assert!(TestResults::from_json_slice(bytes)
            .unwrap_err()
            .contains("unknown value \"diagnostic_fail\""));
        let declared_three = report.to_diagnostic_json().unwrap();
        assert!(TestResults::from_declared_schema_json_slice(&declared_three, 3).is_err());
    }

    #[test]
    fn inconsistent_diagnostic_rows_are_refused() {
        let base = || -> Value {
            serde_json::from_slice(&report().to_diagnostic_json().unwrap()).unwrap()
        };
        let refuse = |change: &dyn Fn(&mut Value)| {
            let mut wire = base();
            change(&mut wire);
            assert!(
                TestResults::from_json_slice(&serde_json::to_vec(&wire).unwrap()).is_err(),
                "{wire}"
            );
        };
        // diagnostic_fail without a reason, with an empty or untrimmed reason, or with a number.
        refuse(&|w| w["results"][2]["diagnostic_reason"] = Value::Null);
        refuse(&|w| w["results"][2]["diagnostic_reason"] = "".into());
        refuse(&|w| w["results"][2]["diagnostic_reason"] = " why".into());
        refuse(&|w| w["results"][2]["diagnostic_reason"] = 7.into());
        // A reason on a blocking fail or on a pass.
        refuse(&|w| w["results"][1]["diagnostic_reason"] = "excuse".into());
        refuse(&|w| w["results"][0]["diagnostic_reason"] = "excuse".into());
        // A diagnostic_fail whose terminal attempt passed.
        refuse(&|w| {
            w["results"][2]["attempt_results"][0]["outcome"] = "passed".into();
            w["results"][2]["attempt_results"][0]["detail"] = Value::Null;
        });
        // The reason field is required in schema 4, even when null.
        refuse(&|w| {
            w["results"][0]
                .as_object_mut()
                .unwrap()
                .remove("diagnostic_reason");
        });
        // And refused in schema 3.
        refuse(&|w| w["schema"] = 3.into());
        assert!(TestResult::diagnostic_failure(
            "a".into(),
            vec![TestAttemptResult::new(1, TestAttemptOutcome::Passed, None).unwrap()],
            "why".into()
        )
        .is_err());
        assert!(TestResult::diagnostic_failure("a".into(), failed_attempt(), "".into()).is_err());
        // Only a measured failure may be excused: the terminal attempt must be failed,
        // cpu_timeout or wall_timeout.
        for (outcome, excusable) in [
            (TestAttemptOutcome::Failed, true),
            (TestAttemptOutcome::CpuTimeout, true),
            (TestAttemptOutcome::WallTimeout, true),
            (TestAttemptOutcome::Cancelled, false),
            (TestAttemptOutcome::InfrastructureError, false),
            (TestAttemptOutcome::NoResult, false),
        ] {
            let attempts = vec![TestAttemptResult::new(1, outcome, Some("cause".into())).unwrap()];
            assert_eq!(
                TestResult::diagnostic_failure("a".into(), attempts, "why".into()).is_ok(),
                excusable,
                "{outcome:?}"
            );
            let mut wire = base();
            wire["results"][2]["attempt_results"][0]["outcome"] = outcome.value().into();
            assert_eq!(
                TestResults::from_json_slice(&serde_json::to_vec(&wire).unwrap()).is_ok(),
                excusable,
                "reader, {outcome:?}"
            );
        }
    }
}
