//! A self-contained throughput benchmark: landing rate of the queue against a serial lander.
//!
//! It builds a throwaway bare remote with `prs` independent changes (each adds one test file; a
//! seeded fraction of them fail their test), a validator whose every run costs `test_secs` of
//! wall time, and lands the same queue twice from scratch: serially (one change per batch, depth
//! 1, the complete suite every time, the classic one-at-a-time drain) and with the queue's
//! settings (batches, speculative depth, focused selection). It reports changes landed per hour
//! of wall time for each.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::git::CombineMode;
use crate::ops::{self, Submission};
use crate::run::{RunOptions, RunReport, Runner};
use crate::state::{Config, LandConfig, State, ValidatorConfig, DEFAULT_PRIORITY};

/// Benchmark parameters.
#[derive(Clone, Debug)]
pub struct BenchOptions {
    /// Number of changes.
    pub prs: usize,
    /// Fraction of changes that fail their test (0.0 to 1.0).
    pub fail_rate: f64,
    /// Wall seconds every validation costs.
    pub test_secs: f64,
    /// Queue speculative depth.
    pub depth: usize,
    /// Queue maximum batch size.
    pub max_batch: usize,
    /// Seed for choosing the failing changes.
    pub seed: u64,
    /// Keep the fixture here instead of deleting it.
    pub keep: Option<PathBuf>,
}

/// One configuration's outcome.
#[derive(Clone, Debug, Serialize)]
pub struct BenchRow {
    /// Configuration name.
    pub name: String,
    /// Depth used.
    pub depth: usize,
    /// Maximum batch size used.
    pub max_batch: usize,
    /// Selection used for batch validations (`all` or `focused`).
    pub selection: String,
    /// Changes landed.
    pub landed: usize,
    /// Changes ejected.
    pub ejected: usize,
    /// Validations run.
    pub validations: usize,
    /// Wall seconds.
    pub wall_s: f64,
    /// Changes landed per hour.
    pub prs_per_hour: f64,
}

/// Benchmark result.
#[derive(Clone, Debug, Serialize)]
pub struct BenchReport {
    /// Changes in the queue.
    pub prs: usize,
    /// Changes that fail their test.
    pub failing: Vec<i64>,
    /// Seconds per validation.
    pub test_secs: f64,
    /// One row per configuration.
    pub rows: Vec<BenchRow>,
    /// Queue rate divided by serial rate.
    pub speedup: f64,
}

const RUNNER: &str = r#"if [ "$MQ_SELECTION" = all ]; then set -- $(ls tests); else set -- $MQ_TESTS; fi
rc=0
for t in "$@"; do
  o=pass
  if [ -f "tests/$t" ] && grep -q FAIL "tests/$t"; then o=fail; rc=1; fi
  printf '{"id":"%s","outcome":"%s"}\n' "$t" "$o" >> "$MQ_RESULTS"
done
sleep "$MQ_BENCH_SECS"
exit $rc"#;

const SELECT: &str = "git diff --name-only \"$MQ_BASE\" HEAD -- tests | sed 's|^tests/||'";

