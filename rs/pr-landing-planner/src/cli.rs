//! Package-facing command-line interface.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::classify::{flaky_signatures_from_value, ClassifyConfig};
use crate::collect::{collect_graph, CollectOptions};
use crate::context::parse_landing_context;
use crate::emit::{
    render_actions, render_clusters_human, render_clusters_json, render_graph_human,
    render_graph_json, render_human, render_json, render_status_human, render_status_json,
};
use crate::fixture::{load_fixture_text, FakeHost};
use crate::host::{GitHubHost, VcsHost};
use crate::model::{PlanResult, DEFAULT_BASE, DEFAULT_GATE_CHECK, DEFAULT_REPO};
use crate::plan::{assemble_result, DEFAULT_FRESHNESS_MAX_BEHIND};
use crate::priority::{make_priority_provider, DEFAULT_LABEL_PATTERN};
use crate::VERSION;

/// Installed executable name.
pub const PROG: &str = "pr-landing-planner";
/// Default open-PR count at which the status view emits a warning.
pub const DEFAULT_WARN_THRESHOLD: usize = 8;
const USER_GUIDE: &str = include_str!("embedded_userguide.md");

const DEMO_FIXTURE: &str = r#"repo: OWNER/NAME
base: main
prs:
  - number: 1043
    title: fast, fresh, all green
    head_ref: feat-a
    additions: 10
    labels: [validated-locally]
    changed_files: [src/a.rs]
    checks:
      - {name: CI, status: COMPLETED, conclusion: SUCCESS}
      - {name: merge-gate, status: COMPLETED, conclusion: SUCCESS}
  - number: 987
    title: green but 6 behind base
    head_ref: feat-b
    additions: 40
    commits_behind: 6
    changed_files: [src/b.rs]
    checks:
      - {name: CI, status: COMPLETED, conclusion: SUCCESS}
      - {name: merge-gate, status: COMPLETED, conclusion: SUCCESS}
  - number: 942
    title: CI green, required gate stale
    head_ref: feat-c
    changed_files: [src/c.rs]
    checks:
      - {name: CI, status: COMPLETED, conclusion: SUCCESS}
      - {name: merge-gate, status: COMPLETED, conclusion: FAILURE, text: stale result}
  - number: 1050
    title: gate fired while CI queued
    head_ref: feat-d
    changed_files: [src/d.rs]
    checks:
      - {name: CI, status: IN_PROGRESS, conclusion: ""}
      - {name: merge-gate, status: COMPLETED, conclusion: FAILURE, text: "Full CI still queued; rerun after CI completes"}
  - number: 1049
    title: red on wasm-core (add a --flaky-signatures file to reclassify as flaky)
    head_ref: feat-e
    changed_files: [src/e.rs]
    checks:
      - {name: wasm-core, status: COMPLETED, conclusion: FAILURE, text: browser flake}
      - {name: merge-gate, status: COMPLETED, conclusion: SUCCESS}
conflicts:
  - {a: 987, b: 942, paths: [src/shared.rs]}
"#;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommandKind {
    Plan,
    Graph,
    Clusters,
    Status,
    Quickstart,
}

#[derive(Debug)]
struct Options {
    command: Option<CommandKind>,
    userguide: bool,
    help: bool,
    emit_demo: bool,
    repo: String,
    base: String,
    prs: Option<String>,
    git_dir: PathBuf,
    remote: String,
    landing_context: Option<PathBuf>,
    net_wrapper: String,
    gh_cmd: String,
    conflict_detector: String,
    fixture: Option<PathBuf>,
    gate_check: String,
    flaky_signatures: Option<PathBuf>,
    outage_min_prs: usize,
    freshness_max_behind: Option<i64>,
    priority_source: String,
    priority_label_pattern: String,
    priority_cmd: String,
    batch: bool,
    format: String,
    archive_dir: Option<PathBuf>,
    no_archive: bool,
    warn_threshold: usize,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            command: None,
            userguide: false,
            help: false,
            emit_demo: false,
            repo: DEFAULT_REPO.to_owned(),
            base: DEFAULT_BASE.to_owned(),
            prs: None,
            git_dir: PathBuf::from("."),
            remote: "origin".to_owned(),
            landing_context: None,
            net_wrapper: String::new(),
            gh_cmd: "gh".to_owned(),
            conflict_detector: "merge-tree".to_owned(),
            fixture: None,
            gate_check: DEFAULT_GATE_CHECK.to_owned(),
            flaky_signatures: None,
            outage_min_prs: 2,
            freshness_max_behind: DEFAULT_FRESHNESS_MAX_BEHIND,
            priority_source: "none".to_owned(),
            priority_label_pattern: DEFAULT_LABEL_PATTERN.to_owned(),
            priority_cmd: String::new(),
            batch: false,
            format: "human".to_owned(),
            archive_dir: None,
            no_archive: false,
            warn_threshold: DEFAULT_WARN_THRESHOLD,
        }
    }
}

fn help() -> String {
    format!(
        "{PROG} v{VERSION}\nA conflict-graph + CI-aware, advisory pull-request landing planner.\n\nUSAGE:\n  {PROG} <command> [OPTIONS]\n  {PROG} --userguide\n\nCOMMANDS:\n  plan        build the graph and print the fused landing plan\n  graph       print the conflict and ordering graph\n  clusters    print real-conflict stack landing lanes\n  status      print per-PR CI health\n  quickstart  print a self-contained getting-started guide\n\nCOMMON OPTIONS:\n  --repo OWNER/NAME          repository (required for live collection)\n  --base BRANCH              base branch (default: {DEFAULT_BASE})\n  --fixture FILE             deterministic JSON/YAML input instead of a live host\n  --format FORMAT            human/json; plan also supports actions\n  --git-dir DIR              local clone for live collection\n  --prs N,N                  restrict to PR numbers\n  --conflict-detector KIND   merge-tree or file-overlap\n  --gate-check NAME          required gate name (default: {DEFAULT_GATE_CHECK})\n  --landing-context FILE     exact-head evidence, validation authority, policy, and agent context\n  --flaky-signatures FILE    known flaky name/text regexes\n  --net-wrapper COMMAND      prefix live network commands\n  --gh-cmd PATH              gh-compatible executable\n\nPLAN OPTIONS:\n  --outage-min-prs N         minimum correlated failures for outage classification\n  --freshness-max-behind N   caller override (default: none; being behind is not a rebase reason)\n  --priority-source SOURCE   none, labels, or command\n  --priority-label-pattern R label priority pattern (default: {DEFAULT_LABEL_PATTERN})\n  --priority-cmd COMMAND     command used by the command priority source\n  --batch                    emit a conflict/dependency-free root batch\n  --archive-dir DIR          directory for canonical JSON plan archives\n  --no-archive               disable the default live-run archive\n\nSTATUS OPTIONS:\n  --warn-threshold N         unhealthy-PR warning threshold (default: {DEFAULT_WARN_THRESHOLD})\n\nQUICKSTART OPTIONS:\n  --emit-demo                print the bundled deterministic fixture\n\nGENERAL OPTIONS:\n  -h, --help                 print help\n  --version                  print version\n  --userguide                print the complete embedded reference\n\nRun `{PROG} quickstart` for examples. The tool never mutates a PR."
    )
}

fn quickstart() -> String {
    format!(
        "{PROG} v{VERSION}\nA conflict-graph + CI-aware, ADVISORY pull-request landing planner.\n\nThe idea\n  Fuse real merge conflicts, exact-head CI evidence, caller validation authority, freshness, holds,\n  priority, and semantic mechanism overlaps into deterministic per-PR actions. The planner never\n  changes a PR.\n\n1. Install\n  cargo install pr-landing-planner\n\n2. Try the bundled fixture (no repository or network)\n  {PROG} quickstart --emit-demo > demo.yaml\n  {PROG} plan --fixture demo.yaml\n\n3. Run against a live repository\n  {PROG} plan --repo OWNER/NAME --base main --git-dir /path/to/clone\n\nRed classifications\n  real -> hold-fix | flaky -> refire-ci | stale-required-check -> refire-stale-gate\n  evaluate-once-race -> wait | runner-outage -> escalate-runner-outage\n\nOutput\n  --format human | json | actions (plan only)\n  JSON is deterministic; actions starts with capturable key=value counts.\n\nUse `{PROG} --userguide` for all flags, fixture fields, and landing-context semantics.\n\nDemo fixture\n{DEMO_FIXTURE}"
    )
}

fn parse_command(value: &str) -> Option<CommandKind> {
    match value {
        "plan" => Some(CommandKind::Plan),
        "graph" => Some(CommandKind::Graph),
        "clusters" => Some(CommandKind::Clusters),
        "status" => Some(CommandKind::Status),
        "quickstart" => Some(CommandKind::Quickstart),
        _ => None,
    }
}

