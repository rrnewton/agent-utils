//! Human-readable DAG visualizations: Graphviz DOT and quick ASCII art.
//!
//! Pure functions over a [`DagConfig`] — no I/O. Direct port of
//! `py/safe_ci_dag_runner/viz.py`; the output is BYTE-IDENTICAL to the Python build (same
//! headers, cluster order, edge lines, ASCII layers) and cross-tested against it.
//!
//! * [`to_dot`] emits Graphviz: one cluster per group, solid dependency edges, and a dashed
//!   edge chain for each cap-1 scarce resource.
//! * [`to_ascii`] emits a compact topological-layer view for a glance in the terminal.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::model::{step_classification, DagConfig, Step};

/// Selected steps (all, since the CLI never passes a subset). Kept as a helper to mirror the
/// Python structure and leave room for a future `--select` flag.
fn selected_steps<'a>(cfg: &'a DagConfig, selected: Option<&HashSet<String>>) -> Vec<&'a Step> {
    cfg.steps
        .iter()
        .filter(|s| selected.is_none_or(|sel| sel.contains(&s.tag())))
        .collect()
}

/// Each step's deps, filtered to steps actually present in the selection (order preserved).
fn kept_deps(steps: &[&Step]) -> HashMap<String, Vec<String>> {
    let tags: HashSet<String> = steps.iter().map(|s| s.tag()).collect();
    steps
        .iter()
        .map(|s| {
            let kept: Vec<String> = s.deps.iter().filter(|d| tags.contains(*d)).cloned().collect();
            (s.tag(), kept)
        })
        .collect()
}

/// Render the DAG as Graphviz DOT.
pub fn to_dot(cfg: &DagConfig, name: &str, selected: Option<&HashSet<String>>) -> String {
    let steps = selected_steps(cfg, selected);
    let deps = kept_deps(&steps);

    // Groups in sorted order; steps within a group sorted by job (mirrors Python).
    let mut by_group: BTreeMap<String, Vec<&Step>> = BTreeMap::new();
    for step in &steps {
        by_group.entry(step.group.clone()).or_default().push(step);
    }

    let mut out: Vec<String> = vec![
        format!("digraph {name} {{"),
        "  rankdir=LR;".to_string(),
        "  node [shape=box, style=rounded, fontsize=10];".to_string(),
        "  labelloc=\"t\";".to_string(),
        "  label=\"DAG  (solid = dependency;  dashed = shared cap-1 resource -> serialized)\";"
            .to_string(),
    ];

    for (i, (group, group_steps)) in by_group.iter().enumerate() {
        out.push(format!("  subgraph cluster_{i} {{"));
        out.push(format!("    label=\"{group}\"; style=dashed; color=gray70;"));
        let mut sorted_steps: Vec<&&Step> = group_steps.iter().collect();
        sorted_steps.sort_by(|a, b| a.job.cmp(&b.job));
        for step in sorted_steps {
            let tag = step.tag();
            out.push(format!(
                "    \"{tag}\" [label=\"{tag}\\n[{}]\"];",
                step_classification(step).value()
            ));
        }
        out.push("  }".to_string());
    }

    for step in &steps {
        let tag = step.tag();
        for dep in &deps[&tag] {
            out.push(format!("  \"{dep}\" -> \"{tag}\";"));
        }
    }

    // A dashed chain across the users of each cap-1 resource: a hint that they serialize.
    for (res, cap) in &cfg.resource_caps {
        if *cap == 1 {
            let mut users: Vec<String> = steps
                .iter()
                .filter(|s| s.hint.resources.get(res).copied().unwrap_or(0) > 0)
                .map(|s| s.tag())
                .collect();
            users.sort();
            for pair in users.windows(2) {
                out.push(format!(
                    "  \"{}\" -> \"{}\" [style=dashed, color=gray60, constraint=false, label=\"{res}\"];",
                    pair[0], pair[1]
                ));
            }
        }
    }
    out.push("}".to_string());
    out.join("\n") + "\n"
}

