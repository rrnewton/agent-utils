//! The team gh wrapper (`gh-team`, linked into a team's tool directory as `gh`): it acts as the
//! account the team names in `GH_PACED_ACCOUNT`, paces every call through gh-paced, never
//! recurses through itself, refuses shared-state `gh auth` commands, and never falls back to
//! another account.

use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const PACED: &str = env!("CARGO_BIN_EXE_gh-paced");

/// A fake real gh: `auth token --hostname H --user team-account` prints a token; any other call
/// logs its arguments, host, and token.
const FAKE_GH: &str = r#"#!/bin/bash
if [ "$1 $2" = "auth token" ]; then
  [ "$3 $4 $5 $6" = "--hostname github.com --user team-account" ] && [ -z "${GH_TOKEN:-}" ] \
    && echo stored-token && exit 0
  exit 1
fi
if [ "$1 $2" = "api rate_limit" ]; then
  r=$(( $(date +%s) + 3000 ))
  printf '{"resources":{"core":{"limit":5000,"used":1,"remaining":4999,"reset":%s},"graphql":{"limit":5000,"used":0,"remaining":5000,"reset":%s},"search":{"limit":30,"used":0,"remaining":30,"reset":%s}}}\n' "$r" "$r" "$r"
  exit 0
fi
echo "call $* host=$GH_HOST token=$GH_TOKEN" >> "${FAKE_GH_LOG:?}"
echo "real gh ran"
"#;

struct Team {
    dir: PathBuf,
}

impl Team {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("gh-team-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for sub in ["bin", "state", "home"] {
            std::fs::create_dir_all(dir.join(sub)).unwrap();
        }
        let gh = dir.join("real-gh");
        std::fs::write(&gh, FAKE_GH).unwrap();
        std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
        let wrapper = Path::new(env!("CARGO_MANIFEST_DIR")).join("gh-team");
        symlink(&wrapper, dir.join("bin/gh")).unwrap();
        std::fs::write(dir.join("gh.log"), "").unwrap();
        Self { dir }
    }

    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let mut command = Command::new(self.dir.join("bin/gh"));
        command
            .env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.dir.join("bin").display()),
            )
            .env("HOME", self.dir.join("home"))
            .env("GH_PACED", PACED)
            .env("GH_PACED_REAL_GH", self.dir.join("real-gh"))
            .env("GH_PACED_STATE_DIR", self.dir.join("state"))
            .env("FAKE_GH_LOG", self.dir.join("gh.log"))
            .env("GH_TOKEN", "ambient-token-that-must-not-leak")
            .args(args)
            .stdin(Stdio::null());
        for (key, value) in env {
            command.env(key, value);
        }
        command.output().unwrap()
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.dir.join("gh.log")).unwrap()
    }
}

impl Drop for Team {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn runs_the_real_gh_as_the_team_account_through_gh_paced() {
    let team = Team::new("account");
    let output = team.run(
        &["pr", "view", "5"],
        &[("GH_PACED_ACCOUNT", "team-account")],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "real gh ran\n");
    assert_eq!(
        team.log(),
        "call pr view 5 host=github.com token=stored-token\n"
    );
    // gh-paced kept this call on the team account's budget.
    assert!(team.dir.join("state").read_dir().unwrap().next().is_some());
}

#[test]
fn refuses_without_an_account_and_never_falls_back_to_another() {
    let team = Team::new("refusals");
    let missing = team.run(&["pr", "list"], &[]);
    assert_eq!(missing.status.code(), Some(78));
    assert!(stderr(&missing).contains("GH_PACED_ACCOUNT is not set"));
    let other = team.run(&["pr", "list"], &[("GH_PACED_ACCOUNT", "someone-else")]);
    assert_eq!(other.status.code(), Some(1));
    assert!(stderr(&other).contains("refusing to fall back"));
    assert_eq!(team.log(), "");
}

#[test]
fn refuses_shared_state_auth_commands_and_recursion() {
    let team = Team::new("guards");
    for verb in ["login", "logout", "refresh", "switch"] {
        let output = team.run(&["auth", verb], &[("GH_PACED_ACCOUNT", "team-account")]);
        assert_eq!(output.status.code(), Some(64), "{verb}");
        assert!(stderr(&output).contains("refusing shared-state command"));
    }
    let wrapper = team.dir.join("bin/gh");
    let recursive = team.run(
        &["pr", "list"],
        &[
            ("GH_PACED_ACCOUNT", "team-account"),
            ("GH_PACED_REAL_GH", wrapper.to_str().unwrap()),
        ],
    );
    assert_eq!(recursive.status.code(), Some(78));
    assert!(stderr(&recursive).contains("refusing to recurse"));
    let unpaced = team.run(
        &["pr", "list"],
        &[
            ("GH_PACED_ACCOUNT", "team-account"),
            ("GH_PACED", "/nonexistent/gh-paced"),
        ],
    );
    assert_eq!(unpaced.status.code(), Some(127));
    assert!(stderr(&unpaced).contains("Refusing to call GitHub unpaced"));
    assert_eq!(team.log(), "");
}