fn required_value(
    args: &[String],
    index: &mut usize,
    inline: Option<&str>,
    flag: &str,
) -> Result<String, String> {
    if let Some(value) = inline {
        if value.is_empty() {
            return Err(format!("{flag} requires a value"));
        }
        return Ok(value.to_owned());
    }
    *index += 1;
    args.get(*index)
        .filter(|value| !value.is_empty())
        .cloned()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut options = Options::default();
    let mut index = 0;
    while index < args.len() {
        let argument = &args[index];
        if !argument.starts_with('-') {
            if options.command.is_some() {
                return Err(format!("unexpected positional argument {argument:?}"));
            }
            options.command = Some(
                parse_command(argument).ok_or_else(|| format!("unknown command {argument:?}"))?,
            );
            index += 1;
            continue;
        }
        let (flag, inline) = argument
            .split_once('=')
            .map_or((argument.as_str(), None), |(flag, value)| {
                (flag, Some(value))
            });
        let mut value = || required_value(args, &mut index, inline, flag);
        match flag {
            "-h" | "--help" => options.help = true,
            "--userguide" => options.userguide = true,
            "--version" => return Err("__VERSION__".to_owned()),
            "--emit-demo" => options.emit_demo = true,
            "--batch" => options.batch = true,
            "--no-archive" => options.no_archive = true,
            "--repo" => options.repo = value()?,
            "--base" => options.base = value()?,
            "--prs" => options.prs = Some(value()?),
            "--git-dir" => options.git_dir = PathBuf::from(value()?),
            "--remote" => options.remote = value()?,
            "--landing-context" => options.landing_context = Some(PathBuf::from(value()?)),
            "--net-wrapper" => options.net_wrapper = value()?,
            "--gh-cmd" => options.gh_cmd = value()?,
            "--conflict-detector" => options.conflict_detector = value()?,
            "--fixture" => options.fixture = Some(PathBuf::from(value()?)),
            "--gate-check" => options.gate_check = value()?,
            "--flaky-signatures" => options.flaky_signatures = Some(PathBuf::from(value()?)),
            "--outage-min-prs" => {
                options.outage_min_prs = parse_nonnegative_usize(&value()?, flag)?
            }
            "--freshness-max-behind" => {
                options.freshness_max_behind = Some(parse_number(&value()?, flag)?)
            }
            "--priority-source" => options.priority_source = value()?,
            "--priority-label-pattern" => options.priority_label_pattern = value()?,
            "--priority-cmd" => options.priority_cmd = value()?,
            "--format" => options.format = value()?,
            "--archive-dir" => options.archive_dir = Some(PathBuf::from(value()?)),
            "--warn-threshold" => {
                options.warn_threshold = parse_nonnegative_usize(&value()?, flag)?
            }
            _ => return Err(format!("unknown option {flag:?}")),
        }
        index += 1;
    }
    validate(&options)?;
    Ok(options)
}

fn parse_number<T: std::str::FromStr>(value: &str, flag: &str) -> Result<T, String> {
    value
        .parse()
        .map_err(|_| format!("{flag} expects an integer, got {value:?}"))
}

fn parse_nonnegative_usize(value: &str, flag: &str) -> Result<usize, String> {
    let parsed: i64 = parse_number(value, flag)?;
    usize::try_from(parsed)
        .map_err(|_| format!("{flag} expects a nonnegative integer, got {value:?}"))
}

fn validate(options: &Options) -> Result<(), String> {
    if options
        .freshness_max_behind
        .is_some_and(|freshness| freshness < 0)
    {
        return Err("--freshness-max-behind must be nonnegative".to_owned());
    }
    if options.gate_check.trim().is_empty() {
        return Err("--gate-check must be non-empty".to_owned());
    }
    if !matches!(
        options.conflict_detector.as_str(),
        "merge-tree" | "file-overlap"
    ) {
        return Err("--conflict-detector must be merge-tree or file-overlap".to_owned());
    }
    if !matches!(
        options.priority_source.as_str(),
        "none" | "labels" | "command"
    ) {
        return Err("--priority-source must be none, labels, or command".to_owned());
    }
    let valid_format = match options.command {
        Some(CommandKind::Plan) => matches!(options.format.as_str(), "human" | "json" | "actions"),
        Some(CommandKind::Graph | CommandKind::Clusters | CommandKind::Status) => {
            matches!(options.format.as_str(), "human" | "json")
        }
        _ => true,
    };
    if !valid_format {
        return Err(format!(
            "unsupported --format {:?} for this command",
            options.format
        ));
    }
    Ok(())
}

fn load_doc(path: &Path) -> Result<Value, String> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    let yaml = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "yaml" | "yml"));
    load_fixture_text(&text, yaml)
}

fn only_numbers(raw: Option<&str>) -> Result<Option<BTreeSet<i64>>, String> {
    raw.map(|raw| {
        raw.split(',')
            .filter(|value| !value.trim().is_empty())
            .map(|value| {
                let number: i64 = value
                    .trim()
                    .parse()
                    .map_err(|_| format!("invalid PR number {value:?}"))?;
                if number <= 0 {
                    return Err(format!(
                        "invalid PR number {value:?}: expected a positive i64"
                    ));
                }
                Ok(number)
            })
            .collect()
    })
    .transpose()
}

fn build_result(options: &Options) -> Result<PlanResult, String> {
    let wrapper = if options.net_wrapper.is_empty() {
        Vec::new()
    } else {
        shell_words::split(&options.net_wrapper)
            .map_err(|error| format!("invalid --net-wrapper: {error}"))?
    };
    let (mut host, repo, base): (Box<dyn VcsHost>, String, String) = if let Some(path) =
        &options.fixture
    {
        let (host, repo, base) = FakeHost::from_value(&load_doc(path)?)?;
        (Box::new(host), repo, base)
    } else {
        if options.repo.trim().is_empty() {
            return Err(
                "--repo OWNER/NAME is required for live runs (or use --fixture FILE)".to_owned(),
            );
        }
        (
            Box::new(GitHubHost::new(
                options.git_dir.clone(),
                options.remote.clone(),
                wrapper.clone(),
                options.gh_cmd.clone(),
            )),
            options.repo.clone(),
            options.base.clone(),
        )
    };
    let landing_context = options
        .landing_context
        .as_ref()
        .map(|path| parse_landing_context(&load_doc(path)?))
        .transpose()?
        .unwrap_or_default();
    let mut classify = ClassifyConfig {
        gate_check: options.gate_check.clone(),
        outage_min_prs: options.outage_min_prs,
        ..ClassifyConfig::default()
    };
    if let Some(path) = &options.flaky_signatures {
        classify.flaky_signatures = flaky_signatures_from_value(&load_doc(path)?);
    }
    classify.validate()?;
    let mut priority = make_priority_provider(
        &options.priority_source,
        &options.priority_label_pattern,
        &options.priority_cmd,
        &wrapper,
    )?;
    let only = only_numbers(options.prs.as_deref())?;
    let graph = collect_graph(
        host.as_mut(),
        CollectOptions {
            repo: &repo,
            base: &base,
            only: only.as_ref(),
            conflict_detector: &options.conflict_detector,
            classify_config: &classify,
            priority_provider: priority.as_mut(),
            landing_context: &landing_context,
        },
    )?;
    if let Some(error) = priority.last_error() {
        return Err(error.to_owned());
    }
    Ok(assemble_result(
        graph,
        options.freshness_max_behind,
        options.outage_min_prs,
        options.batch,
    ))
}

fn archive_dir(options: &Options) -> Option<PathBuf> {
    if options.no_archive {
        return None;
    }
    if let Some(path) = &options.archive_dir {
        return Some(expand_user(path));
    }
    if options.fixture.is_some() {
        return None;
    }
    if let Some(path) = std::env::var_os("PR_LANDING_PLANNER_ARCHIVE_DIR") {
        return Some(expand_user(Path::new(&path)));
    }
    if let Some(path) = std::env::var_os("XDG_STATE_HOME") {
        return Some(expand_user(Path::new(&path)).join("pr-landing-planner/plans"));
    }
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|path| path.join(".local/state/pr-landing-planner/plans"))
}

fn expand_user(path: &Path) -> PathBuf {
    let Some(raw) = path.to_str() else {
        return path.to_owned();
    };
    if raw == "~" {
        return std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| path.to_owned());
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    path.to_owned()
}

fn utc_stamp(now: std::time::Duration) -> String {
    let seconds = now.as_secs();
    let days = (seconds / 86_400) as i64;
    let seconds_in_day = seconds % 86_400;

    // Convert days since 1970-01-01 to a proleptic Gregorian date. This is the
    // civil-from-days algorithm, shifted so day zero is the Unix epoch.
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);

    let hour = seconds_in_day / 3_600;
    let minute = seconds_in_day % 3_600 / 60;
    let second = seconds_in_day % 60;
    format!(
        "{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}_{:06}Z",
        now.subsec_micros()
    )
}