/// Longest-dependency-depth layer for each step tag.
fn layers_of(steps: &[&Step], deps: &HashMap<String, Vec<String>>) -> HashMap<String, i64> {
    let mut depth: HashMap<String, i64> = HashMap::new();

    fn visit(tag: &str, deps: &HashMap<String, Vec<String>>, depth: &mut HashMap<String, i64>) -> i64 {
        if let Some(d) = depth.get(tag) {
            return *d;
        }
        let parents = deps.get(tag).cloned().unwrap_or_default();
        let d = if parents.is_empty() {
            0
        } else {
            1 + parents.iter().map(|p| visit(p, deps, depth)).max().unwrap_or(0)
        };
        depth.insert(tag.to_string(), d);
        d
    }

    for step in steps {
        visit(&step.tag(), deps, &mut depth);
    }
    depth
}

/// Render the DAG as compact ASCII art, grouped by topological layer.
pub fn to_ascii(cfg: &DagConfig, selected: Option<&HashSet<String>>) -> String {
    let steps = selected_steps(cfg, selected);
    let deps = kept_deps(&steps);
    let depth = layers_of(&steps, &deps);

    // depth -> steps (cfg order preserved before the per-layer tag sort).
    let mut layers: BTreeMap<i64, Vec<&Step>> = BTreeMap::new();
    for step in &steps {
        layers.entry(depth[&step.tag()]).or_default().push(step);
    }

    let edge_count: usize = deps.values().map(|d| d.len()).sum();
    let width = steps.iter().map(|s| s.tag().len()).max().unwrap_or(1);

    let mut out: Vec<String> = vec![
        format!(
            "DAG - {} steps, {} edges, {} layer(s)",
            steps.len(),
            edge_count,
            layers.len()
        ),
        String::new(),
    ];

    for (level, layer_steps) in &layers {
        out.push(format!("layer {level}:"));
        let mut sorted_steps: Vec<&&Step> = layer_steps.iter().collect();
        sorted_steps.sort_by_key(|a| a.tag());
        for step in sorted_steps {
            let tag = step.tag();
            let res: String = step
                .hint
                .resources
                .iter()
                .map(|(k, v)| format!(" {{{k}:{v}}}"))
                .collect();
            let mut step_deps = deps[&tag].clone();
            step_deps.sort();
            let dep = if step_deps.is_empty() {
                String::new()
            } else {
                format!("  <- {}", step_deps.join(", "))
            };
            out.push(format!(
                "  {tag:<width$}  [{}]{res}{dep}",
                step_classification(step).value(),
                width = width
            ));
        }
    }
    out.join("\n") + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ResourceHint;

    fn cfg() -> DagConfig {
        let browser = |n: i64| {
            let mut m = BTreeMap::new();
            m.insert("browser".to_string(), n);
            m
        };
        let mk = |group: &str, job: &str, deps: &[&str], res: BTreeMap<String, i64>| Step {
            group: group.into(),
            job: job.into(),
            desc: String::new(),
            cmd: "true".into(),
            deps: deps.iter().map(|s| s.to_string()).collect(),
            env: BTreeMap::new(),
            hint: ResourceHint {
                resources: res,
                ..Default::default()
            },
            networkonly: false,
            engine_only: false,
            timeout: 1800,
        };
        let mut caps = BTreeMap::new();
        caps.insert("browser".to_string(), 1);
        DagConfig {
            steps: vec![
                mk("build", "app", &[], BTreeMap::new()),
                mk("test", "unit", &["build.app"], BTreeMap::new()),
                mk("e2e", "a", &["build.app"], browser(1)),
                mk("e2e", "b", &["build.app"], browser(1)),
            ],
            resource_caps: caps,
            ..Default::default()
        }
    }

    #[test]
    fn dot_has_clusters_nodes_and_edges() {
        let dot = to_dot(&cfg(), "dag", None);
        assert!(dot.starts_with("digraph dag {"));
        assert!(dot.contains("\"build.app\" -> \"test.unit\";"));
        assert!(dot.contains("\"build.app\" -> \"e2e.a\";"));
        assert!(dot.contains("\"e2e.a\" -> \"e2e.b\" [style=dashed"));
        assert!(dot.trim_end().ends_with('}'));
    }

    #[test]
    fn ascii_shows_layers_deps_and_resources() {
        let art = to_ascii(&cfg(), None);
        assert!(art.contains("layer 0:") && art.contains("layer 1:"));
        assert!(art.contains("build.app"));
        assert!(art.contains("<- build.app"));
        assert!(art.contains("{browser:1}"));
    }
}