fn sh(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "bench")
        .env("GIT_AUTHOR_EMAIL", "bench@example.com")
        .env("GIT_COMMITTER_NAME", "bench")
        .env("GIT_COMMITTER_EMAIL", "bench@example.com")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .map_err(|error| format!("cannot run git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Deterministic choice of failing changes (a small linear congruential generator).
fn failing(prs: usize, rate: f64, seed: u64) -> Vec<i64> {
    let count = ((prs as f64) * rate.clamp(0.0, 1.0)).round() as usize;
    let mut state = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    let mut chosen = std::collections::BTreeSet::new();
    while chosen.len() < count.min(prs) {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        chosen.insert(((state >> 33) % prs as u64) as i64 + 1);
    }
    chosen.into_iter().collect()
}

fn fixture(root: &Path, prs: usize, bad: &[i64]) -> Result<(), String> {
    std::fs::create_dir_all(root).map_err(|e| format!("cannot create {}: {e}", root.display()))?;
    sh(root, &["init", "-q", "--bare", "-b", "main", "remote.git"])?;
    sh(root, &["clone", "-q", "remote.git", "dev"])?;
    let dev = root.join("dev");
    std::fs::create_dir_all(dev.join("tests")).map_err(|e| e.to_string())?;
    std::fs::write(dev.join("tests/t000"), "ok\n").map_err(|e| e.to_string())?;
    sh(&dev, &["add", "-A"])?;
    sh(&dev, &["commit", "-q", "-m", "base"])?;
    sh(&dev, &["push", "-q", "origin", "HEAD:main"])?;
    for pr in 1..=prs as i64 {
        let name = format!("t{pr:03}");
        sh(&dev, &["checkout", "-q", "-B", &name, "main"])?;
        let body = if bad.contains(&pr) { "FAIL\n" } else { "ok\n" };
        std::fs::write(dev.join("tests").join(&name), body).map_err(|e| e.to_string())?;
        sh(&dev, &["add", "-A"])?;
        sh(&dev, &["commit", "-q", "-m", &name])?;
        sh(&dev, &["push", "-q", "origin", &format!("HEAD:{name}")])?;
    }
    Ok(())
}

/// Build a fresh fixture under `root/name` and drain it with the given settings.
fn land_all(
    root: &Path,
    name: &str,
    bad: &[i64],
    prs: usize,
    shape: (usize, usize),
    focused: bool,
    secs: f64,
) -> Result<RunReport, String> {
    let (depth, max_batch) = shape;
    let root = root.join(name);
    fixture(&root, prs, bad)?;
    let queue = root.join("queue");
    sh(&root, &["clone", "-q", "remote.git", "queue"])?;
    let state = State::at(queue.join(".git/merge-queue"));
    let config = Config {
        repo: queue.clone(),
        remote: "origin".to_owned(),
        combine: CombineMode::Rebase,
        max_batch,
        depth,
        validator: ValidatorConfig {
            kind: "command".to_owned(),
            run: RUNNER.replace("$MQ_BENCH_SECS", &format!("{secs}")),
            select: if focused {
                SELECT.to_owned()
            } else {
                String::new()
            },
            policy: if focused {
                "path-routing".to_owned()
            } else {
                String::new()
            },
            ..ValidatorConfig::default()
        },
        land: LandConfig {
            driver: "push".to_owned(),
            ..LandConfig::default()
        },
        ..Config::default()
    };
    state.init(&config)?;
    for pr in 1..=prs as i64 {
        ops::submit(
            &state,
            &config,
            Submission {
                pr,
                head_ref: format!("t{pr:03}"),
                priority: DEFAULT_PRIORITY,
                ..Submission::default()
            },
        )?;
    }
    Runner::new(
        &state,
        RunOptions {
            quiet: true,
            ..RunOptions::default()
        },
    )?
    .run()
}

fn row(name: &str, depth: usize, max_batch: usize, focused: bool, report: &RunReport) -> BenchRow {
    BenchRow {
        name: name.to_owned(),
        depth,
        max_batch,
        selection: if focused { "focused" } else { "all" }.to_owned(),
        landed: report.landed.iter().map(|l| l.prs.len()).sum(),
        ejected: report.ejected.len(),
        validations: report.validations.len(),
        wall_s: report.wall_s,
        prs_per_hour: report.prs_per_hour,
    }
}

/// Run the benchmark.
pub fn bench(options: &BenchOptions) -> Result<BenchReport, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let root = options.keep.clone().unwrap_or_else(|| {
        std::env::temp_dir().join(format!("merge-queue-bench-{}-{nanos}", std::process::id()))
    });
    let bad = failing(options.prs, options.fail_rate, options.seed);
    let result = (|| {
        let serial = land_all(
            &root,
            "serial",
            &bad,
            options.prs,
            (1, 1),
            false,
            options.test_secs,
        )?;
        let queue = land_all(
            &root,
            "queue",
            &bad,
            options.prs,
            (options.depth, options.max_batch),
            true,
            options.test_secs,
        )?;
        Ok::<_, String>((serial, queue))
    })();
    if options.keep.is_none() {
        let _ = std::fs::remove_dir_all(&root);
    }
    let (serial, queue) = result?;
    let rows = vec![
        row("serial", 1, 1, false, &serial),
        row("queue", options.depth, options.max_batch, true, &queue),
    ];
    let speedup = if rows[0].prs_per_hour > 0.0 {
        rows[1].prs_per_hour / rows[0].prs_per_hour
    } else {
        0.0
    };
    Ok(BenchReport {
        prs: options.prs,
        failing: bad,
        test_secs: options.test_secs,
        rows,
        speedup,
    })
}
