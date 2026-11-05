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

/// Budget charge multiplier for a list call that also carries reviews.
///
/// The list call stays one batched call, but `reviews` and `latestReviews`
/// are two nested connections per pull request, and the hosting service
/// prices a query by the nodes it may return: a page of 100 pull requests
/// with two nested review connections costs about three times a plain page
/// (the page itself plus one page-equivalent per nested connection). So the
/// reviews query is charged 3 requests per hundred results instead of 1.
pub const REVIEWS_COST_FACTOR: u32 = 3;

/// Author associations whose reviews count for approval at head: people
/// with write access to the repository, as the hosting service reports
/// them. A review from anyone else (`CONTRIBUTOR`, `NONE`, ...) never
/// approves and never blocks.
pub const TRUSTED_ASSOCIATIONS: [&str; 3] = ["OWNER", "MEMBER", "COLLABORATOR"];

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
    /// How long one `gh` call may run before its process group is killed and it fails.
    pub timeout: std::time::Duration,
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
    /// The hosting service's overall review verdict (`APPROVED`,
    /// `CHANGES_REQUESTED`, `REVIEW_REQUIRED`, or empty when the base has no
    /// review rule). Populated only when the list call asked for reviews.
    #[serde(default, rename = "reviewDecision")]
    pub review_decision: Option<String>,
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
/// "<sha>"}, "author": {"login": "..."}, "authorAssociation": "MEMBER",
/// "submittedAt": "..."}`; the flat REST spellings (`commit_id`, `user`,
/// `author_association`, `submitted_at`) are also accepted so fakes and other
/// hosts can use either. `latestReviews` entries carry an empty commit oid
/// through `gh`, so approval-at-head reads `reviews` (whose commit oid is
/// populated), with `latestReviews` as a fallback for hosts that do populate
/// it.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Review {
    /// Review state (`APPROVED`, `CHANGES_REQUESTED`, `COMMENTED`, ...).
    #[serde(default)]
    pub state: String,
    /// Reviewer.
    #[serde(default, alias = "user")]
    pub author: Option<Author>,
    /// The reviewer's association with the repository (`OWNER`, `MEMBER`,
    /// `COLLABORATOR`, `CONTRIBUTOR`, `NONE`, ...).
    #[serde(default, rename = "authorAssociation", alias = "author_association")]
    pub author_association: String,
    /// RFC 3339 submission time, used to order reviews when present.
    #[serde(default, rename = "submittedAt", alias = "submitted_at")]
    pub submitted_at: String,
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

    /// Reviewer login, empty when the review names none.
    pub fn reviewer(&self) -> &str {
        self.author
            .as_ref()
            .map_or("", |author| author.login.as_str())
    }

    /// Whether the reviewer is named and has write access
    /// ([`TRUSTED_ASSOCIATIONS`]).
    pub fn trusted(&self) -> bool {
        !self.reviewer().is_empty()
            && TRUSTED_ASSOCIATIONS
                .iter()
                .any(|association| self.author_association.eq_ignore_ascii_case(association))
    }

    /// Whether this review requests changes.
    pub fn requests_changes(&self) -> bool {
        self.state.eq_ignore_ascii_case("CHANGES_REQUESTED")
    }

    /// Whether this review leaves the reviewer's verdict unchanged
    /// (comments and unsubmitted drafts), as on the hosting service.
    fn neutral(&self) -> bool {
        self.state.eq_ignore_ascii_case("COMMENTED") || self.state.eq_ignore_ascii_case("PENDING")
    }

    /// Whether this is an approving review of exactly `head`: both must be
    /// full 40-hex commit ids and equal (an abbreviation never counts).
    pub fn approves(&self, head: &str) -> bool {
        let reviewed = self.reviewed_commit();
        (self.state.eq_ignore_ascii_case("APPROVED")
            || self.state.eq_ignore_ascii_case("APPROVING"))
            && is_full_oid(reviewed)
            && is_full_oid(head)
            && reviewed.eq_ignore_ascii_case(head)
    }
}

