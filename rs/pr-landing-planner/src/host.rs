//! VCS-host seam and the production GitHub/`git` implementation.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Map, Value};

use crate::classify::parse_rollup;
use crate::model::RawPr;

/// Read-only repository-host and local Git operations required by collection.
pub trait VcsHost {
    /// List open pull requests, optionally scoped by a target base.
    fn list_open_prs(&mut self, repo: &str, base: Option<&str>) -> Result<Vec<RawPr>, String>;
    /// Fetch sources into private destinations and return resolved SHAs by destination.
    fn prefetch_refs(
        &mut self,
        refspecs: &[(String, String)],
    ) -> Result<BTreeMap<String, String>, String>;
    /// Return paths that conflict when merging two commit identities.
    fn merge_tree(&mut self, left: &str, right: &str) -> Result<Vec<String>, String>;
    /// Return whether one commit is an ancestor of another.
    fn is_ancestor(&mut self, ancestor: &str, descendant: &str) -> Result<bool, String>;
    /// Return files changed from the merge base through the head.
    fn changed_files(&mut self, base_sha: &str, head_sha: &str)
        -> Result<BTreeSet<String>, String>;
    /// Return how many commits from the base are absent from the head.
    fn commits_behind(&mut self, head_sha: &str, base_sha: &str) -> Result<i64, String>;
}

const LIGHT_FIELDS: [&str; 14] = [
    "number",
    "title",
    "author",
    "baseRefName",
    "headRefName",
    "headRefOid",
    "isDraft",
    "mergeable",
    "reviewDecision",
    "createdAt",
    "updatedAt",
    "additions",
    "deletions",
    "labels",
];
const ROLLUP_WORKERS: usize = 8;

/// Production adapter using `gh` for metadata and a local Git clone for graph probes.
pub struct GitHubHost {
    git_dir: PathBuf,
    remote: String,
    wrapper: Vec<String>,
    gh: String,
}

impl GitHubHost {
    /// Construct an adapter for a clone, remote, optional command prefix, and `gh` executable.
    pub fn new(git_dir: PathBuf, remote: String, wrapper: Vec<String>, gh: String) -> Self {
        Self {
            git_dir,
            remote,
            wrapper,
            gh,
        }
    }

    fn run(&self, args: &[String], cwd: Option<&Path>, allowed: &[i32]) -> Result<Output, String> {
        let (program, rest) = args
            .split_first()
            .ok_or_else(|| "cannot run an empty command".to_owned())?;
        let mut command = Command::new(program);
        command.args(rest);
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        let output = command.output().map_err(|error| {
            format!(
                "failed to run {}: {error}",
                shell_words::join(args.to_vec())
            )
        })?;
        let code = output.status.code().unwrap_or(-1);
        if !allowed.contains(&code) {
            return Err(format!(
                "command failed ({code}): {}\n{}",
                shell_words::join(args.to_vec()),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(output)
    }

    fn net(&self, command: Vec<String>) -> Vec<String> {
        self.wrapper.iter().cloned().chain(command).collect()
    }

    fn fetch_rollup(
        &self,
        repo: &str,
        number: i64,
    ) -> Result<Option<Vec<crate::model::CheckRun>>, String> {
        let args = self.net(vec![
            self.gh.clone(),
            "pr".into(),
            "view".into(),
            number.to_string(),
            "--repo".into(),
            repo.into(),
            "--json".into(),
            "number,headRefOid,statusCheckRollup".into(),
        ]);
        let output = match self.run(&args, None, &[0]) {
            Ok(output) => output,
            Err(_) => return Ok(None),
        };
        let value: Value = if String::from_utf8_lossy(&output.stdout).trim().is_empty() {
            json_object()
        } else {
            serde_json::from_slice(&output.stdout)
                .map_err(|error| format!("invalid JSON from gh pr view #{number}: {error}"))?
        };
        let Some(obj) = value.as_object() else {
            return Ok(None);
        };
        Ok(Some(parse_rollup(
            obj.get("statusCheckRollup").unwrap_or(&Value::Null),
            &string(obj, "headRefOid"),
        )))
    }

    fn fetch_rollups(
        &self,
        repo: &str,
        numbers: &[i64],
    ) -> Result<BTreeMap<i64, Vec<crate::model::CheckRun>>, String> {
        let next = AtomicUsize::new(0);
        let worker_count = numbers.len().min(ROLLUP_WORKERS);
        let mut completed = std::thread::scope(|scope| {
            let mut workers = Vec::with_capacity(worker_count);
            for _ in 0..worker_count {
                workers.push(scope.spawn(|| {
                    let mut local = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        let Some(number) = numbers.get(index).copied() else {
                            break;
                        };
                        local.push((index, number, self.fetch_rollup(repo, number)));
                    }
                    local
                }));
            }
            let mut completed = Vec::with_capacity(numbers.len());
            for worker in workers {
                completed.extend(
                    worker
                        .join()
                        .map_err(|_| "rollup collection worker panicked".to_owned())?,
                );
            }
            Ok::<_, String>(completed)
        })?;
        completed.sort_by_key(|(index, _, _)| *index);

        let mut rollups = BTreeMap::new();
        let mut failed = Vec::new();
        for (_, number, outcome) in completed {
            match outcome? {
                Some(checks) => {
                    rollups.insert(number, checks);
                }
                None => failed.push(number),
            }
        }
        if !failed.is_empty() {
            failed.sort_unstable();
            eprintln!(
                "pr-landing-planner: NOTE: rollup fetch failed for {} PR(s) ({}); treating them as pending (no checks)",
                failed.len(),
                failed.iter().map(|number| format!("#{number}")).collect::<Vec<_>>().join(",")
            );
        }
        Ok(rollups)
    }
}

fn json_object() -> Value {
    Value::Object(Map::new())
}

fn string(obj: &Map<String, Value>, key: &str) -> String {
    match obj.get(key) {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Bool(value)) => value.to_string(),
        _ => String::new(),
    }
}

fn integer(obj: &Map<String, Value>, key: &str) -> i64 {
    obj.get(key).and_then(Value::as_i64).unwrap_or(0)
}

fn labels(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
        .filter_map(|entry| entry.get("name").and_then(Value::as_str))
        .map(str::to_owned)
        .collect()
}

impl VcsHost for GitHubHost {
    fn list_open_prs(&mut self, repo: &str, _base: Option<&str>) -> Result<Vec<RawPr>, String> {
        let args = self.net(vec![
            self.gh.clone(),
            "pr".into(),
            "list".into(),
            "--repo".into(),
            repo.into(),
            "--state".into(),
            "open".into(),
            "--limit".into(),
            "500".into(),
            "--json".into(),
            LIGHT_FIELDS.join(","),
        ]);
        let output = self.run(&args, None, &[0])?;
        let value: Value = if String::from_utf8_lossy(&output.stdout).trim().is_empty() {
            Value::Array(Vec::new())
        } else {
            serde_json::from_slice(&output.stdout)
                .map_err(|error| format!("invalid JSON from gh pr list: {error}"))?
        };
        let entries = value
            .as_array()
            .ok_or("expected a JSON array from gh pr list")?;
        let numbers = entries
            .iter()
            .filter_map(Value::as_object)
            .map(|obj| integer(obj, "number"))
            .collect::<Vec<_>>();
        let rollups = self.fetch_rollups(repo, &numbers)?;
        let mut prs = Vec::new();
        for value in entries {
            let Some(obj) = value.as_object() else {
                continue;
            };
            let number = integer(obj, "number");
            let checks = rollups.get(&number).cloned().unwrap_or_default();
            let author = obj
                .get("author")
                .and_then(Value::as_object)
                .and_then(|author| author.get("login"))
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_owned();
            prs.push(RawPr {
                number,
                head_ref: string(obj, "headRefName"),
                base_ref: string(obj, "baseRefName"),
                api_head_sha: string(obj, "headRefOid"),
                title: string(obj, "title"),
                author,
                is_draft: obj.get("isDraft").and_then(Value::as_bool).unwrap_or(false),
                mergeable: string(obj, "mergeable"),
                review_decision: string(obj, "reviewDecision"),
                created_at: string(obj, "createdAt"),
                updated_at: string(obj, "updatedAt"),
                additions: integer(obj, "additions"),
                deletions: integer(obj, "deletions"),
                labels: labels(obj.get("labels")),
                checks,
                mechanism_symbols: Vec::new(),
            });
        }
        Ok(prs)
    }

