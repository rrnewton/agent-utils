//! Writing a project's `.herdr-run.yaml` from the annotated template.
//!
//! The template carries EVERY knob, each set to the value the tool would use anyway, so a project
//! can adopt the file and change nothing. That is deliberate: the reference for what is
//! configurable is the file the tool writes into the project, not a block of documentation that
//! restates it and then drifts from it.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::config::CONFIG_FILENAMES;
use crate::error::{HerdrRunError, Result};

/// The complete annotated `.herdr-run.yaml` written by `herdr-run init`.
pub const CONFIG_TEMPLATE: &str = include_str!("config_template.yaml");

/// Write the annotated configuration template into `directory` and return the path written.
///
/// Refuses to overwrite an existing configuration unless `force`. It also refuses when only the
/// `.yml` spelling exists, because the `.yaml` this would write silently takes precedence over it:
/// a caller who ends up with two files and no warning has been given a puzzle, not a config.
pub fn write_config_template(directory: &Path, force: bool) -> Result<PathBuf> {
    let destination = directory.join(CONFIG_FILENAMES[0]);
    if !force {
        for filename in CONFIG_FILENAMES {
            let existing = directory.join(filename);
            if !existing.exists() {
                continue;
            }
            let precedence = if filename == CONFIG_FILENAMES[0] {
                String::new()
            } else {
                format!(
                    ", and the {} this would write takes precedence over it",
                    CONFIG_FILENAMES[0]
                )
            };
            return Err(HerdrRunError::config(format!(
                "{} already exists{precedence}; pass --force to overwrite",
                existing.display()
            )));
        }
    }
    let mut options = OpenOptions::new();
    options.write(true);
    if force {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }
    let mut file = options.open(&destination).map_err(|error| {
        HerdrRunError::config(format!("cannot write {}: {error}", destination.display()))
    })?;
    file.write_all(CONFIG_TEMPLATE.as_bytes())
        .and_then(|()| file.flush())
        .map_err(|error| {
            HerdrRunError::config(format!("cannot write {}: {error}", destination.display()))
        })?;
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{load_config, Config, KNOWN_KEYS};
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_directory(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "herdr-run-init-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temporary directory");
        path
    }

    /// The template must mention every key the parser knows about.
    ///
    /// This is the promise that makes the guide able to point AT the generated file instead of
    /// restating it: a new configuration key that nobody adds to the template turns the file into
    /// a partial answer, and a partial answer is worse than none because it looks complete.
    #[test]
    fn the_template_carries_every_configuration_key() {
        for key in KNOWN_KEYS {
            assert!(
                CONFIG_TEMPLATE.contains(&format!("\n{key}:")),
                "the generated .herdr-run.yaml never sets {key}"
            );
        }
    }

    /// Writing the template and reading it back must reproduce the built-in defaults exactly.
    #[test]
    fn the_written_template_parses_to_the_built_in_defaults() {
        let root = temporary_directory("defaults");
        let path = write_config_template(&root, false).expect("write template");
        assert_eq!(path, root.join(".herdr-run.yaml"));
        let written = load_config(Some(&path), &root).expect("template must parse");
        let expected = Config {
            source_path: written.source_path.clone(),
            project_root: written.project_root.clone(),
            ..Config::default()
        };
        assert_eq!(written, expected);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_existing_configuration_is_not_clobbered_without_force() {
        for filename in CONFIG_FILENAMES {
            let root = temporary_directory("clobber");
            let existing = root.join(filename);
            fs::write(&existing, "workspace: mine\n").unwrap();
            let error = write_config_template(&root, false).expect_err(filename);
            assert_eq!(error.exit_code(), crate::error::EXIT_CONFIG);
            assert!(error.message().contains("pass --force"), "{error}");
            assert_eq!(
                fs::read_to_string(&existing).unwrap(),
                "workspace: mine\n",
                "{filename} was modified by a refused init"
            );
            if filename != ".herdr-run.yaml" {
                assert!(
                    error.message().contains("takes precedence over it"),
                    "an existing {filename} must say the new file would shadow it: {error}"
                );
                assert!(
                    !root.join(".herdr-run.yaml").exists(),
                    "a refused init must not leave a shadowing file behind"
                );
            }
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn force_overwrites_an_existing_configuration() {
        let root = temporary_directory("force");
        let existing = root.join(".herdr-run.yaml");
        fs::write(&existing, "workspace: mine\n").unwrap();
        let path = write_config_template(&root, true).expect("forced write");
        assert_eq!(fs::read_to_string(&path).unwrap(), CONFIG_TEMPLATE);
        fs::remove_dir_all(root).unwrap();
    }

    /// The two things the owner asked the template to say out loud.
    #[test]
    fn the_template_names_the_allow_everything_mode_and_the_human_only_rule() {
        assert!(CONFIG_TEMPLATE.contains(r#"allow: ["*"]"#));
        assert!(CONFIG_TEMPLATE.contains("ALLOW-EVERYTHING MODE"));
        assert!(CONFIG_TEMPLATE.contains("a human-only knob"));
        assert!(CONFIG_TEMPLATE.contains("DO NOT LET AN AGENT EDIT THIS SECTION"));
        assert!(CONFIG_TEMPLATE.contains("worktrees/slotNN/"));
    }
}