fn archive(options: &Options, result: &PlanResult) {
    let Some(directory) = archive_dir(options) else {
        return;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let repo = result.graph.repository.replace('/', "_");
    let base = result.graph.base.replace('/', "_");
    let path = directory.join(format!(
        "plan-{}-{}-{}.json",
        if repo.is_empty() { "repo" } else { &repo },
        if base.is_empty() { "base" } else { &base },
        utc_stamp(now),
    ));
    let write = fs::create_dir_all(&directory)
        .and_then(|()| fs::write(&path, format!("{}\n", render_json(result))));
    match write {
        Ok(()) => eprintln!("{PROG}: NOTE: plan archived to {}", path.display()),
        Err(error) => eprintln!(
            "{PROG}: WARN: could not archive plan to {}: {error}",
            path.display()
        ),
    }
}

fn run(options: &Options) -> Result<String, String> {
    let result = build_result(options)?;
    let output = match options.command {
        Some(CommandKind::Plan) => {
            let output = match options.format.as_str() {
                "json" => render_json(&result),
                "actions" => render_actions(&result),
                _ => render_human(&result),
            };
            archive(options, &result);
            output
        }
        Some(CommandKind::Graph) => match options.format.as_str() {
            "json" => render_graph_json(&result),
            _ => render_graph_human(&result),
        },
        Some(CommandKind::Clusters) => match options.format.as_str() {
            "json" => render_clusters_json(&result),
            _ => render_clusters_human(&result),
        },
        Some(CommandKind::Status) => match options.format.as_str() {
            "json" => render_status_json(&result),
            _ => render_status_human(&result, options.warn_threshold),
        },
        _ => return Err("no collection command selected".to_owned()),
    };
    Ok(output)
}

/// Run the command-line interface with arguments excluding the executable name.
pub fn main(args: &[String]) -> i32 {
    let options = match parse(args) {
        Ok(options) => options,
        Err(error) if error == "__VERSION__" => {
            println!("{PROG} {VERSION}");
            return 0;
        }
        Err(error) => {
            eprintln!("{PROG}: {error}");
            eprintln!("Try '{PROG} --help' for usage.");
            return 2;
        }
    };
    if options.userguide {
        print!("{USER_GUIDE}");
        return 0;
    }
    if options.help || options.command.is_none() {
        println!("{}", help());
        return 0;
    }
    if options.command == Some(CommandKind::Quickstart) {
        if options.emit_demo {
            print!("{DEMO_FIXTURE}");
        } else {
            println!("{}", quickstart());
        }
        return 0;
    }
    match run(&options) {
        Ok(output) => {
            println!("{output}");
            0
        }
        Err(error) => {
            eprintln!("{PROG}: {error}");
            2
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_equals_forms_and_rejects_unknown_formats() {
        let options = parse(&[
            "plan".into(),
            "--fixture=x.yaml".into(),
            "--format=actions".into(),
        ])
        .unwrap();
        assert_eq!(options.format, "actions");
        assert!(parse(&["status".into(), "--format=actions".into()]).is_err());
    }

    #[test]
    fn docs_are_rust_package_specific() {
        assert!(quickstart().contains("cargo install"));
        assert!(USER_GUIDE.contains("cargo install pr-landing-planner"));
        assert!(USER_GUIDE.contains("pr-landing-planner = \"0.1\""));
        assert!(help().contains("none, labels, or command"));
        assert!(!help().to_lowercase().contains("beads"));
        // G1: replaces the differential `version` byte-equality case and the
        // Rust half of guide:py/rs (compare_package_guides). The literal
        // golden fails if the version bumps without updating this test.
        assert_eq!(format!("{PROG} {VERSION}\n"), "pr-landing-planner 0.1.0\n");
        let (code, version, _) = invoke(&["--version"]);
        assert_eq!(code, 0);
        assert_eq!(version, "pr-landing-planner 0.1.0\n");
        let (code, guide, _) = invoke(&["--userguide"]);
        assert_eq!(code, 0);
        assert!(guide.len() > 500, "guide too short: {}", guide.len());
        assert!(guide.to_lowercase().contains("pr-landing-planner"));
        for leak in ["pip install", "pypi", "python api", "py/"] {
            assert!(
                !guide.to_lowercase().contains(leak),
                "rust guide leaks python-edition token {leak:?}"
            );
        }
    }

    #[test]
    fn archive_timestamp_matches_the_utc_filename_contract() {
        assert_eq!(
            utc_stamp(std::time::Duration::new(0, 123_456_789)),
            "19700101T000000_123456Z"
        );
        assert_eq!(
            utc_stamp(std::time::Duration::new(1_709_210_096, 987_654_321)),
            "20240229T123456_987654Z"
        );
    }

    // ---- Retirement replacements (G1-G8, #2139): the cross.differential
    // pr-landing-planner cases were the only coverage for the CLI surface
    // contract below. Each test names the differential case it replaces.

    fn invoke(args: &[&str]) -> (i32, String, String) {
        // Mirror cli::main: same exit codes and stdout/stderr shapes, in-process.
        let owned: Vec<String> = args.iter().map(|value| value.to_string()).collect();
        match parse(&owned) {
            Err(error) if error == "__VERSION__" => {
                (0, format!("{PROG} {VERSION}\n"), String::new())
            }
            Err(error) => (
                2,
                String::new(),
                format!("{PROG}: {error}\nTry '{PROG} --help' for usage.\n"),
            ),
            Ok(options) => {
                if options.userguide {
                    return (0, USER_GUIDE.to_owned(), String::new());
                }
                if options.help || options.command.is_none() {
                    return (0, format!("{}\n", help()), String::new());
                }
                if options.command == Some(CommandKind::Quickstart) {
                    return if options.emit_demo {
                        (0, DEMO_FIXTURE.to_owned(), String::new())
                    } else {
                        (0, format!("{}\n", quickstart()), String::new())
                    };
                }
                match run(&options) {
                    Ok(output) => (0, format!("{output}\n"), String::new()),
                    Err(error) => (2, String::new(), format!("{PROG}: {error}\n")),
                }
            }
        }
    }

    fn temp_file(name: &str, text: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let nonce = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("planner-cli-{}-{nonce}-{name}", std::process::id()));
        fs::write(&path, text).unwrap();
        path
    }

    fn sha256_hex(text: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut digest = Sha256::new();
        digest.update(text.as_bytes());
        format!("{:x}", digest.finalize())
    }

    /// The differential's 14-PR representative fixture, frozen verbatim (G3).
    const REPRESENTATIVE_FIXTURE: &str = r###"{"repo":"OWNER/NAME","base":"main","prs":[{"number":1,"title":"green","head_ref":"green","additions":4,"changed_files":["src/a.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"SUCCESS"},{"name":"merge-gate","status":"COMPLETED","conclusion":"SUCCESS"}]},{"number":2,"title":"behind","head_ref":"behind","commits_behind":3,"changed_files":["src/b.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"SUCCESS"},{"name":"merge-gate","status":"COMPLETED","conclusion":"SUCCESS"}]},{"number":3,"title":"stale gate","head_ref":"stale","changed_files":["src/c.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"SUCCESS"},{"name":"merge-gate","status":"COMPLETED","conclusion":"FAILURE","text":"stale"}]},{"number":4,"title":"gate race","head_ref":"race","changed_files":["src/d.rs"],"checks":[{"name":"CI","status":"IN_PROGRESS","conclusion":""},{"name":"merge-gate","status":"COMPLETED","conclusion":"FAILURE","text":"Full CI still queued; rerun after CI completes"}]},{"number":5,"title":"real failure","head_ref":"red","changed_files":["src/e.rs"],"checks":[{"name":"unit","status":"COMPLETED","conclusion":"FAILURE","text":"assertion"},{"name":"merge-gate","status":"COMPLETED","conclusion":"SUCCESS"}]},{"number":6,"title":"missing required gate","head_ref":"missing-gate","changed_files":["src/f.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"SUCCESS"}]},{"number":7,"title":"neutral required gate","head_ref":"neutral-gate","changed_files":["src/g.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"SUCCESS"},{"name":"merge-gate","status":"COMPLETED","conclusion":"NEUTRAL"}]},{"number":8,"title":"non-gate queued words","head_ref":"real-queued","changed_files":["src/h.rs"],"checks":[{"name":"unit","status":"COMPLETED","conclusion":"FAILURE","text":"dependency still queued"},{"name":"merge-gate","status":"COMPLETED","conclusion":"SUCCESS"}]},{"number":9,"title":"draft policy change","head_ref":"policy","is_draft":true,"changed_files":["src/i.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"SUCCESS"},{"name":"merge-gate","status":"COMPLETED","conclusion":"SUCCESS"}]},{"number":10,"title":"approval required","head_ref":"review-required","review_decision":"REVIEW_REQUIRED","changed_files":["src/j.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"SUCCESS"},{"name":"merge-gate","status":"COMPLETED","conclusion":"SUCCESS"}]},{"number":11,"title":"changes requested","head_ref":"changes-requested","review_decision":"CHANGES_REQUESTED","changed_files":["src/k.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"SUCCESS"},{"name":"merge-gate","status":"COMPLETED","conclusion":"SUCCESS"}]},{"number":12,"title":"green child of broken predecessor","head_ref":"child-of-red","base_ref":"red","changed_files":["src/l.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"SUCCESS"},{"name":"merge-gate","status":"COMPLETED","conclusion":"SUCCESS"}]},{"number":13,"title":"cycle root","head_ref":"cycle-root","changed_files":["src/m.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"SUCCESS"},{"name":"merge-gate","status":"COMPLETED","conclusion":"SUCCESS"}]},{"number":14,"title":"cycle child","head_ref":"cycle-child","base_ref":"cycle-root","changed_files":["src/n.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"SUCCESS"},{"name":"merge-gate","status":"COMPLETED","conclusion":"SUCCESS"}]}],"conflicts":[{"a":1,"b":2,"paths":["src/shared.rs"]}],"ancestry":[{"before":1,"after":3},{"before":14,"after":13}]}"###;

    /// Landing context for PR 9 of the representative fixture (G3).
    const REPRESENTATIVE_CONTEXT: &str = r###"{"prs":[{"pr":9,"head_sha":"sha-9","base_sha":"basesha-main","validation_evidence":"clean-validate-record","validation_authority":"hard-green","policy_class":"gate-policy","assigned_agent":"reviewer"}]}"###;

    /// PR-394 review-evidence fixture from the differential (G7).
    const REVIEW_FIXTURE: &str = r###"{"repo":"R","base":"main","prs":[{"number":394,"head_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","review_decision":"CHANGES_REQUESTED","updated_at":"2026-09-04T12:00:00Z","review_events":[{"kind":"review","identity":"review-1","author":"reviewer","state":"CHANGES_REQUESTED","head_sha":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","created_at":"2026-09-04T12:00:00Z","updated_at":"2026-09-04T12:00:00Z","last_edited_at":"","body":"please address this","retirement_actor_permission":""},{"kind":"issue-comment","identity":"comment-1","author":"release-authority","state":"ACTIVE","head_sha":"","created_at":"2026-09-04T12:00:00Z","updated_at":"2026-09-04T12:00:00Z","last_edited_at":"","body":"[team, release-authority, session, model, role=observer]\nCHANGES-REQUESTED-WITHDRAWN-AT: codex aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa BY release-authority\nRETIRES 123456","retirement_actor_permission":"write"},{"kind":"review-comment","identity":"thread-1","author":"reviewer","state":"RESOLVED","head_sha":"","created_at":"2026-09-04T12:00:00Z","updated_at":"2026-09-04T12:00:00Z","last_edited_at":"","body":"inline objection retired","retirement_actor_permission":""}],"checks":[{"name":"merge-gate","status":"COMPLETED","conclusion":"SUCCESS"}]}]}"###;

    /// Digest the retired differential's independent oracle (`review_digest()`)
    /// computed for [`REVIEW_FIXTURE`]; frozen at retirement (G7).
    const REVIEW_DIGEST_GOLDEN: &str =
        "5e2f93d98b74e4600056209af299019a49458437dbc3bcc06ca078381fda2f71";

    #[test]
    fn help_schema_matches_differential_contract() {
        // G2: replaces schema:{py,rs}/commands + schema:{py,rs}/plan.
        let (code, top, _) = invoke(&["--help"]);
        assert_eq!(code, 0);
        for token in ["plan", "graph", "clusters", "status", "quickstart"] {
            assert!(top.contains(token), "top help missing {token}");
        }
        let (code, plan_help, _) = invoke(&["plan", "--help"]);
        assert_eq!(code, 0);
        for flag in [
            "--repo",
            "--base",
            "--fixture",
            "--format",
            "--git-dir",
            "--prs",
            "--conflict-detector",
            "--gate-check",
            "--landing-context",
            "--flaky-signatures",
            "--outage-min-prs",
            "--freshness-max-behind",
            "--priority-source",
            "--batch",
        ] {
            assert!(plan_help.contains(flag), "plan help missing {flag}");
        }
        assert!(plan_help.contains("command"));
        assert!(!plan_help.to_lowercase().contains("beads"));
    }

    #[test]
    fn representative_cli_goldens_and_safety_oracle() {
        // G3: replaces representative:{plan,graph,clusters,status}/* exact-byte
        // cases plus the representative:safety-* semantic oracle (case 8).
        let fixture = temp_file("representative.json", REPRESENTATIVE_FIXTURE);
        let context = temp_file("representative-context.json", REPRESENTATIVE_CONTEXT);
        let fixture_arg = fixture.to_string_lossy().into_owned();
        let context_arg = context.to_string_lossy().into_owned();
        let exact: [(&str, &str, &str); 7] = [
            (
                "plan",
                "human",
                r###"Repository: OWNER/NAME  base: main
14 open PR(s), 1 real conflict(s), 0 file-overlap risk(s), 4 ordering edge(s), 0 mechanism overlap(s)

CI health:
  #1     ci=passed behind=0 size=4  green
  #2     ci=passed behind=3 size=0  behind
  #3     ci=failed [stale-required-check] behind=0 size=0  stale gate
  #4     ci=failed [evaluate-once-race] behind=0 size=0  gate race
  #5     ci=failed [real] behind=0 size=0  real failure
  #6     ci=passed behind=0 size=0  missing required gate
  #7     ci=no-result behind=0 size=0  neutral required gate
  #8     ci=failed [real] behind=0 size=0  non-gate queued words
  #9     ci=passed behind=0 size=0 HELD  draft policy change
  #10    ci=passed behind=0 size=0 HELD  approval required
  #11    ci=passed behind=0 size=0 HELD  changes requested
  #12    ci=passed behind=0 size=0  green child of broken predecessor
  #13    ci=passed behind=0 size=0 HELD  cycle root
  #14    ci=passed behind=0 size=0 HELD  cycle child

Parallel-safe groups (each group lands in any order):
  group 0: #2, #4, #5, #6, #7, #8
  group 1: #12, #1
  group 2: #3

Land now: #1, #2
Recommended order: #2, #4, #5, #6, #7, #8, #12, #1, #3
Batch (green-only, conflict-free): #2

Per-PR actions:
  #1     land-now               authoritative CI green, gate ok  (green)
  #2     land-now               authoritative CI green, gate ok  (behind)
  #3     refire-stale-gate      CI green on head; required gate 'merge-gate' is stale  (stale gate)
  #4     wait                   gate evaluated once while full CI was still queued (benign; treat as pending)  (gate race)
  #5     hold-fix               real red on: unit  (real failure)
  #6     refire-ci              required gate has NO_RESULT (absent); re-dispatch  (missing required gate)
  #7     refire-ci              required gate has NO_RESULT; re-dispatch  (neutral required gate)
  #8     hold-fix               real red on: unit  (non-gate queued words)
  #9     escalate-gate-policy   gate-policy change requires coordinator decision; validation evidence is not approval  (draft policy change)
  #10    wait                   held: review-required  (approval required)
  #11    wait                   held: changes-requested  (changes requested)
  #12    wait                   dependency #5 requires hold-fix; wait and rerun  (green child of broken predecessor)
  #13    wait                   held: ordering-cycle (resolve dependency cycle and rerun)  (cycle root)
  #14    wait                   held: ordering-cycle (resolve dependency cycle and rerun)  (cycle child)

Mechanism overlaps (same mechanism — review together, may be opposite intent):
  (none)

Diagnostics:
  stale required-check (refire gate): #3
  real reds (hold + fix): #5, #8
  evaluate-once race (benign): #4

Held PRs:
  #9: draft
  #10: review-required
  #11: changes-requested
  #13: ordering-cycle
  #14: ordering-cycle

Advisory only: this plan recommends; it never arms or merges anything.
"###,
            ),
            (
                "plan",
                "actions",
                r###"open_prs=14
land_now=2
rebase=0
refire_stale_gate=1
refire_ci=2
hold_fix=2
escalate_outage=0
escalate_gate_policy=1
wait=6
held=5
stale_gates=1
flaky_reds=0
real_reds=2
evaluate_once_race=1
mechanism_overlaps=0
rebases_avoided=1
validate_runs_avoided=1
unclassified_mechanisms=0
outage=0
NOTE: evaluate-once-race pr=4 (benign gate noise; treat as pending)
ACTION: land-now pr=2 group=0 size=0 why="authoritative CI green, gate ok"
NOTE: wait pr=4 group=0 why="gate evaluated once while full CI was still queued (benign; treat as pending)"
ACTION: hold-fix pr=5 group=0 why="real red on: unit"
ACTION: refire-ci pr=6 group=0 why="required gate has NO_RESULT (absent); re-dispatch"
ACTION: refire-ci pr=7 group=0 why="required gate has NO_RESULT; re-dispatch"
ACTION: hold-fix pr=8 group=0 why="real red on: unit"
NOTE: wait pr=12 group=1 why="dependency #5 requires hold-fix; wait and rerun"
ACTION: land-now pr=1 group=1 size=4 why="authoritative CI green, gate ok"
ACTION: refire-stale-gate pr=3 group=2 why="CI green on head; required gate 'merge-gate' is stale"
ERROR: escalate-gate-policy pr=9 why="gate-policy change requires coordinator decision; validation evidence is not approval"
NOTE: wait pr=10 why="held: review-required"
NOTE: wait pr=11 why="held: changes-requested"
NOTE: wait pr=13 why="held: ordering-cycle (resolve dependency cycle and rerun)"
NOTE: wait pr=14 why="held: ordering-cycle (resolve dependency cycle and rerun)"
"###,
            ),
            (
                "graph",
                "human",
                r###"Repository: OWNER/NAME  base: main
14 open PR(s), 1 real conflict(s), 0 file-overlap risk(s), 4 ordering edge(s), 0 mechanism overlap(s)

Stacks:
  #1 -> #3
  #5 -> #12

Real conflicts (git merge-tree):
  #1 <-> #2: src/shared.rs

File-overlap risks (auto-mergeable but shared files):
  (none)

Mechanism overlaps (same mechanism — review together, may be opposite intent):
  (none)

Unclassified mechanism candidates (recognise, then extend the enum):
  (none)

Held PRs:
  #9: draft
  #10: review-required
  #11: changes-requested
  #13: ordering-cycle
  #14: ordering-cycle
"###,
            ),
            (
                "clusters",
                "human",
                r###"Repository: OWNER/NAME  base: main
14 open PR(s), 1 real conflict(s) => 13 cluster(s): 1 multi-PR stack(s), 12 independent singleton(s)

Conflict clusters land each as ONE stack (base -> tip):
  stack 0: #2 -> #1  (2 PRs, 1 rebases avoided)
      shared conflict set: src/shared.rs

Parallel lanes (clusters share no conflict => land concurrently):
  #2(+1), #3, #4, #5, #6, #7, #8, #9, #10, #11, #12, #13, #14

Metric:
  rebases avoided by stacking = 1 (serial landing of these clusters would cost 1 extra rebase(s))
  validate runs avoided = 1 (the validate record is SHA-keyed, so each avoided rebase is an avoided validate run)
  A branch behind its fetched base must rebase before landing. The consuming workspace may authorize that rebase to retain soft-green without pre-landing revalidation; post-facto validation remains due. Landing each real-conflict cluster as ONE stack avoids the same count of repeated rebases and post-facto validate runs.

Advisory only: this plan recommends; it never arms or merges anything.
"###,
            ),
            (
                "clusters",
                "json",
                r###"{
  "base": "main",
  "clusters": [
    {
      "conflict_paths": [
        "src/shared.rs"
      ],
      "members": [
        2,
        1
      ],
      "rebases_avoided": 1,
      "size": 2
    },
    {
      "conflict_paths": [],
      "members": [
        3
      ],
      "rebases_avoided": 0,
      "size": 1
    },
    {
      "conflict_paths": [],
      "members": [
        4
      ],
      "rebases_avoided": 0,
      "size": 1
    },
    {
      "conflict_paths": [],
      "members": [
        5
      ],
      "rebases_avoided": 0,
      "size": 1
    },
    {
      "conflict_paths": [],
      "members": [
        6
      ],
      "rebases_avoided": 0,
      "size": 1
    },
    {
      "conflict_paths": [],
      "members": [
        7
      ],
      "rebases_avoided": 0,
      "size": 1
    },
    {
      "conflict_paths": [],
      "members": [
        8
      ],
      "rebases_avoided": 0,
      "size": 1
    },
    {
      "conflict_paths": [],
      "members": [
        9
      ],
      "rebases_avoided": 0,
      "size": 1
    },
    {
      "conflict_paths": [],
      "members": [
        10
      ],
      "rebases_avoided": 0,
      "size": 1
    },
    {
      "conflict_paths": [],
      "members": [
        11
      ],
      "rebases_avoided": 0,
      "size": 1
    },
    {
      "conflict_paths": [],
      "members": [
        12
      ],
      "rebases_avoided": 0,
      "size": 1
    },
    {
      "conflict_paths": [],
      "members": [
        13
      ],
      "rebases_avoided": 0,
      "size": 1
    },
    {
      "conflict_paths": [],
      "members": [
        14
      ],
      "rebases_avoided": 0,
      "size": 1
    }
  ],
  "repository": "OWNER/NAME",
  "summary": {
    "clusters": 13,
    "largest_cluster": 2,
    "multi_pr_clusters": 1,
    "open_prs": 14,
    "parallel_lanes": 13,
    "rebases_avoided": 1,
    "singletons": 12,
    "validate_runs_avoided": 1
  }
}
"###,
            ),
            (
                "status",
                "human",
                r###"Open PR health: OWNER/NAME

  #1     ci=passed  green
  #2     ci=passed  behind
  #3     ci=failed [stale-required-check]  stale gate
  #4     ci=failed [evaluate-once-race]  gate race
  #5     ci=failed [real]  real failure
  #6     ci=passed  missing required gate
  #7     ci=no-result  neutral required gate
  #8     ci=failed [real]  non-gate queued words
  #9     ci=passed draft  draft policy change
  #10    ci=passed  approval required
  #11    ci=passed  changes requested
  #12    ci=passed  green child of broken predecessor
  #13    ci=passed  cycle root
  #14    ci=passed  cycle child

Summary
  open:      14
  ci-red:    4
  real reds: 2
  WARNING: 14 open PRs exceeds the 8 threshold; prioritize landing/CI repair.
"###,
            ),
            (
                "status",
                "json",
                r###"{
  "base": "main",
  "prs": [
    {
      "ci": "passed",
      "draft": false,
      "labels": [],
      "pr": 1,
      "red_class": null,
      "title": "green"
    },
    {
      "ci": "passed",
      "draft": false,
      "labels": [],
      "pr": 2,
      "red_class": null,
      "title": "behind"
    },
    {
      "ci": "failed",
      "draft": false,
      "labels": [],
      "pr": 3,
      "red_class": "stale-required-check",
      "title": "stale gate"
    },
    {
      "ci": "failed",
      "draft": false,
      "labels": [],
      "pr": 4,
      "red_class": "evaluate-once-race",
      "title": "gate race"
    },
    {
      "ci": "failed",
      "draft": false,
      "labels": [],
      "pr": 5,
      "red_class": "real",
      "title": "real failure"
    },
    {
      "ci": "passed",
      "draft": false,
      "labels": [],
      "pr": 6,
      "red_class": null,
      "title": "missing required gate"
    },
    {
      "ci": "no-result",
      "draft": false,
      "labels": [],
      "pr": 7,
      "red_class": null,
      "title": "neutral required gate"
    },
    {
      "ci": "failed",
      "draft": false,
      "labels": [],
      "pr": 8,
      "red_class": "real",
      "title": "non-gate queued words"
    },
    {
      "ci": "passed",
      "draft": true,
      "labels": [],
      "pr": 9,
      "red_class": null,
      "title": "draft policy change"
    },
    {
      "ci": "passed",
      "draft": false,
      "labels": [],
      "pr": 10,
      "red_class": null,
      "title": "approval required"
    },
    {
      "ci": "passed",
      "draft": false,
      "labels": [],
      "pr": 11,
      "red_class": null,
      "title": "changes requested"
    },
    {
      "ci": "passed",
      "draft": false,
      "labels": [],
      "pr": 12,
      "red_class": null,
      "title": "green child of broken predecessor"
    },
    {
      "ci": "passed",
      "draft": false,
      "labels": [],
      "pr": 13,
      "red_class": null,
      "title": "cycle root"
    },
    {
      "ci": "passed",
      "draft": false,
      "labels": [],
      "pr": 14,
      "red_class": null,
      "title": "cycle child"
    }
  ],
  "repository": "OWNER/NAME",
  "summary": {
    "failed": 4,
    "no_result": 1,
    "open": 14,
    "outage_suspected": false,
    "passed": 9,
    "real_reds": 2
  }
}
"###,
            ),
        ];
        for (command, format, golden) in exact {
            let mut args = vec![
                command,
                "--fixture",
                fixture_arg.as_str(),
                "--landing-context",
                context_arg.as_str(),
                "--format",
                format,
            ];
            if command == "plan" {
                args.push("--batch");
            }
            let (code, stdout, stderr) = invoke(&args);
            assert_eq!(code, 0, "{command}/{format}: {stderr}");
            assert_eq!(stdout, golden, "{command}/{format} golden drift");
        }
        // The two large JSON renderings are pinned by digest plus the semantic
        // oracle below, rather than by multi-kilobyte literals.
        let (code, plan_json, stderr) = invoke(&[
            "plan",
            "--fixture",
            fixture_arg.as_str(),
            "--landing-context",
            context_arg.as_str(),
            "--format",
            "json",
            "--batch",
        ]);
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(
            sha256_hex(&plan_json),
            "c108c96ca44ddc989c1999f49d6e296277f094d53228269896b78c6ca46f8cdb",
            "plan/json bytes drift"
        );
        let (code, graph_json, stderr) = invoke(&[
            "graph",
            "--fixture",
            fixture_arg.as_str(),
            "--landing-context",
            context_arg.as_str(),
            "--format",
            "json",
        ]);
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(
            sha256_hex(&graph_json),
            "0c3f96d629e55e41c0b00d1d928c07ed6a1e2e93531e744445ab22a9cf27f25d",
            "graph/json bytes drift"
        );
        // Safety oracle under an explicit freshness bound: exact action table.
        let (code, safety_json, stderr) = invoke(&[
            "plan",
            "--fixture",
            fixture_arg.as_str(),
            "--landing-context",
            context_arg.as_str(),
            "--format",
            "json",
            "--batch",
            "--freshness-max-behind",
            "0",
        ]);
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(
            sha256_hex(&safety_json),
            "5da5c458b1471acd500e9d62ad32b713b4f9f57c0ff066bcd86ab8e9fc6ffa3a",
            "safety json bytes drift"
        );
        let parsed: Value = serde_json::from_str(&safety_json).unwrap();
        let decisions = parsed["plan"]["per_pr_actions"].as_array().unwrap();
        let actions: std::collections::BTreeMap<i64, &str> = decisions
            .iter()
            .map(|decision| {
                (
                    decision["pr"].as_i64().unwrap(),
                    decision["action"].as_str().unwrap(),
                )
            })
            .collect();
        let expected: [(i64, &str); 14] = [
            (1, "land-now"),
            (2, "rebase-then-land"),
            (3, "refire-stale-gate"),
            (4, "wait"),
            (5, "hold-fix"),
            (6, "refire-ci"),
            (7, "refire-ci"),
            (8, "hold-fix"),
            (9, "escalate-gate-policy"),
            (10, "wait"),
            (11, "wait"),
            (12, "wait"),
            (13, "wait"),
            (14, "wait"),
        ];
        for (number, action) in expected {
            assert_eq!(actions.get(&number), Some(&action), "PR {number}");
        }
        let nodes = parsed["nodes"].as_array().unwrap();
        assert!(nodes.iter().all(|node| {
            node.get("base_sha").is_some() && node.get("review_decision").is_some()
        }));
        let held: std::collections::BTreeMap<i64, Vec<String>> = parsed["held_prs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| {
                (
                    item["pr"].as_i64().unwrap(),
                    item["reasons"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|reason| reason.as_str().unwrap().to_owned())
                        .collect(),
                )
            })
            .collect();
        assert!(held[&10].contains(&"review-required".to_owned()));
        assert!(held[&11].contains(&"changes-requested".to_owned()));
        assert!(held[&13].contains(&"ordering-cycle".to_owned()));
        assert!(held[&14].contains(&"ordering-cycle".to_owned()));
        let batch = parsed["plan"]["batch"].as_array().unwrap();
        assert!(!batch
            .iter()
            .any(|value| [12, 13, 14].contains(&value.as_i64().unwrap())));
        // Production default (no freshness bound): PR 2 lands behind.
        let default_parsed: Value = serde_json::from_str(&plan_json).unwrap();
        let default_pr2 = default_parsed["plan"]["per_pr_actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|decision| decision["pr"] == 2)
            .unwrap();
        assert_eq!(default_pr2["action"], "land-now");
        let _ = fs::remove_file(fixture);
        let _ = fs::remove_file(context);
    }

    #[test]
    fn priority_cli_wiring_matches_differential() {
        // G4: replaces representative:command-priority + reject:priority/*.
        let fixture = temp_file("priority.json", REPRESENTATIVE_FIXTURE);
        let context = temp_file("priority-context.json", REPRESENTATIVE_CONTEXT);
        let fixture_arg = fixture.to_string_lossy().into_owned();
        let context_arg = context.to_string_lossy().into_owned();
        let (code, stdout, stderr) = invoke(&[
            "plan",
            "--fixture",
            fixture_arg.as_str(),
            "--landing-context",
            context_arg.as_str(),
            "--format",
            "json",
            "--priority-source",
            "command",
            "--priority-cmd",
            "printf '%s' '{pr}'",
        ]);
        assert_eq!(code, 0, "{stderr}");
        assert_eq!(stderr, "");
        assert_eq!(
            sha256_hex(&stdout),
            "d27eb7ba5d54d4f2cd2a94fcfa9feff8cb71d4cdf39224600b76dc52b5ff6213",
            "command-priority bytes drift"
        );
        let rejects: [(&str, &[&str]); 5] = [
            ("missing-command", &["--priority-source", "command"]),
            (
                "missing-label-capture",
                &[
                    "--priority-source",
                    "labels",
                    "--priority-label-pattern",
                    "^p[0-9]+$",
                ],
            ),
            (
                "overflow-command-output",
                &[
                    "--priority-source",
                    "command",
                    "--priority-cmd",
                    "printf 9223372036854775808",
                ],
            ),
            (
                "failed-command",
                &["--priority-source", "command", "--priority-cmd", "exit 9"],
            ),
            (
                "hook-launch-failure",
                &[
                    "--priority-source",
                    "command",
                    "--priority-cmd",
                    "printf 1",
                    "--net-wrapper",
                    "/definitely/missing-priority-wrapper",
                ],
            ),
        ];
        for (label, extra) in rejects {
            let mut args = vec![
                "plan",
                "--fixture",
                fixture_arg.as_str(),
                "--landing-context",
                context_arg.as_str(),
            ];
            args.extend_from_slice(extra);
            let (code, _, stderr) = invoke(&args);
            assert_eq!(code, 2, "reject:priority/{label} must exit 2: {stderr}");
        }
        let _ = fs::remove_file(fixture);
        let _ = fs::remove_file(context);
    }

    #[test]
    #[cfg(unix)]
    fn live_list_refusals_exit_two_via_cli() {
        // G5: replaces reject:live-list-* (fake gh, exit 2 + stderr cause).
        use std::os::unix::fs::PermissionsExt;
        let limit_payload = {
            let entries: Vec<String> = (0..500)
                .map(|number| format!("{{\"number\": {number}}}"))
                .collect();
            format!("[{}]", entries.join(","))
        };
        for (label, payload, message) in [
            ("empty", String::new(), "empty stdout"),
            (
                "non-object-entry",
                "[{\"number\": 1}, \"not-an-object\"]".to_owned(),
                "entry 1 is not an object",
            ),
            ("limit-reached", limit_payload, "500-item limit"),
        ] {
            let script = temp_file(
                &format!("fake-gh-{label}"),
                &format!(
                    "#!/bin/sh\nprintf %s '{}'\n",
                    payload.replace('\'', "'\\''")
                ),
            );
            let mut permissions = fs::metadata(&script).unwrap().permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&script, permissions).unwrap();
            let script_arg = script.to_string_lossy().into_owned();
            let git_dir = std::env::temp_dir().to_string_lossy().into_owned();
            let (code, _, stderr) = invoke(&[
                "plan",
                "--repo",
                "OWNER/NAME",
                "--git-dir",
                git_dir.as_str(),
                "--gh-cmd",
                script_arg.as_str(),
                "--no-archive",
            ]);
            assert_eq!(code, 2, "reject:live-list-{label} must exit 2: {stderr}");
            assert!(
                stderr.contains(message),
                "reject:live-list-{label} stderr must name {message:?}: {stderr}"
            );
            let _ = fs::remove_file(script);
        }
    }

    #[test]
    fn document_and_domain_rejects_exit_two_via_cli() {
        // G6: replaces reject:<16 fixture docs> + reject:<5 contexts> +
        // reject:cli-domain/* + live:repo-required + cli:abbreviation-refused +
        // fixture:unknown-detector. Reasons stay covered in fixture/collect/context.
        let fixture = temp_file("representative.json", REPRESENTATIVE_FIXTURE);
        let fixture_arg = fixture.to_string_lossy().into_owned();
        let invalid_fixtures: [(&str, &str); 16] = [
            ("root-array.json", "[]"),
            ("missing-prs.json", "{\"repo\":\"R\",\"base\":\"main\"}"),
            (
                "prs-not-list.json",
                "{\"repo\":\"R\",\"base\":\"main\",\"prs\":{}}",
            ),
            (
                "number-not-int.json",
                "{\"repo\":\"R\",\"base\":\"main\",\"prs\":[{\"number\":\"one\"}]}",
            ),
            ("malformed.json", "{"),
            ("malformed.yaml", "prs:\n  - number: [\n"),
            ("duplicate-key.json", "{\"prs\":[],\"prs\":[]}"),
            ("duplicate-key.yaml", "prs: []\nprs: []\n"),
            ("merge-key.yaml", "base: &b {repo: R}\n<<: *b\nprs: []\n"),
            (
                "duplicate-pr.json",
                "{\"prs\":[{\"number\":1},{\"number\":1}]}",
            ),
            ("zero-pr.json", "{\"prs\":[{\"number\":0}]}"),
            (
                "overflow-pr.json",
                "{\"prs\":[{\"number\":9223372036854775808}]}",
            ),
            (
                "negative-behind.json",
                "{\"prs\":[{\"number\":1,\"commits_behind\":-1}]}",
            ),
            (
                "unknown-conflict.json",
                "{\"prs\":[{\"number\":1}],\"conflicts\":[{\"a\":1,\"b\":2}]}",
            ),
            (
                "self-ancestry.json",
                "{\"prs\":[{\"number\":1}],\"ancestry\":[{\"before\":1,\"after\":1}]}",
            ),
            (
                "relations-shape.json",
                "{\"prs\":[{\"number\":1}],\"conflicts\":{}}",
            ),
        ];
        for (name, body) in invalid_fixtures {
            let path = temp_file(name, body);
            let path_arg = path.to_string_lossy().into_owned();
            let (code, _, stderr) = invoke(&["plan", "--fixture", path_arg.as_str()]);
            assert_eq!(code, 2, "reject:{name} must exit 2: {stderr}");
            let _ = fs::remove_file(path);
        }
        let bad_contexts: [(&str, &str); 5] = [
            (
                "head-only-context.json",
                "{\"prs\":[{\"pr\":1,\"head_sha\":\"sha-1\",\"validation_evidence\":\"clean-validate-record\"}]}",
            ),
            (
                "exact-base-no-authority-context.json",
                "{\"prs\":[{\"pr\":1,\"head_sha\":\"sha-1\",\"base_sha\":\"basesha-main\",\"validation_evidence\":\"clean-validate-record\"}]}",
            ),
            (
                "stale-base-context.json",
                "{\"prs\":[{\"pr\":1,\"head_sha\":\"sha-1\",\"base_sha\":\"old-base\",\"validation_evidence\":\"clean-validate-record\"}]}",
            ),
            (
                "hard-green-other-base-context.json",
                "{\"prs\":[{\"pr\":1,\"head_sha\":\"sha-1\",\"base_sha\":\"divergent-base\",\"validation_evidence\":\"clean-validate-record\",\"validation_authority\":\"hard-green\"}]}",
            ),
            (
                "authority-without-record-context.json",
                "{\"prs\":[{\"pr\":1,\"head_sha\":\"sha-1\",\"base_sha\":\"old-base\",\"validation_authority\":\"soft-green\"}]}",
            ),
        ];
        for (name, body) in bad_contexts {
            let path = temp_file(name, body);
            let path_arg = path.to_string_lossy().into_owned();
            let (code, _, stderr) = invoke(&[
                "plan",
                "--fixture",
                fixture_arg.as_str(),
                "--landing-context",
                path_arg.as_str(),
            ]);
            assert_eq!(code, 2, "reject:{name} must exit 2: {stderr}");
            let _ = fs::remove_file(path);
        }
        for (flag, value) in [
            ("--outage-min-prs", "-1"),
            ("--freshness-max-behind", "-1"),
            ("--freshness-max-behind", "9223372036854775808"),
            ("--prs", "0"),
            ("--prs", "9223372036854775808"),
        ] {
            let (code, _, stderr) =
                invoke(&["plan", "--fixture", fixture_arg.as_str(), flag, value]);
            assert_eq!(code, 2, "reject:cli-domain/{flag}={value}: {stderr}");
        }
        let (code, _, _) = invoke(&["plan"]);
        assert_eq!(code, 2, "live:repo-required must exit 2");
        let (code, _, _) = invoke(&["plan", "--fixt", fixture_arg.as_str()]);
        assert_eq!(code, 2, "cli:abbreviation-refused must exit 2");
        let (code, _, _) = invoke(&[
            "plan",
            "--fixture",
            fixture_arg.as_str(),
            "--conflict-detector",
            "unknown",
        ]);
        assert_eq!(code, 2, "fixture:unknown-detector must exit 2");
        let _ = fs::remove_file(fixture);
    }

    #[test]
    fn review_evidence_dispositions_match_differential() {
        // G7 (CLI half): replaces the case-12 disposition rows. The digest
        // golden itself is pinned in context::tests; semantics for the
        // same-timestamp / BY-claim / Unicode matrix stay in context.rs.
        let fixture = temp_file("review.json", REVIEW_FIXTURE);
        let fixture_arg = fixture.to_string_lossy().into_owned();
        // Probe only: emitted digest is the frozen oracle value; PR stays held.
        let (code, probe, stderr) = invoke(&[
            "plan",
            "--fixture",
            fixture_arg.as_str(),
            "--format",
            "json",
        ]);
        assert_eq!(code, 0, "{stderr}");
        let parsed: Value = serde_json::from_str(&probe).unwrap();
        assert_eq!(
            parsed["nodes"][0]["review_evidence_digest"], REVIEW_DIGEST_GOLDEN,
            "emitted digest must equal the frozen Python-oracle golden"
        );
        assert_eq!(parsed["nodes"][0]["held"], true);
        // Exact context: unheld and land-now.
        let head = "a".repeat(40);
        let context_body = serde_json::json!({"prs": [{
            "pr": 394,
            "head_sha": head,
            "review_objections_resolved": true,
            "review_evidence_digest": REVIEW_DIGEST_GOLDEN,
        }]})
        .to_string();
        let context = temp_file("review-context.json", &context_body);
        let context_arg = context.to_string_lossy().into_owned();
        let (code, accepted, stderr) = invoke(&[
            "plan",
            "--fixture",
            fixture_arg.as_str(),
            "--landing-context",
            context_arg.as_str(),
            "--format",
            "json",
        ]);
        assert_eq!(code, 0, "{stderr}");
        let parsed: Value = serde_json::from_str(&accepted).unwrap();
        assert_eq!(parsed["nodes"][0]["held"], false);
        assert_eq!(parsed["plan"]["per_pr_actions"][0]["action"], "land-now");
        // Wrong digest: context invalidated, exit 2.
        let wrong_body = serde_json::json!({"prs": [{
            "pr": 394,
            "head_sha": head,
            "review_objections_resolved": true,
            "review_evidence_digest": "0".repeat(64),
        }]})
        .to_string();
        let wrong = temp_file("review-wrong-context.json", &wrong_body);
        let wrong_arg = wrong.to_string_lossy().into_owned();
        let (code, _, stderr) = invoke(&[
            "plan",
            "--fixture",
            fixture_arg.as_str(),
            "--landing-context",
            wrong_arg.as_str(),
            "--format",
            "json",
        ]);
        assert_eq!(code, 2, "wrong review digest must exit 2: {stderr}");
        // Unavailable evidence stays visible and held even with clean validation.
        let unavailable = temp_file(
            "review-unavailable.json",
            "{\"repo\":\"OWNER/NAME\",\"base\":\"main\",\"prs\":[{\"number\":1,\"title\":\"review evidence unavailable\",\"head_ref\":\"green\",\"head_sha\":\"sha-1\",\"review_decision\":\"APPROVED\",\"review_evidence_unavailable\":true,\"changed_files\":[\"src/a.rs\"],\"checks\":[{\"name\":\"merge-gate\",\"status\":\"COMPLETED\",\"conclusion\":\"SUCCESS\"}]}]}",
        );
        let unavailable_arg = unavailable.to_string_lossy().into_owned();
        let authority = temp_file(
            "review-authority-context.json",
            "{\"prs\":[{\"pr\":1,\"head_sha\":\"sha-1\",\"base_sha\":\"basesha-main\",\"validation_evidence\":\"clean-validate-record\",\"validation_authority\":\"hard-green\"}]}",
        );
        let authority_arg = authority.to_string_lossy().into_owned();
        let (code, held_out, stderr) = invoke(&[
            "plan",
            "--fixture",
            unavailable_arg.as_str(),
            "--landing-context",
            authority_arg.as_str(),
            "--format",
            "json",
        ]);
        assert_eq!(code, 0, "{stderr}");
        let parsed: Value = serde_json::from_str(&held_out).unwrap();
        assert_eq!(parsed["nodes"][0]["review_evidence_unavailable"], true);
        assert_eq!(parsed["nodes"][0]["held"], true);
        assert_eq!(parsed["plan"]["per_pr_actions"][0]["action"], "wait");
        let _ = fs::remove_file(fixture);
        let _ = fs::remove_file(context);
        let _ = fs::remove_file(wrong);
        let _ = fs::remove_file(unavailable);
        let _ = fs::remove_file(authority);
    }

    #[test]
    fn frozen_random_corpus_goldens() {
        // G8: replaces random:{i}/plan-json for seed 1234, indices 0-3.
        // Configs were frozen from the seeded generator at retirement; each
        // stdout digest pins the exact Rust bytes (no RNG in this suite).
        let cases: [(&str, &[&str], &str); 4] = [
            (
                r###"{"repo":"R","base":"main","prs":[{"number":1,"head_ref":"feature-1","base_ref":"main","title":"random 0/1","is_draft":true,"review_decision":"REVIEW_REQUIRED","additions":4,"deletions":85,"commits_behind":0,"labels":["priority:0"],"changed_files":["src/2.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"SUCCESS","text":""},{"name":"merge-gate","status":"COMPLETED","conclusion":"SUCCESS","text":""}]},{"number":2,"head_ref":"feature-2","base_ref":"feature-1","title":"random 0/2","is_draft":false,"review_decision":"APPROVED","additions":82,"deletions":79,"commits_behind":3,"labels":["priority:3"],"changed_files":["src/1.rs"],"checks":[{"name":"CI","status":"IN_PROGRESS","conclusion":"","text":""},{"name":"merge-gate","status":"COMPLETED","conclusion":"SUCCESS","text":""}]},{"number":3,"head_ref":"feature-3","base_ref":"main","title":"random 0/3","is_draft":false,"review_decision":"REVIEW_REQUIRED","additions":69,"deletions":59,"commits_behind":0,"labels":["priority:0"],"changed_files":["src/4.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"SUCCESS","text":""},{"name":"merge-gate","status":"COMPLETED","conclusion":"CANCELLED","text":""}]},{"number":4,"head_ref":"feature-4","base_ref":"main","title":"random 0/4","is_draft":false,"review_decision":"APPROVED","additions":34,"deletions":31,"commits_behind":1,"labels":["priority:3"],"changed_files":["src/4.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"FAILURE","text":"assertion"},{"name":"merge-gate","status":"COMPLETED","conclusion":"SUCCESS","text":""}]},{"number":5,"head_ref":"feature-5","base_ref":"main","title":"random 0/5","is_draft":false,"review_decision":"REVIEW_REQUIRED","additions":48,"deletions":21,"commits_behind":0,"labels":["priority:0"],"changed_files":["src/4.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"SUCCESS","text":""},{"name":"merge-gate","status":"COMPLETED","conclusion":"SUCCESS","text":""}]},{"number":6,"head_ref":"feature-6","base_ref":"feature-5","title":"random 0/6","is_draft":false,"review_decision":"","additions":67,"deletions":18,"commits_behind":2,"labels":["priority:3"],"changed_files":["src/0.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"SUCCESS","text":""},{"name":"merge-gate","status":"IN_PROGRESS","conclusion":"","text":""}]},{"number":7,"head_ref":"feature-7","base_ref":"main","title":"random 0/7","is_draft":false,"review_decision":"","additions":99,"deletions":59,"commits_behind":0,"labels":["priority:3"],"changed_files":["src/2.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"SUCCESS","text":""},{"name":"merge-gate","status":"IN_PROGRESS","conclusion":"","text":""}]},{"number":8,"head_ref":"feature-8","base_ref":"feature-7","title":"random 0/8","is_draft":false,"review_decision":"REVIEW_REQUIRED","additions":21,"deletions":56,"commits_behind":4,"labels":["priority:3"],"changed_files":["src/2.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"CANCELLED","text":""},{"name":"merge-gate","status":"COMPLETED","conclusion":"CANCELLED","text":""}]}],"conflicts":[{"a":1,"b":2,"paths":["src/shared.rs"]}]}"###,
                &[
                    "--batch",
                    "--freshness-max-behind",
                    "1",
                    "--conflict-detector",
                    "file-overlap",
                ],
                "02dd8e1bde4af637ecf8d65527ac9da761a3dd7722e3e47646903bda598452f6",
            ),
            (
                r###"{"repo":"R","base":"main","prs":[{"number":1,"head_ref":"feature-1","base_ref":"main","title":"random 1/1","is_draft":false,"review_decision":"APPROVED","additions":48,"deletions":84,"commits_behind":2,"labels":["priority:1"],"changed_files":["src/2.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"SUCCESS","text":""},{"name":"merge-gate","status":"COMPLETED","conclusion":"SUCCESS","text":""}]},{"number":2,"head_ref":"feature-2","base_ref":"main","title":"random 1/2","is_draft":false,"review_decision":"","additions":92,"deletions":41,"commits_behind":3,"labels":["priority:0"],"changed_files":["src/0.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"SUCCESS","text":""},{"name":"merge-gate","status":"IN_PROGRESS","conclusion":"","text":""}]},{"number":3,"head_ref":"feature-3","base_ref":"main","title":"random 1/3","is_draft":false,"review_decision":"","additions":89,"deletions":40,"commits_behind":2,"labels":["priority:3"],"changed_files":["src/0.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"SUCCESS","text":""},{"name":"merge-gate","status":"COMPLETED","conclusion":"FAILURE","text":"stale"}]},{"number":4,"head_ref":"feature-4","base_ref":"main","title":"random 1/4","is_draft":false,"review_decision":"","additions":90,"deletions":58,"commits_behind":4,"labels":["priority:1"],"changed_files":["src/2.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"SUCCESS","text":""},{"name":"merge-gate","status":"COMPLETED","conclusion":"CANCELLED","text":""}]},{"number":5,"head_ref":"feature-5","base_ref":"feature-4","title":"random 1/5","is_draft":false,"review_decision":"APPROVED","additions":7,"deletions":45,"commits_behind":4,"labels":["priority:1"],"changed_files":["src/3.rs"],"checks":[{"name":"CI","status":"IN_PROGRESS","conclusion":"","text":""},{"name":"merge-gate","status":"COMPLETED","conclusion":"CANCELLED","text":""}]},{"number":6,"head_ref":"feature-6","base_ref":"feature-5","title":"random 1/6","is_draft":true,"review_decision":"","additions":18,"deletions":69,"commits_behind":0,"labels":["priority:2"],"changed_files":["src/1.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"CANCELLED","text":""},{"name":"merge-gate","status":"COMPLETED","conclusion":"FAILURE","text":"stale"}]},{"number":7,"head_ref":"feature-7","base_ref":"main","title":"random 1/7","is_draft":true,"review_decision":"APPROVED","additions":94,"deletions":41,"commits_behind":1,"labels":["priority:3"],"changed_files":["src/2.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"SUCCESS","text":""},{"name":"merge-gate","status":"IN_PROGRESS","conclusion":"","text":""}]}],"conflicts":[{"a":1,"b":6,"paths":["src/shared.rs"]}]}"###,
                &[
                    "--batch",
                    "--freshness-max-behind",
                    "2",
                    "--conflict-detector",
                    "file-overlap",
                ],
                "f99e8ff86752f914ffd31ead4b97d9fbece8f1b6b5a71454a4ede939ca312195",
            ),
            (
                r###"{"repo":"R","base":"main","prs":[{"number":1,"head_ref":"feature-1","base_ref":"main","title":"random 2/1","is_draft":false,"review_decision":"APPROVED","additions":52,"deletions":51,"commits_behind":4,"labels":["priority:2"],"changed_files":["src/4.rs"],"checks":[{"name":"CI","status":"IN_PROGRESS","conclusion":"","text":""},{"name":"merge-gate","status":"COMPLETED","conclusion":"SUCCESS","text":""}]},{"number":2,"head_ref":"feature-2","base_ref":"main","title":"random 2/2","is_draft":false,"review_decision":"REVIEW_REQUIRED","additions":50,"deletions":6,"commits_behind":2,"labels":["priority:3"],"changed_files":["src/1.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"SUCCESS","text":""},{"name":"merge-gate","status":"COMPLETED","conclusion":"CANCELLED","text":""}]},{"number":3,"head_ref":"feature-3","base_ref":"main","title":"random 2/3","is_draft":true,"review_decision":"APPROVED","additions":38,"deletions":54,"commits_behind":3,"labels":["priority:1"],"changed_files":["src/3.rs"],"checks":[{"name":"CI","status":"IN_PROGRESS","conclusion":"","text":""},{"name":"merge-gate","status":"COMPLETED","conclusion":"CANCELLED","text":""}]},{"number":4,"head_ref":"feature-4","base_ref":"main","title":"random 2/4","is_draft":false,"review_decision":"APPROVED","additions":26,"deletions":21,"commits_behind":4,"labels":["priority:3"],"changed_files":["src/3.rs"],"checks":[{"name":"CI","status":"IN_PROGRESS","conclusion":"","text":""},{"name":"merge-gate","status":"COMPLETED","conclusion":"FAILURE","text":"stale"}]},{"number":5,"head_ref":"feature-5","base_ref":"feature-4","title":"random 2/5","is_draft":false,"review_decision":"APPROVED","additions":8,"deletions":17,"commits_behind":4,"labels":["priority:2"],"changed_files":["src/2.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"CANCELLED","text":""},{"name":"merge-gate","status":"COMPLETED","conclusion":"SUCCESS","text":""}]},{"number":6,"head_ref":"feature-6","base_ref":"main","title":"random 2/6","is_draft":false,"review_decision":"REVIEW_REQUIRED","additions":37,"deletions":72,"commits_behind":1,"labels":["priority:2"],"changed_files":["src/0.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"FAILURE","text":"assertion"},{"name":"merge-gate","status":"COMPLETED","conclusion":"FAILURE","text":"stale"}]}],"conflicts":[]}"###,
                &[
                    "--batch",
                    "--freshness-max-behind",
                    "1",
                    "--conflict-detector",
                    "file-overlap",
                ],
                "3e0a654bc5a1d0732265abb2085b5abed1571b7d53c1b422205c0ad0edf808ed",
            ),
            (
                r###"{"repo":"R","base":"main","prs":[{"number":1,"head_ref":"feature-1","base_ref":"main","title":"random 3/1","is_draft":false,"review_decision":"REVIEW_REQUIRED","additions":75,"deletions":74,"commits_behind":4,"labels":["priority:0"],"changed_files":["src/1.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"FAILURE","text":"assertion"},{"name":"merge-gate","status":"COMPLETED","conclusion":"CANCELLED","text":""}]},{"number":2,"head_ref":"feature-2","base_ref":"feature-1","title":"random 3/2","is_draft":true,"review_decision":"","additions":85,"deletions":5,"commits_behind":1,"labels":["priority:1"],"changed_files":["src/2.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"FAILURE","text":"assertion"},{"name":"merge-gate","status":"COMPLETED","conclusion":"FAILURE","text":"stale"}]},{"number":3,"head_ref":"feature-3","base_ref":"main","title":"random 3/3","is_draft":false,"review_decision":"APPROVED","additions":89,"deletions":62,"commits_behind":4,"labels":["priority:1"],"changed_files":["src/2.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"CANCELLED","text":""},{"name":"merge-gate","status":"COMPLETED","conclusion":"SUCCESS","text":""}]},{"number":4,"head_ref":"feature-4","base_ref":"main","title":"random 3/4","is_draft":false,"review_decision":"APPROVED","additions":63,"deletions":87,"commits_behind":1,"labels":["priority:0"],"changed_files":["src/1.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"SUCCESS","text":""},{"name":"merge-gate","status":"COMPLETED","conclusion":"FAILURE","text":"stale"}]},{"number":5,"head_ref":"feature-5","base_ref":"main","title":"random 3/5","is_draft":false,"review_decision":"REVIEW_REQUIRED","additions":99,"deletions":50,"commits_behind":2,"labels":["priority:1"],"changed_files":["src/4.rs"],"checks":[{"name":"CI","status":"IN_PROGRESS","conclusion":"","text":""},{"name":"merge-gate","status":"IN_PROGRESS","conclusion":"","text":""}]},{"number":6,"head_ref":"feature-6","base_ref":"main","title":"random 3/6","is_draft":false,"review_decision":"REVIEW_REQUIRED","additions":52,"deletions":5,"commits_behind":4,"labels":["priority:0"],"changed_files":["src/2.rs"],"checks":[{"name":"CI","status":"COMPLETED","conclusion":"CANCELLED","text":""},{"name":"merge-gate","status":"COMPLETED","conclusion":"SUCCESS","text":""}]}],"conflicts":[{"a":2,"b":6,"paths":["src/shared.rs"]}]}"###,
                &[
                    "--batch",
                    "--freshness-max-behind",
                    "2",
                    "--conflict-detector",
                    "file-overlap",
                ],
                "7e593dcc956ea5c5de890344cca09bda0de5b3fd19169e973b47dfa3b5b869ed",
            ),
        ];
        for (index, (fixture_body, extra, golden_sha)) in cases.iter().enumerate() {
            let path = temp_file(&format!("random-{index}.json"), fixture_body);
            let path_arg = path.to_string_lossy().into_owned();
            let mut args = vec!["plan", "--fixture", path_arg.as_str(), "--format", "json"];
            args.extend_from_slice(extra);
            let (code, stdout, stderr) = invoke(&args);
            assert_eq!(code, 0, "random:{index}: {stderr}");
            assert_eq!(
                sha256_hex(&stdout),
                *golden_sha,
                "random:{index}/plan-json bytes drift"
            );
            let parsed: Value = serde_json::from_str(&stdout).unwrap();
            assert!(parsed["nodes"].is_array(), "random:{index} nodes");
            let _ = fs::remove_file(path);
        }
    }
}