    fn prefetch_refs(
        &mut self,
        refspecs: &[(String, String)],
    ) -> Result<BTreeMap<String, String>, String> {
        if refspecs.is_empty() {
            return Ok(BTreeMap::new());
        }
        let mut command = vec![
            "git".into(),
            "fetch".into(),
            "--quiet".into(),
            "--no-tags".into(),
            self.remote.clone(),
        ];
        command.extend(
            refspecs
                .iter()
                .map(|(source, dest)| format!("+{source}:{dest}")),
        );
        let args = self.net(command);
        self.run(&args, Some(&self.git_dir), &[0])?;
        let mut resolved = BTreeMap::new();
        for (_, dest) in refspecs {
            let args = vec!["git".into(), "rev-parse".into(), dest.clone()];
            let output = self.run(&args, Some(&self.git_dir), &[0])?;
            resolved.insert(
                dest.clone(),
                String::from_utf8_lossy(&output.stdout).trim().to_owned(),
            );
        }
        Ok(resolved)
    }

    fn merge_tree(&mut self, left: &str, right: &str) -> Result<Vec<String>, String> {
        let args = vec![
            "git".into(),
            "merge-tree".into(),
            "--write-tree".into(),
            "--name-only".into(),
            "--messages".into(),
            left.into(),
            right.into(),
        ];
        let output = self.run(&args, Some(&self.git_dir), &[0, 1])?;
        if output.status.success() {
            return Ok(Vec::new());
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut paths = stdout
            .lines()
            .skip(1)
            .take_while(|line| !line.trim().is_empty())
            .map(|line| line.trim().to_owned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        paths.sort();
        Ok(paths)
    }

    fn is_ancestor(&mut self, ancestor: &str, descendant: &str) -> Result<bool, String> {
        let args = vec![
            "git".into(),
            "merge-base".into(),
            "--is-ancestor".into(),
            ancestor.into(),
            descendant.into(),
        ];
        Ok(self
            .run(&args, Some(&self.git_dir), &[0, 1])?
            .status
            .success())
    }

    fn changed_files(
        &mut self,
        base_sha: &str,
        head_sha: &str,
    ) -> Result<BTreeSet<String>, String> {
        let args = vec![
            "git".into(),
            "merge-base".into(),
            base_sha.into(),
            head_sha.into(),
        ];
        let output = self.run(&args, Some(&self.git_dir), &[0])?;
        let merge_base = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        let args = vec![
            "git".into(),
            "diff".into(),
            "--name-only".into(),
            format!("{merge_base}...{head_sha}"),
        ];
        let output = self.run(&args, Some(&self.git_dir), &[0])?;
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect())
    }

    fn commits_behind(&mut self, head_sha: &str, base_sha: &str) -> Result<i64, String> {
        let args = vec![
            "git".into(),
            "rev-list".into(),
            "--count".into(),
            format!("{head_sha}..{base_sha}"),
        ];
        let output = self.run(&args, Some(&self.git_dir), &[0])?;
        Ok(String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .unwrap_or(0))
    }
}
