//! Acceptance list: every downstream-used symbol resolves from the two small
//! crates alone, with no scheduler, cgroup, CLI runtime, or YAML in the tree.
//!
//! This file only names paths; it never runs the runner.

use dagrun_model::env::{
    ARTIFACT_READY_DIR_ENV, DELEGATED_CGROUP_ENV, DELEGATED_UNBOXED_ENV, DIRECT_CGROUP_ENV,
    FORCE_ATTEMPT_ENV, IN_SCOPE_ENV, OUTER_RUN_ENV, SCOPE_UNIT_ENV, STEP_STARTED_MONOTONIC_NS_ENV,
};
use dagrun_model::io::{dag_from_json, dag_from_value, dag_to_json, DagJsonError};
use dagrun_model::model::{
    command_with_inner_jobs, env_with_inner_jobs, resolved_wall_timeout, result_manifest_owner,
    ArtifactDep, CmdType, DagConfig, DagManifest, PublishedArtifact, ResourceHint, ResultManifest,
    Step, StepClass, StructuredTestResultsManifest,
};
use dagrun_model::{
    monotonic_now_ns, select_steps_by_labels, select_steps_by_tags, steps_violating_run_timeout,
    to_ascii_groups,
};
use testrun_core::proccpu::ProcessGroupCpu;
use testrun_core::{
    TestAttemptOutcome, TestAttemptResult, TestResult, TestResults, DIAGNOSTIC_RESULTS_SCHEMA,
    TEST_COUNTS_PATH_ENV,
};

#[test]
fn every_acceptance_path_resolves() {
    // Env names.
    let _ = (
        DELEGATED_CGROUP_ENV,
        DELEGATED_UNBOXED_ENV,
        DIRECT_CGROUP_ENV,
        IN_SCOPE_ENV,
        SCOPE_UNIT_ENV,
        FORCE_ATTEMPT_ENV,
        ARTIFACT_READY_DIR_ENV,
        STEP_STARTED_MONOTONIC_NS_ENV,
        OUTER_RUN_ENV,
        TEST_COUNTS_PATH_ENV,
        DIAGNOSTIC_RESULTS_SCHEMA,
    );
    // IO entry points and error type.
    let _ = (dag_from_json, dag_from_value, dag_to_json);
    let _: Option<DagJsonError> = None;
    // Model types, variants, and methods.
    let _ = (
        CmdType::Unknown,
        StepClass::CpuBound,
        StepClass::LatencyBound,
        StepClass::Light,
        ResultManifest::ManifestCell,
        ResultManifest::StructuredTestResults,
    );
    let _: Option<ArtifactDep> = None;
    let _: Option<DagManifest> = None;
    let _: Option<PublishedArtifact> = None;
    let _: Option<ResourceHint> = None;
    let _ = DagConfig::default();
    let _ = DagConfig::with_steps;
    let _ = Step::effective_result_manifests;
    let _ = Step::structured_test_results_manifest;
    let _ = Step::tag;
    let _ = StepClass::value;
    let _ = StructuredTestResultsManifest::current("owner");
    let _ = StructuredTestResultsManifest::diagnostic("owner");
    let _ = StructuredTestResultsManifest::skipped("owner");
    // Pure helpers.
    let _ = (
        result_manifest_owner,
        command_with_inner_jobs,
        env_with_inner_jobs,
        resolved_wall_timeout,
        select_steps_by_labels,
        select_steps_by_tags,
        steps_violating_run_timeout,
        monotonic_now_ns,
        to_ascii_groups,
    );
    // testrun-core types, variants, and methods.
    let _ = (
        TestAttemptOutcome::Passed,
        TestAttemptOutcome::CpuTimeout,
        TestAttemptOutcome::WallTimeout,
        TestAttemptOutcome::NoResult,
        TestAttemptOutcome::InfrastructureError,
        TestAttemptOutcome::Failed,
    );
    let _: Option<TestAttemptResult> = None;
    let _: Option<TestResult> = None;
    let _: Option<TestResults> = None;
    let _ = TestAttemptResult::new;
    let _ = TestResult::new;
    let _ = TestResult::is_blocking_failure;
    let _ = TestResult::is_diagnostic_failure;
    let _ = TestResults::current;
    let _ = TestResults::diagnostic;
    let _ = ProcessGroupCpu::new;
    let _ = ProcessGroupCpu::seconds;
    // A minimal config round-trips through the small crates alone.
    let cfg = dag_from_json(r#"{"steps":[]}"#).unwrap();
    assert_eq!(
        dag_to_json(&cfg),
        dag_to_json(&dag_from_value(&serde_json::json!({"steps":[]})).unwrap())
    );
}
