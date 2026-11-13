//! Step selection by tag or label, with dependency ancestry.

use std::collections::{HashMap, HashSet};

use crate::model::{DagConfig, Step};

/// Every transitive dependency of each step, over plain and artifact edges.
pub fn transitive_deps(steps: &[Step]) -> HashMap<String, HashSet<String>> {
    // Mixed edges: an artifact producer is a transitive dependency of its
    // consumer (the producer must run and signal), exactly as model
    // validation walks it; see Step::dependency_tags.
    let direct: HashMap<String, Vec<String>> = steps
        .iter()
        .map(|s| (s.tag(), s.dependency_tags().map(str::to_string).collect()))
        .collect();
    let mut result: HashMap<String, HashSet<String>> = HashMap::new();
    for step in steps {
        let tag = step.tag();
        let mut deps: HashSet<String> = HashSet::new();
        let mut stack: Vec<String> = direct
            .get(&tag)
            .into_iter()
            .flatten()
            .rev()
            .cloned()
            .collect();
        while let Some(dep) = stack.pop() {
            if !deps.insert(dep.clone()) {
                continue;
            }
            if let Some(next) = direct.get(&dep) {
                stack.extend(next.iter().rev().cloned());
            }
        }
        result.insert(tag, deps);
    }
    result
}

/// Return the exactly named steps and, by default, every dependency they require.
///
/// `ignore_selected_deps` keeps only the named steps and drops dependency edges to steps outside
/// the selection because their outputs are assumed already present. Edges among selected steps are
/// preserved. Registration order is preserved in both modes, matching `run --selected`.
///
/// The source configuration is never modified. Its complete non-step policy is cloned into the
/// returned configuration. Returns an error when the request is empty or any tag is unknown.
pub fn select_steps_by_tags(
    cfg: &DagConfig,
    tags: &[String],
    ignore_selected_deps: bool,
) -> Result<DagConfig, String> {
    if tags.is_empty() {
        return Err("--selected requires at least one tag".to_string());
    }
    let by_tag: HashSet<String> = cfg.steps.iter().map(Step::tag).collect();
    let unknown: Vec<String> = tags
        .iter()
        .filter(|tag| !by_tag.contains(*tag))
        .cloned()
        .collect();
    if !unknown.is_empty() {
        let mut known: Vec<String> = by_tag.iter().cloned().collect();
        known.sort();
        let known_s = if known.is_empty() {
            "(none)".to_string()
        } else {
            known.join(", ")
        };
        return Err(format!(
            "--selected: unknown step tag(s): {}. Known tags: {known_s}",
            unknown.join(", ")
        ));
    }
    let mut selected: HashSet<String> = tags.iter().cloned().collect();
    if !ignore_selected_deps {
        let dependencies = transitive_deps(&cfg.steps);
        for tag in tags {
            selected.extend(
                dependencies
                    .get(tag)
                    .expect("selected tags were validated against the DAG")
                    .iter()
                    .cloned(),
            );
        }
    }
    let mut new_cfg = cfg.clone();
    new_cfg.steps = cfg
        .steps
        .iter()
        .filter(|s| selected.contains(&s.tag()))
        .map(|s| {
            let mut step = s.clone();
            if ignore_selected_deps {
                step.deps.retain(|dep| selected.contains(dep));
                step.artifact_deps
                    .retain(|dep| selected.contains(&dep.step));
            }
            step
        })
        .collect();
    Ok(new_cfg)
}

/// Return every step carrying any requested label and its dependency ancestry.
///
/// An empty label list selects no steps, preserving the original public-library behavior. The CLI
/// rejects an empty `--labels` value before calling this function.
pub fn select_steps_by_labels(cfg: &DagConfig, labels: &[String]) -> Result<DagConfig, String> {
    if labels.is_empty() {
        return Ok(cfg.with_steps(Vec::new()));
    }
    let known: HashSet<String> = cfg
        .steps
        .iter()
        .flat_map(|step| step.labels.iter().cloned())
        .collect();
    let unknown: Vec<String> = labels
        .iter()
        .filter(|label| !known.contains(*label))
        .cloned()
        .collect();
    if !unknown.is_empty() {
        let mut known = known.into_iter().collect::<Vec<_>>();
        known.sort();
        let known = if known.is_empty() {
            "(none)".to_string()
        } else {
            known.join(", ")
        };
        return Err(format!(
            "--labels: unknown label(s): {}. Known labels: {known}",
            unknown.join(", ")
        ));
    }
    let tags = cfg
        .steps
        .iter()
        .filter(|step| labels.iter().any(|label| step.labels.contains(label)))
        .map(Step::tag)
        .collect::<Vec<_>>();
    select_steps_by_tags(cfg, &tags, false)
}
