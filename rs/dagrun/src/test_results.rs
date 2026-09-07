//! Structured per-step test results written by controlled test runners.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::path::Path;

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use serde_json::Map;
use serde_json::Value;

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
pub const CURRENT_SCHEMA: u64 = 3;
/// Schema 2 remains readable while result producers migrate atomically.
pub const RETAINED_RESULTS_SCHEMA: u64 = 2;

/// Classified terminal cause for one test-runner attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TestAttemptOutcome {
    /// Attempt exited successfully.
    Passed,
    /// Attempt exited unsuccessfully without a timeout or external cancellation.
    Failed,
    /// Attempt exceeded its per-attempt CPU-time bound.
    CpuTimeout,
    /// Attempt exceeded its separate wall-clock backstop.
    WallTimeout,
    /// Attempt was stopped by an external signal.
    Cancelled,
    /// Attempt could not produce trustworthy execution or accounting evidence.
    InfrastructureError,
    /// Attempt produced an explicit no-result classification that is neither a
    /// timeout nor an infrastructure/accounting failure.
    NoResult,
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
}

impl TestResult {
    /// Construct one retained schema-2 terminal result.
    ///
    /// Current schema-3 writers must use [Self::with_attempt_results].
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
        })
    }

    /// Attach complete classified attempt evidence for the current schema.
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
        };
        validate_result(&result, true)?;
        Ok(result)
    }
}

fn parse_result_row(
    row: &Map<String, Value>,
    index: usize,
    current: bool,
) -> Result<TestResult, String> {
    let expected = if current {
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
    let passed = match row.get("result").and_then(Value::as_str) {
        Some("pass") => true,
        Some("fail") => false,
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
    if !current {
        return TestResult::new(id, passed, attempts);
    }
    let attempt_rows = match row.get("attempt_results") {
        Some(Value::Array(rows)) => rows,
        Some(Value::Null) => {
            return Err(format!(
                "structured-test-results-results[{index}].attempt_results must be an array for current schema"
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
    let result = TestResult {
        id,
        passed,
        attempts,
        attempt_results: Some(attempt_results),
    };
    validate_result(&result, false)?;
    Ok(result)
}

/// Counts and terminal per-test results captured from one controlled test-runner step.
///
/// Schema 1 retained only the two counts. It remains readable with `results == None`,
/// schema 2 retained terminal rows without typed attempts; schema 3 is the only current write path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestResults {
    /// Tests that executed, according to the controlled runner's aggregate report.
    pub executed_tests: u64,
    /// Tests excluded by selection, according to the same aggregate report.
    pub filtered_tests: u64,
    /// Per-test terminal results. `None` is retained schema 1, not an empty run.
    pub results: Option<Vec<TestResult>>,
}

impl TestResults {
    /// Construct the current complete shape.
    pub fn current(
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
        })
    }

    /// Read retained schema 1/2 data or the complete current schema.
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
                })
            }
            2 | CURRENT_SCHEMA => {
                exact_fields(
                    object,
                    &["schema", "executed_tests", "filtered_tests", "results"],
                )?;
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
                    results.push(
                        parse_result_row(row, index, schema == CURRENT_SCHEMA).map_err(
                            |error| format!("structured-test-results-results[{index}]: {error}"),
                        )?,
                    );
                }
                validate_results(executed_tests, &results)?;
                Ok(Self {
                    executed_tests,
                    filtered_tests,
                    results: Some(results),
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
        if declared_schema != RETAINED_RESULTS_SCHEMA && declared_schema != CURRENT_SCHEMA {
            return Err(format!(
                "structured-test-results declaration has unsupported schema {declared_schema}; expected retained schema {RETAINED_RESULTS_SCHEMA} or current schema {CURRENT_SCHEMA}"
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
            "schema": CURRENT_SCHEMA,
            "executed_tests": self.executed_tests,
            "filtered_tests": self.filtered_tests,
            "results": rows,
        }))
        .map_err(|error| format!("structured-test-results-json: {error}"))
    }

    /// Atomically publish the current shape at the scheduler-owned path.
    pub fn write_current(&self, path: &Path) -> Result<(), String> {
        let bytes = self.to_current_json()?;
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| "structured-test-results-path has no UTF-8 file name".to_string())?;
        let temporary = path.with_file_name(format!(".{file_name}.tmp-{}", std::process::id()));
        fs::write(&temporary, bytes).map_err(|error| {
            format!(
                "structured-test-results-write {}: {error}",
                temporary.display()
            )
        })?;
        if let Err(error) = fs::rename(&temporary, path) {
            let _ = fs::remove_file(&temporary);
            return Err(format!(
                "structured-test-results-publish {}: {error}",
                path.display()
            ));
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

fn validate_result(result: &TestResult, require_attempt_results: bool) -> Result<(), String> {
    TestResult::new(result.id.clone(), result.passed, result.attempts)?;
    let Some(attempts) = result.attempt_results.as_ref() else {
        if require_attempt_results {
            return Err("structured-test-results current row lacks attempt_results".into());
        }
        return Ok(());
    };
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
        let error = TestResults::current(1, 0, vec![retained.clone()]).unwrap_err();
        assert!(error.contains("current row lacks attempt_results"));

        let report = TestResults {
            executed_tests: 1,
            filtered_tests: 0,
            results: Some(vec![retained]),
        };
        assert!(report
            .to_current_json()
            .unwrap_err()
            .contains("current row lacks attempt_results"));
        let bytes = br#"{"schema":3,"executed_tests":1,"filtered_tests":0,"results":[{"id":"suite$case","result":"fail","attempts":1,"attempt_results":null}]}"#;
        assert!(TestResults::from_json_slice(bytes)
            .unwrap_err()
            .contains("must be an array for current schema"));
    }
}
