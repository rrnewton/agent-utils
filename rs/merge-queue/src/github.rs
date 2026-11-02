//! The hosting service's API, behind a request budget.
//!
//! The queue itself needs no API call to plan, validate or land: heads arrive by one batched
//! `git fetch`, and landing is a push. The API is used only to read a ready source in one batched
//! call (`sync`) and, optionally, to close pull requests after landing. Every call goes through a
//! per-host budget (requests per rolling hour, shared by every queue that points at the same
//! budget file), because the hosting service's credentials are a scarce shared resource.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use crate::state::GitHubConfig;

/// Default requests per rolling hour when the configuration names none.
pub const DEFAULT_MAX_PER_HOUR: u32 = 60;

/// Requests recorded in the rolling hour.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Ledger {
    /// Unix seconds of each charged request, oldest first.
    requests: Vec<u64>,
}

/// A request budget stored in one bounded file (it never holds more than an hour of entries).
#[derive(Clone, Debug)]
pub struct Budget {
    /// Budget file.
    pub path: PathBuf,
    /// Requests allowed per rolling hour.
    pub max_per_hour: u32,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

impl Budget {
    /// Charge `cost` requests, or refuse with when the next one becomes available.
    pub fn charge(&self, cost: u32) -> Result<(), String> {
        self.charge_at(cost, now_secs())
    }

    fn charge_at(&self, cost: u32, now: u64) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&self.path)
            .map_err(|error| format!("cannot open {}: {error}", self.path.display()))?;
        file.lock_exclusive()
            .map_err(|error| format!("cannot lock {}: {error}", self.path.display()))?;
        let text = std::fs::read_to_string(&self.path).unwrap_or_default();
        let mut ledger: Ledger = serde_json::from_str(&text).unwrap_or_default();
        ledger.requests.retain(|at| now.saturating_sub(*at) < 3600);
        let used = ledger.requests.len() as u32;
        if used + cost > self.max_per_hour {
            let _ = FileExt::unlock(&file);
            let wait = ledger
                .requests
                .first()
                .map_or(0, |oldest| 3600u64.saturating_sub(now - oldest));
            return Err(format!(
                "API request budget exhausted: {used} of {} used in the last hour, {cost} more needed; retry in about {} min",
                self.max_per_hour,
                wait.div_ceil(60)
            ));
        }
        ledger
            .requests
            .extend(std::iter::repeat_n(now, cost as usize));
        let written = serde_json::to_string(&ledger)
            .map_err(|error| error.to_string())
            .and_then(|text| {
                std::fs::write(&self.path, text)
                    .map_err(|error| format!("cannot write {}: {error}", self.path.display()))
            });
        let _ = FileExt::unlock(&file);
        written
    }

    /// Requests left in the rolling hour.
    pub fn remaining(&self) -> u32 {
        let text = std::fs::read_to_string(&self.path).unwrap_or_default();
        let ledger: Ledger = serde_json::from_str(&text).unwrap_or_default();
        let now = now_secs();
        let used = ledger
            .requests
            .iter()
            .filter(|at| now.saturating_sub(**at) < 3600)
            .count() as u32;
        self.max_per_hour.saturating_sub(used)
    }
}

/// A budgeted client for the hosting service's command-line tool.
#[derive(Clone, Debug)]
pub struct Hosting {
    /// Configuration.
    pub config: GitHubConfig,
    /// Budget.
    pub budget: Budget,
}

/// One pull request as the ready source reports it.
#[derive(Clone, Debug, Deserialize)]
pub struct ListedPr {
    /// Number.
    pub number: i64,
    /// Head commit.
    #[serde(rename = "headRefOid")]
    pub head: String,
    /// Draft flag.
    #[serde(rename = "isDraft", default)]
    pub draft: bool,
    /// Title.
    #[serde(default)]
    pub title: String,
    /// Author.
    #[serde(default)]
    pub author: Option<Author>,
    /// Labels.
    #[serde(default)]
    pub labels: Vec<Label>,
}

/// Author of a pull request.
#[derive(Clone, Debug, Deserialize)]
pub struct Author {
    /// Login.
    #[serde(default)]
    pub login: String,
}

/// A label.
#[derive(Clone, Debug, Deserialize)]
pub struct Label {
    /// Name.
    pub name: String,
}

impl Hosting {
    /// A client for `config`, charging the budget file under `state_root` unless the
    /// configuration names a shared one.
    pub fn new(config: &GitHubConfig, state_root: &Path) -> Result<Self, String> {
        if config.repo.is_empty() {
            return Err("github.repo (owner/name) is not configured".to_owned());
        }
        let path = if config.budget_file.is_empty() {
            state_root.join("api-budget.json")
        } else {
            PathBuf::from(&config.budget_file)
        };
        let max_per_hour = if config.max_per_hour == 0 {
            DEFAULT_MAX_PER_HOUR
        } else {
            config.max_per_hour
        };
        Ok(Self {
            config: config.clone(),
            budget: Budget { path, max_per_hour },
        })
    }

    fn run(&self, cost: u32, args: &[String]) -> Result<String, String> {
        self.budget.charge(cost)?;
        let program: Vec<String> = if self.config.gh.is_empty() {
            vec!["gh".to_owned()]
        } else {
            self.config.gh.clone()
        };
        let output = Command::new(&program[0])
            .args(&program[1..])
            .args(args)
            .output()
            .map_err(|error| format!("cannot run {}: {error}", program.join(" ")))?;
        if !output.status.success() {
            return Err(format!(
                "{} {} failed: {}",
                program.join(" "),
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    /// Open pull requests targeting `base`, optionally only those with `label`, in one batched
    /// list call (charged one request per hundred results requested).
    pub fn list_open(
        &self,
        base: &str,
        label: Option<&str>,
        limit: u32,
    ) -> Result<Vec<ListedPr>, String> {
        let mut args: Vec<String> = [
            "pr",
            "list",
            "--repo",
            &self.config.repo,
            "--base",
            base,
            "--state",
            "open",
            "--limit",
            &limit.to_string(),
            "--json",
            "number,headRefOid,isDraft,title,author,labels",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        if let Some(label) = label {
            args.push("--label".to_owned());
            args.push(label.to_owned());
        }
        let text = self.run(limit.div_ceil(100).max(1), &args)?;
        serde_json::from_str(&text).map_err(|error| format!("unexpected pr list output: {error}"))
    }

    /// Close a landed pull request with a pointer to the commit that contains it.
    pub fn close(&self, pr: i64, comment: &str) -> Result<(), String> {
        let args: Vec<String> = [
            "pr",
            "close",
            &pr.to_string(),
            "--repo",
            &self.config.repo,
            "--comment",
            comment,
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        self.run(1, &args).map(drop)
    }
}

#[cfg(test)]
mod unit {
    use super::*;
    use crate::git::testrepo::TempDir;

    #[test]
    fn budget_refuses_past_the_hourly_limit_and_recovers() {
        let tmp = TempDir::new("budget");
        let budget = Budget {
            path: tmp.0.join("b/api-budget.json"),
            max_per_hour: 3,
        };
        budget.charge_at(2, 1_000).unwrap();
        budget.charge_at(1, 1_100).unwrap();
        let refused = budget.charge_at(1, 1_200).unwrap_err();
        assert!(refused.contains("3 of 3"), "{refused}");
        assert!(refused.contains("retry in about 57 min"), "{refused}");
        // An hour after the first two, they no longer count.
        budget.charge_at(2, 4_600).unwrap();
        assert!(budget.charge_at(1, 4_601).is_err());
    }
}
