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

/// Environment variable naming the real `gh` executable for fleet hosts.
/// On those hosts `gh` on `PATH` may be a stub or pacing wrapper; the
/// variable points at the real GitHub CLI. It is a fallback only: an
/// explicitly configured `github.gh` (via `init --gh`) always wins.
pub const FLEET_REAL_GH_ENV: &str = "FLEET_REAL_GH";

/// Resolve the `gh` command words: a non-empty configured command wins,
/// then the `FLEET_REAL_GH` environment supplement (shell-style words),
/// then plain `gh` on `PATH`. This mirrors `MERGE_QUEUE_AGENTCTL` for
/// notifications.
pub fn resolve_gh_command(config_gh: &[String], fleet_env: Option<&str>) -> Vec<String> {
    if !config_gh.is_empty() {
        return config_gh.to_vec();
    }
    if let Some(env) = fleet_env.filter(|env| !env.trim().is_empty()) {
        if let Ok(words) = shell_words::split(env) {
            if !words.is_empty() {
                return words;
            }
        }
    }
    vec!["gh".to_owned()]
}

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
    /// Reviews (populated only when the list call asked for them).
    #[serde(default)]
    pub reviews: Vec<Review>,
    /// Latest review per reviewer (fallback; see [`Review`]).
    #[serde(default, rename = "latestReviews")]
    pub latest_reviews: Vec<Review>,
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

/// One review of a pull request, as the batched list call reports it.
///
/// The `gh` (GraphQL) shape is `{"state": "APPROVED", "commit": {"oid":
/// "<sha>"}}`; the flat `commit_id` spelling (REST shape) is also accepted
/// so fakes and other hosts can use either. `latestReviews` entries carry
/// an empty commit oid through `gh`, so approval-at-head reads `reviews`
/// (whose commit oid is populated), with `latestReviews` as a fallback for
/// hosts that do populate it.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Review {
    /// Review state (`APPROVED`, `CHANGES_REQUESTED`, `COMMENTED`, ...).
    #[serde(default)]
    pub state: String,
    /// Commit the review was submitted on (GraphQL shape).
    #[serde(default)]
    pub commit: Option<ReviewCommit>,
    /// Commit the review was submitted on (flat REST shape).
    #[serde(default, alias = "commitId")]
    pub commit_id: Option<String>,
}

/// The commit a review was submitted on.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ReviewCommit {
    /// Commit OID.
    #[serde(default, alias = "commit_id", alias = "commitId")]
    pub oid: String,
}

impl Review {
    /// Commit this review was submitted on, in either reported shape.
    pub fn reviewed_commit(&self) -> &str {
        if let Some(commit) = &self.commit {
            if !commit.oid.is_empty() {
                return &commit.oid;
            }
        }
        self.commit_id.as_deref().unwrap_or_default()
    }

    /// Whether this is an approving review of exactly `head` (a prefix of
    /// the head, as elsewhere in the queue, also counts).
    pub fn approves(&self, head: &str) -> bool {
        let reviewed = self.reviewed_commit();
        (self.state.eq_ignore_ascii_case("APPROVED")
            || self.state.eq_ignore_ascii_case("APPROVING"))
            && !reviewed.is_empty()
            && !head.is_empty()
            && (reviewed == head || head.starts_with(reviewed))
    }
}

impl ListedPr {
    /// Whether any reported review approves the current head commit.
    pub fn approved_at_head(&self) -> bool {
        self.reviews
            .iter()
            .chain(self.latest_reviews.iter())
            .any(|review| review.approves(&self.head))
    }
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
        let fleet_env = std::env::var(FLEET_REAL_GH_ENV).ok();
        let program: Vec<String> = resolve_gh_command(&self.config.gh, fleet_env.as_deref());
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
    ///
    /// With `include_reviews`, the same single list call also asks for
    /// `reviews,latestReviews`: the request count and budget charge are
    /// unchanged (still one charged request per hundred results), only the
    /// payload grows. There is never a per-pull-request review call.
    pub fn list_open(
        &self,
        base: &str,
        label: Option<&str>,
        limit: u32,
        include_reviews: bool,
    ) -> Result<Vec<ListedPr>, String> {
        let fields = if include_reviews {
            "number,headRefOid,isDraft,title,author,labels,reviews,latestReviews"
        } else {
            "number,headRefOid,isDraft,title,author,labels"
        };
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
            fields,
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
    fn approval_at_head_reads_reviews_in_gh_and_flat_shapes() {
        let head = "a".repeat(40);
        let old = "b".repeat(40);
        let parse = |text: String| -> ListedPr { serde_json::from_str(&text).unwrap() };
        // gh shape: reviews[].commit.oid is populated at the head.
        let pr = parse(format!(
            r#"{{"number":1,"headRefOid":"{head}","reviews":[{{"state":"APPROVED","commit":{{"oid":"{head}"}},"author":{{"login":"reviewer-fictional"}}}}]}}"#
        ));
        assert!(pr.approved_at_head());
        // A stale approval (review on the old head) does not count.
        let pr = parse(format!(
            r#"{{"number":1,"headRefOid":"{head}","reviews":[{{"state":"APPROVED","commit":{{"oid":"{old}"}}}}]}}"#
        ));
        assert!(!pr.approved_at_head());
        // Non-approving states at the head do not count.
        for state in ["CHANGES_REQUESTED", "COMMENTED", "DISMISSED", "PENDING"] {
            let pr = parse(format!(
                r#"{{"number":1,"headRefOid":"{head}","reviews":[{{"state":"{state}","commit":{{"oid":"{head}"}}}}]}}"#
            ));
            assert!(!pr.approved_at_head(), "{state}");
        }
        // Flat REST shape: commit_id, APPROVING spelling.
        let pr = parse(format!(
            r#"{{"number":1,"headRefOid":"{head}","reviews":[{{"state":"APPROVING","commit_id":"{head}"}}]}}"#
        ));
        assert!(pr.approved_at_head());
        // No reviews at all: not approved.
        let pr = parse(format!(r#"{{"number":1,"headRefOid":"{head}"}}"#));
        assert!(!pr.approved_at_head());
    }

    #[test]
    fn gh_command_prefers_config_then_fleet_env_then_plain_gh() {
        let config = vec!["configured".to_owned(), "--fast".to_owned()];
        assert_eq!(
            resolve_gh_command(&config, Some("/real/gh")),
            config,
            "configured gh must win over the fleet supplement"
        );
        assert_eq!(
            resolve_gh_command(&[], Some("/real/gh")),
            vec!["/real/gh".to_owned()]
        );
        assert_eq!(
            resolve_gh_command(&[], Some("/real/gh --pager off")),
            vec![
                "/real/gh".to_owned(),
                "--pager".to_owned(),
                "off".to_owned()
            ]
        );
        assert_eq!(resolve_gh_command(&[], None), vec!["gh".to_owned()]);
        assert_eq!(resolve_gh_command(&[], Some("  ")), vec!["gh".to_owned()]);
    }

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
