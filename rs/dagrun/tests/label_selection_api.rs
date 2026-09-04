//! Public selection API coverage for library consumers.

use dagrun::{
    dag_config_carry_diff, dag_from_json, run_dag, select_steps_by_labels, select_steps_by_tags,
    DagConfig, RunResult, Step,
};

fn graph() -> DagConfig {
    dag_from_json(
        r#"{"steps":[
            {"group":"build","job":"shared","cmd":"true"},
            {"group":"test","job":"portable","cmd":"true","labels":["portable","all"],"deps":["build.shared"]},
            {"group":"test","job":"privileged","cmd":"true","labels":["privileged","all"],"deps":["build.shared"]}
        ]}"#,
    )
    .unwrap()
}

fn tags(cfg: &DagConfig) -> Vec<String> {
    cfg.steps.iter().map(Step::tag).collect()
}

fn assert_source_unchanged(before: &DagConfig, after: &DagConfig) {
    assert_eq!(dag_config_carry_diff(before, after), Vec::<String>::new());
}

#[test]
fn library_consumers_select_exact_tags_with_dependency_ancestry_and_typed_result() {
    let cfg = graph();
    let authored = cfg.clone();

    let selected = select_steps_by_tags(&cfg, &["test.portable".into()], false).unwrap();
    assert_eq!(tags(&selected), ["build.shared", "test.portable"]);
    assert_source_unchanged(&authored, &cfg);

    let result: RunResult = run_dag(&selected, 2, false, 0);
    assert!(result.ok);
    assert_eq!(result.outcomes.len(), 2);
}

#[test]
fn library_consumers_can_ignore_unselected_dependencies_without_rewriting_the_source() {
    let cfg = graph();
    let authored = cfg.clone();

    let leaf_only = select_steps_by_tags(&cfg, &["test.portable".into()], true).unwrap();
    assert_eq!(tags(&leaf_only), ["test.portable"]);
    assert!(leaf_only.steps[0].deps.is_empty());

    let connected =
        select_steps_by_tags(&cfg, &["build.shared".into(), "test.portable".into()], true).unwrap();
    assert_eq!(tags(&connected), ["build.shared", "test.portable"]);
    assert_eq!(connected.steps[1].deps, ["build.shared"]);
    assert_source_unchanged(&authored, &cfg);
}

#[test]
fn tag_selection_refuses_non_exact_or_unknown_ids_without_changing_the_source() {
    let cfg = graph();
    let authored = cfg.clone();

    let error = select_steps_by_tags(&cfg, &["portable".into()], false).unwrap_err();
    assert!(error.contains("unknown step tag(s): portable"), "{error}");
    assert!(error.contains("test.portable"), "{error}");
    assert_source_unchanged(&authored, &cfg);
}

#[test]
fn library_consumers_select_labels_with_dependency_ancestry() {
    let cfg = graph();
    let authored = cfg.clone();

    let selected = select_steps_by_labels(&cfg, &["privileged".into()]).unwrap();
    assert_eq!(tags(&selected), ["build.shared", "test.privileged"]);
    assert_source_unchanged(&authored, &cfg);
}

#[test]
fn label_selection_unions_matches_and_refuses_unknown_labels() {
    let cfg = graph();
    let authored = cfg.clone();

    let selected = select_steps_by_labels(&cfg, &["portable".into(), "privileged".into()]).unwrap();
    assert_eq!(
        tags(&selected),
        ["build.shared", "test.portable", "test.privileged"]
    );

    let error = select_steps_by_labels(&cfg, &["test.portable".into()]).unwrap_err();
    assert!(error.contains("unknown label(s): test.portable"), "{error}");
    assert_source_unchanged(&authored, &cfg);
}
