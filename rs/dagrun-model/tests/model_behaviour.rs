//! Behavioural coverage for the pieces a downstream vendoring only
//! `dagrun-model` (and `testrun-core`) relies on, exercised through the
//! public API alone: passthrough substitution, step selection, ASCII
//! rendering, strict JSON errors, and the monotonic step-start epoch.
//!
//! Ported from the runner's own tests (`rs/dagrun/src/cli.rs`,
//! `rs/dagrun/src/viz.rs`) for #1513, so `cargo test -p dagrun-model`
//! covers them without the scheduler, cgroup runtime, or CLI.

use dagrun_model::ascii::{to_ascii, to_ascii_groups};
use dagrun_model::env::STEP_STARTED_MONOTONIC_NS_ENV;
use dagrun_model::model::Step;
use dagrun_model::{
    apply_passthrough_args, dag_from_json, monotonic_now_ns, select_steps_by_labels,
    select_steps_by_tags, ARGS_PLACEHOLDER,
};

fn tags(cfg: &dagrun_model::DagConfig) -> Vec<String> {
    cfg.steps.iter().map(Step::tag).collect()
}

fn tiny() -> dagrun_model::DagConfig {
    dag_from_json(
        r#"{"steps":[{"group":"build","job":"app","cmd":"echo {args}"},{"group":"test","job":"unit","cmd":"true","deps":["build.app"]}]}"#,
    )
    .unwrap()
}

#[test]
fn passthrough_requires_declaration_and_substitutes() {
    let cfg = tiny();
    assert_eq!(ARGS_PLACEHOLDER, "{args}");
    let applied = apply_passthrough_args(&cfg, Some("--case xyz")).unwrap();
    assert_eq!(applied.steps[0].cmd, "echo --case xyz");
    // Steps that never declared the placeholder keep their command verbatim.
    assert_eq!(applied.steps[1].cmd, "true");
    // The source configuration is never modified.
    assert_eq!(cfg.steps[0].cmd, "echo {args}");

    let selected = select_steps_by_tags(&cfg, &["test.unit".to_string()], true).unwrap();
    let error = apply_passthrough_args(&selected, Some("x")).unwrap_err();
    assert!(
        error.contains("no selected step declares the '{args}' placeholder"),
        "{error}"
    );
    assert!(error.contains("--selected"), "{error}");

    // No args and no declaration is a no-op, not an error.
    let undeclared = apply_passthrough_args(&selected, None).unwrap();
    assert_eq!(tags(&undeclared), ["test.unit"]);
}

#[test]
fn selected_step_includes_every_dependency_it_requires() {
    let cfg = dag_from_json(
        r#"{"steps":[
            {"group":"package","job":"tarball","cmd":"true","deps":["test.unit"]},
            {"group":"test","job":"unit","cmd":"true","deps":["build.app"]},
            {"group":"build","job":"app","cmd":"true"}
        ]}"#,
    )
    .unwrap();

    let selected = select_steps_by_tags(&cfg, &["package.tarball".to_string()], false).unwrap();
    assert_eq!(
        tags(&selected),
        ["package.tarball", "test.unit", "build.app"]
    );
    assert_eq!(selected.steps[0].deps, ["test.unit"]);
    assert_eq!(selected.steps[1].deps, ["build.app"]);
}

#[test]
fn selected_step_includes_its_artifact_producer() {
    let cfg = dag_from_json(
        r#"{"steps":[
            {"group":"e2e","job":"run","cmd":"true",
             "artifact_deps":[{"step":"build.app","artifact":"app-bin"}]},
            {"group":"build","job":"app","cmd":"true",
             "publishes":[{"name":"app-bin","path":"target/app"}]},
            {"group":"other","job":"unrelated","cmd":"true"}
        ]}"#,
    )
    .unwrap();

    // The artifact producer is a required dependency even though the
    // consumer does not list it in deps: without it the selection
    // would name a step that is not there.
    let selected = select_steps_by_tags(&cfg, &["e2e.run".to_string()], false).unwrap();
    assert_eq!(tags(&selected), ["e2e.run", "build.app"]);
    // Untagged selection is unchanged: the producer alone selects
    // neither its consumer nor the unrelated step.
    let producer_only = select_steps_by_tags(&cfg, &["build.app".to_string()], false).unwrap();
    assert_eq!(tags(&producer_only), ["build.app"]);
}

#[test]
fn selected_dependencies_can_be_explicitly_ignored() {
    let cfg = tiny();
    let selected = select_steps_by_tags(&cfg, &["test.unit".to_string()], true).unwrap();
    assert_eq!(tags(&selected), ["test.unit"]);
    assert!(selected.steps[0].deps.is_empty());
}

