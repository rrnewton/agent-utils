//! Structured per-step test results written by controlled test runners.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Map;
use serde_json::Value;

const CURRENT_SCHEMA: u64 = 2;

/// Terminal result of one named test, including how many attempts the test runner made.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TestResult {
    /// Stable test identity emitted by the controlled test runner.
    pub id: String,
    /// Terminal verdict after the runner's own retries.
    pub passed: bool,
    /// Number of attempts made by the test runner; always at least one.
    pub attempts: u64,
}

impl TestResult {
    /// Construct one validated terminal result.
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
        })
    }
}

/// Counts and terminal per-test results captured from one controlled test-runner step.
///
/// Schema 1 retained only the two counts. It remains readable with `results == None`,
/// but only schema 2 is writable and authoritative for individual test results.
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
        Ok(Self {
            executed_tests,
            filtered_tests,
            results: Some(results),
        })
    }

    /// Read retained schema 1 counts or the complete current schema.
    pub fn from_json_slice(bytes: &[u8]) -> Result<Self, String> {
        let value: Value = serde_json::from_slice(bytes)
            .map_err(|error| format!("structured-test-results-json: {error}"))?;
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
            CURRENT_SCHEMA => {
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
                    exact_fields(row, &["id", "result", "attempts"])?;
                    let id = row
                        .get("id")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            format!("structured-test-results-results[{index}].id must be a string")
                        })?
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
                        format!(
                            "structured-test-results-results[{index}].attempts must be an unsigned integer"
                        )
                    })?;
                    results.push(TestResult::new(id, passed, attempts).map_err(|error| {
                        format!("structured-test-results-results[{index}]: {error}")
                    })?);
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

    /// Serialize the current shape, refusing retained count-only evidence.
    pub fn to_current_json(&self) -> Result<Vec<u8>, String> {
        let results = self.results.as_ref().ok_or_else(|| {
            "structured-test-results-schema: retained schema 1 has no current write path"
                .to_string()
        })?;
        validate_results(self.executed_tests, results)?;
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
        TestResult::new(result.id.clone(), result.passed, result.attempts)?;
        if !ids.insert(result.id.as_str()) {
            return Err(format!(
                "structured-test-results-id is duplicated: {:?}",
                result.id
            ));
        }
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
