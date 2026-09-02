//! Public label-selection API coverage for library consumers.

use dagrun::{dag_from_json, select_steps_by_labels, Step};

#[test]
fn library_consumers_select_labels_with_dependency_ancestry() {
    let cfg = dag_from_json(
        r#"{"steps":[
            {"group":"build","job":"shared","cmd":"true"},
            {"group":"test","job":"portable","cmd":"true","labels":["portable"],"deps":["build.shared"]},
            {"group":"test","job":"privileged","cmd":"true","labels":["privileged"],"deps":["build.shared"]}
        ]}"#,
    )
    .unwrap();

    let selected = select_steps_by_labels(&cfg, &["privileged".into()]).unwrap();
    assert_eq!(
        selected.steps.iter().map(Step::tag).collect::<Vec<_>>(),
        ["build.shared", "test.privileged"]
    );
}