/// Whether `text` is a full 40-hex commit id.
fn is_full_oid(text: &str) -> bool {
    text.len() == 40 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

impl ListedPr {
    /// Whether the current head is approved, failing closed.
    ///
    /// Only reviews from trusted reviewers ([`Review::trusted`]) count, and
    /// only each reviewer's latest verdict (comments do not replace one):
    /// approved means at least one such latest verdict approves exactly the
    /// head commit and none requests changes. A `reviewDecision` of
    /// `CHANGES_REQUESTED` or `REVIEW_REQUIRED` also blocks. Missing review
    /// data, an abbreviated head, or reviews without a reviewer or an
    /// association never approve.
    pub fn approved_at_head(&self) -> bool {
        if !is_full_oid(&self.head) {
            return false;
        }
        let decision = self.review_decision.as_deref().unwrap_or_default();
        if decision.eq_ignore_ascii_case("CHANGES_REQUESTED")
            || decision.eq_ignore_ascii_case("REVIEW_REQUIRED")
        {
            return false;
        }
        // `latestReviews` is already one verdict per reviewer: a trusted
        // change request there blocks even if `reviews` was truncated.
        if self
            .latest_reviews
            .iter()
            .any(|review| review.trusted() && review.requests_changes())
        {
            return false;
        }
        let source = if self.reviews.is_empty() {
            &self.latest_reviews
        } else {
            &self.reviews
        };
        // Oldest first: by submission time when every review carries one,
        // else in reported order (the hosting service lists oldest first).
        let mut ordered: Vec<(usize, &Review)> = source.iter().enumerate().collect();
        if ordered
            .iter()
            .all(|(_, review)| !review.submitted_at.is_empty())
        {
            ordered.sort_by(|a, b| a.1.submitted_at.cmp(&b.1.submitted_at).then(a.0.cmp(&b.0)));
        }
        let mut latest: std::collections::BTreeMap<&str, &Review> =
            std::collections::BTreeMap::new();
        for (_, review) in ordered {
            if review.trusted() && !review.neutral() {
                latest.insert(review.reviewer(), review);
            }
        }
        !latest.values().any(|review| review.requests_changes())
            && latest.values().any(|review| review.approves(&self.head))
    }
}

impl Hosting {
    /// A client for `config`, charging the budget file under `state_root` unless the
    /// configuration names a shared one; each call is killed and fails after `timeout`.
    pub fn new(
        config: &GitHubConfig,
        state_root: &Path,
        timeout: std::time::Duration,
    ) -> Result<Self, String> {
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
            timeout,
        })
    }

    fn run(&self, cost: u32, args: &[String]) -> Result<String, String> {
        self.budget.charge(cost)?;
        let fleet_env = std::env::var(FLEET_REAL_GH_ENV).ok();
        let program: Vec<String> = resolve_gh_command(&self.config.gh, fleet_env.as_deref());
        let mut command = Command::new(&program[0]);
        command.args(&program[1..]).args(args);
        let output = crate::bounded::output_within(command, self.timeout, &program.join(" "))?;
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
    /// `reviews,latestReviews,reviewDecision`. It is still one call (there is
    /// never a per-pull-request review call), but it is charged
    /// [`REVIEWS_COST_FACTOR`] requests per hundred results, because the
    /// nested review connections make it that much more expensive.
    pub fn list_open(
        &self,
        base: &str,
        label: Option<&str>,
        limit: u32,
        include_reviews: bool,
    ) -> Result<Vec<ListedPr>, String> {
        let fields = if include_reviews {
            "number,headRefOid,isDraft,title,author,labels,reviews,latestReviews,reviewDecision"
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
        let pages = limit.div_ceil(100).max(1);
        let cost = if include_reviews {
            pages.saturating_mul(REVIEWS_COST_FACTOR)
        } else {
            pages
        };
        let text = self.run(cost, &args)?;
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

    fn parse(text: &str) -> ListedPr {
        serde_json::from_str(text).unwrap()
    }

    /// A gh-shape review: `(state, commit, login, association)`.
    fn review(state: &str, commit: &str, login: &str, association: &str) -> String {
        format!(
            r#"{{"state":"{state}","commit":{{"oid":"{commit}"}},"author":{{"login":"{login}"}},"authorAssociation":"{association}"}}"#
        )
    }

    fn listed(head: &str, reviews: &[String]) -> ListedPr {
        parse(&format!(
            r#"{{"number":1,"headRefOid":"{head}","reviews":[{}]}}"#,
            reviews.join(",")
        ))
    }

    #[test]
    fn approval_at_head_reads_reviews_in_gh_and_flat_shapes() {
        let head = "a".repeat(40);
        let old = "b".repeat(40);
        // gh shape: reviews[].commit.oid is populated at the head.
        let pr = listed(&head, &[review("APPROVED", &head, "reviewer-a", "MEMBER")]);
        assert!(pr.approved_at_head());
        // A stale approval (review on the old head) does not count.
        let pr = listed(&head, &[review("APPROVED", &old, "reviewer-a", "MEMBER")]);
        assert!(!pr.approved_at_head());
        // Non-approving states at the head do not count.
        for state in ["CHANGES_REQUESTED", "COMMENTED", "DISMISSED", "PENDING"] {
            let pr = listed(&head, &[review(state, &head, "reviewer-a", "MEMBER")]);
            assert!(!pr.approved_at_head(), "{state}");
        }
        // Flat REST shape: commit_id, user, author_association, APPROVING.
        let pr = parse(&format!(
            r#"{{"number":1,"headRefOid":"{head}","reviews":[{{"state":"APPROVING","commit_id":"{head}","user":{{"login":"reviewer-a"}},"author_association":"COLLABORATOR"}}]}}"#
        ));
        assert!(pr.approved_at_head());
        // No reviews at all: not approved.
        let pr = parse(&format!(r#"{{"number":1,"headRefOid":"{head}"}}"#));
        assert!(!pr.approved_at_head());
    }

    #[test]
    fn approval_at_head_uses_each_reviewers_latest_verdict() {
        let head = "a".repeat(40);
        // Approved, then changes requested by the same reviewer: blocked.
        let pr = listed(
            &head,
            &[
                review("APPROVED", &head, "reviewer-a", "MEMBER"),
                review("CHANGES_REQUESTED", &head, "reviewer-a", "MEMBER"),
            ],
        );
        assert!(!pr.approved_at_head());
        // Changes requested, then approved: approved.
        let pr = listed(
            &head,
            &[
                review("CHANGES_REQUESTED", &head, "reviewer-a", "MEMBER"),
                review("APPROVED", &head, "reviewer-a", "MEMBER"),
            ],
        );
        assert!(pr.approved_at_head());
        // A later comment does not cancel an approval.
        let pr = listed(
            &head,
            &[
                review("APPROVED", &head, "reviewer-a", "MEMBER"),
                review("COMMENTED", &head, "reviewer-a", "MEMBER"),
            ],
        );
        assert!(pr.approved_at_head());
        // A later dismissal does.
        let pr = listed(
            &head,
            &[
                review("APPROVED", &head, "reviewer-a", "MEMBER"),
                review("DISMISSED", &head, "reviewer-a", "MEMBER"),
            ],
        );
        assert!(!pr.approved_at_head());
        // Submission times, when every review has one, order the verdicts.
        let pr = parse(&format!(
            r#"{{"number":1,"headRefOid":"{head}","reviews":[
                {{"state":"CHANGES_REQUESTED","commit":{{"oid":"{head}"}},"author":{{"login":"reviewer-a"}},"authorAssociation":"MEMBER","submittedAt":"2026-01-02T00:00:00Z"}},
                {{"state":"APPROVED","commit":{{"oid":"{head}"}},"author":{{"login":"reviewer-a"}},"authorAssociation":"MEMBER","submittedAt":"2026-01-01T00:00:00Z"}}]}}"#
        ));
        assert!(!pr.approved_at_head());
    }

    #[test]
    fn approval_at_head_counts_only_trusted_reviewers_and_any_change_request_blocks() {
        let head = "a".repeat(40);
        for association in ["CONTRIBUTOR", "FIRST_TIME_CONTRIBUTOR", "NONE", ""] {
            let pr = listed(
                &head,
                &[review("APPROVED", &head, "reviewer-a", association)],
            );
            assert!(!pr.approved_at_head(), "{association:?} must not approve");
        }
        for association in TRUSTED_ASSOCIATIONS {
            let pr = listed(
                &head,
                &[review("APPROVED", &head, "reviewer-a", association)],
            );
            assert!(pr.approved_at_head(), "{association} approves");
        }
        // A review without a reviewer never counts.
        let pr = parse(&format!(
            r#"{{"number":1,"headRefOid":"{head}","reviews":[{{"state":"APPROVED","commit":{{"oid":"{head}"}},"authorAssociation":"MEMBER"}}]}}"#
        ));
        assert!(!pr.approved_at_head());
        // One trusted reviewer's change request blocks another's approval.
        let pr = listed(
            &head,
            &[
                review("APPROVED", &head, "reviewer-a", "OWNER"),
                review("CHANGES_REQUESTED", &head, "reviewer-b", "COLLABORATOR"),
            ],
        );
        assert!(!pr.approved_at_head());
        // An untrusted change request does not block.
        let pr = listed(
            &head,
            &[
                review("APPROVED", &head, "reviewer-a", "OWNER"),
                review("CHANGES_REQUESTED", &head, "drive-by", "NONE"),
            ],
        );
        assert!(pr.approved_at_head());
        // A trusted change request in latestReviews blocks too.
        let pr = parse(&format!(
            r#"{{"number":1,"headRefOid":"{head}","reviews":[{}],"latestReviews":[{{"state":"CHANGES_REQUESTED","commit":{{"oid":""}},"author":{{"login":"reviewer-b"}},"authorAssociation":"MEMBER"}}]}}"#,
            review("APPROVED", &head, "reviewer-a", "MEMBER")
        ));
        assert!(!pr.approved_at_head());
    }

    #[test]
    fn approval_at_head_requires_full_oids_and_honours_review_decision() {
        let head = "a".repeat(40);
        // An abbreviated review commit never matches.
        let pr = listed(
            &head,
            &[review("APPROVED", &head[..12], "reviewer-a", "MEMBER")],
        );
        assert!(!pr.approved_at_head());
        // An abbreviated or malformed head never approves.
        let short = &head[..12];
        let pr = listed(short, &[review("APPROVED", short, "reviewer-a", "MEMBER")]);
        assert!(!pr.approved_at_head());
        // Case differences in hex are the same commit.
        let pr = listed(
            &"A".repeat(40),
            &[review("APPROVED", &head, "reviewer-a", "MEMBER")],
        );
        assert!(pr.approved_at_head());
        // The hosting service's own verdict can block.
        for decision in ["CHANGES_REQUESTED", "REVIEW_REQUIRED"] {
            let pr = parse(&format!(
                r#"{{"number":1,"headRefOid":"{head}","reviewDecision":"{decision}","reviews":[{}]}}"#,
                review("APPROVED", &head, "reviewer-a", "MEMBER")
            ));
            assert!(!pr.approved_at_head(), "{decision}");
        }
        let pr = parse(&format!(
            r#"{{"number":1,"headRefOid":"{head}","reviewDecision":"APPROVED","reviews":[{}]}}"#,
            review("APPROVED", &head, "reviewer-a", "MEMBER")
        ));
        assert!(pr.approved_at_head());
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