#[test]
fn labels_select_a_union_and_include_dependency_ancestry() {
    let cfg = dag_from_json(
        r#"{"steps":[
            {"group":"build","job":"app","cmd":"true"},
            {"group":"test","job":"unit","cmd":"true","labels":["quick","full"],"deps":["build.app"]},
            {"group":"test","job":"slow","cmd":"true","labels":["full"]},
            {"group":"privileged","job":"host","cmd":"true","labels":["super"]}
        ]}"#,
    )
    .unwrap();

    let quick = select_steps_by_labels(&cfg, &["quick".into()]).unwrap();
    assert_eq!(tags(&quick), ["build.app", "test.unit"]);
    let union = select_steps_by_labels(&cfg, &["quick".into(), "super".into()]).unwrap();
    assert_eq!(tags(&union), ["build.app", "test.unit", "privileged.host"]);
    let error = select_steps_by_labels(&cfg, &["test.unit".into()]).unwrap_err();
    assert!(error.contains("unknown label(s): test.unit"), "{error}");
    assert!(
        error.contains("Known labels: full, quick, super"),
        "{error}"
    );
}

fn grouped_cfg() -> dagrun_model::DagConfig {
    dag_from_json(
        r#"{"steps":[
            {"group":"build","job":"app","cmd":"true"},
            {"group":"test","job":"unit","cmd":"true","deps":["build.app"]},
            {"group":"e2e","job":"a","cmd":"true","deps":["build.app"],"hint":{"resources":{"browser":1}}},
            {"group":"e2e","job":"b","cmd":"true","deps":["build.app"],"hint":{"resources":{"browser":1}}}
        ],"resource_caps":{"browser":1}}"#,
    )
    .unwrap()
}

#[test]
fn ascii_shows_layers_deps_and_resources() {
    let art = to_ascii(&grouped_cfg(), None);
    assert!(
        art.starts_with("DAG - 4 steps, 3 edges, 2 layer(s)\n"),
        "{art}"
    );
    assert!(
        art.contains("layer 0:") && art.contains("layer 1:"),
        "{art}"
    );
    assert!(art.contains("build.app"), "{art}");
    assert!(art.contains("<- build.app"), "{art}");
    assert!(art.contains("{browser:1}"), "{art}");
    // Layers are topological: the dependency sits in the earlier layer.
    let layer0 = art.find("layer 0:").unwrap();
    let layer1 = art.find("layer 1:").unwrap();
    let app = art.find("build.app").unwrap();
    assert!(layer0 < app && app < layer1, "{art}");
}

#[test]
fn ascii_groups_header_counts_groups_steps_and_merged_edges() {
    let art = to_ascii_groups(&grouped_cfg(), None);
    assert!(
        art.starts_with(
            "DAG by group - 3 groups, 4 steps, 2 group edges \
             (3 cross-group step edges merged; 0 within groups)\n"
        ),
        "{art}"
    );
    assert!(art.contains("build"), "{art}");
    assert!(art.contains("1 step"), "{art}");
    assert!(art.contains("2 steps"), "{art}");
    assert!(art.contains("<- build (1)"), "{art}");
    assert!(art.contains("<- build (2)"), "{art}");
}

#[test]
fn steps_not_a_list_error_names_what_was_read_and_what_to_do() {
    let error = dag_from_json(r#"{"steps": "not a list"}"#)
        .unwrap_err()
        .to_string();
    // What was expected and what was actually read.
    assert!(error.contains("top-level 'steps' list"), "{error}");
    assert!(error.contains("'steps' with type str"), "{error}");
    assert!(error.contains("top-level keys: 'steps'"), "{error}");
    // What to do about it.
    assert!(error.contains("Pass a dagrun DAG file"), "{error}");
    assert!(error.contains("dagrun quickstart"), "{error}");
}

#[test]
fn monotonic_step_start_epoch_contract() {
    assert_eq!(
        STEP_STARTED_MONOTONIC_NS_ENV,
        "DAGRUN_STEP_STARTED_MONOTONIC_NS"
    );
    // The helper the scheduler stamps into a step's env: present on this
    // platform and never moving backwards between successive reads.
    // (Ordering only; no elapsed-time or wall-clock assertion.)
    let first = monotonic_now_ns().expect("CLOCK_MONOTONIC must be readable");
    let second = monotonic_now_ns().expect("CLOCK_MONOTONIC must be readable");
    assert!(first > 0);
    assert!(second >= first);
}
