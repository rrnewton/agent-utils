//! A run marks every command it starts, and a dagrun below that command refuses by default.
//!
//! The override is deliberately explicit. It permits a reviewed temporary exception, and the
//! permitted run replaces the inherited marker with its own identity for any deeper descendant.

use std::process::{Command, Output};

const OUTER_RUN_ENV: &str = "DAGRUN_OUTER_RUN";

fn write_dag(dir: &std::path::Path, name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let dag = dir.join(format!("{name}.json"));
    let marker = dir.join(format!("{name}.marker"));
    let document = serde_json::json!({
        "steps": [{
            "group": "g",
            "job": "j",
            "cmd": format!("printf '%s' \"$DAGRUN_OUTER_RUN\" > {}", marker.display()),
            "env": {"DAGRUN_OUTER_RUN": "forged"},
        }]
    });
    std::fs::write(&dag, serde_json::to_vec(&document).unwrap()).unwrap();
    (dag, marker)
}

fn run(bin: &str, dag: &std::path::Path, outer: Option<&str>, allow_nested: bool) -> Output {
    let mut command = Command::new(bin);
    command.args([
        "run",
        "--dag",
        dag.to_str().unwrap(),
        "--unsafe-no-cgroups",
        "--no-profile",
        "--no-profile-feedback",
        "-q",
    ]);
    if allow_nested {
        command.arg("--allow-unwise-nest-dagruns");
    }
    match outer {
        Some(value) => {
            command.env(OUTER_RUN_ENV, value);
        }
        None => {
            command.env_remove(OUTER_RUN_ENV);
        }
    }
    command.output().expect("failed to spawn dagrun")
}

#[test]
fn nested_run_refuses_by_outer_run_and_override_is_explicit() {
    let bin = env!("CARGO_BIN_EXE_dagrun");
    let dir = std::env::temp_dir().join(format!("dagrun_nested_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (dag, marker) = write_dag(&dir, "inner");

    let refused = run(bin, &dag, Some("--dag outer.json"), false);
    let refused_text = String::from_utf8_lossy(&refused.stderr);
    assert_eq!(refused.status.code(), Some(2), "{refused_text}");
    assert!(
        refused_text.contains("refusing nested invocation"),
        "{refused_text}"
    );
    assert!(refused_text.contains("--dag outer.json"), "{refused_text}");
    assert!(
        refused_text.contains("--allow-unwise-nest-dagruns"),
        "{refused_text}"
    );
    assert!(!marker.exists(), "a refused nested run launched its step");

    let allowed = run(bin, &dag, Some("--dag outer.json"), true);
    let allowed_text = String::from_utf8_lossy(&allowed.stderr);
    assert_eq!(allowed.status.code(), Some(0), "{allowed_text}");
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap(),
        "g.j",
        "the allowed inner run must identify itself to deeper descendants"
    );

    std::fs::remove_file(&marker).unwrap();
    let top_level = run(bin, &dag, None, false);
    let top_level_text = String::from_utf8_lossy(&top_level.stderr);
    assert_eq!(top_level.status.code(), Some(0), "{top_level_text}");
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap(),
        "g.j",
        "an ordinary top-level run must execute and mark its descendants"
    );

    let outer = dir.join("outer.json");
    let outer_document = serde_json::json!({
        "steps": [{
            "group": "g",
            "job": "nested",
            "cmd": format!(
                "{} run --dag {} --unsafe-no-cgroups --no-profile --no-profile-feedback",
                bin,
                dag.display()
            ),
        }]
    });
    std::fs::write(&outer, serde_json::to_vec(&outer_document).unwrap()).unwrap();
    let nested = Command::new(bin)
        .args([
            "run",
            "--dag",
            outer.to_str().unwrap(),
            "--unsafe-no-cgroups",
            "--no-profile",
            "--no-profile-feedback",
            "-v",
        ])
        .env_remove(OUTER_RUN_ENV)
        .output()
        .expect("failed to spawn outer dagrun");
    let nested_text = format!(
        "{}{}",
        String::from_utf8_lossy(&nested.stdout),
        String::from_utf8_lossy(&nested.stderr)
    );
    assert_eq!(nested.status.code(), Some(1), "{nested_text}");
    assert!(
        nested_text.contains("refusing nested invocation"),
        "the actual child dagrun did not refuse:\n{nested_text}"
    );
    assert!(
        nested_text.contains("g.nested"),
        "the refusal did not name the outer run step:\n{nested_text}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
