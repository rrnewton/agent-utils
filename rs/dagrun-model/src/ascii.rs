//! Compact ASCII rendering of a DAG collapsed by group.

use std::collections::{BTreeMap, HashMap, HashSet};

use std::fmt::Write as _;

use crate::model::{step_classification, DagConfig, Step};

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
            let kept: Vec<String> = s
                .deps
                .iter()
                .filter(|d| tags.contains(*d))
                .cloned()
                .collect();
            (s.tag(), kept)
        })
        .collect()
}

fn step_count_text(n: usize) -> String {
    if n == 1 {
        "1 step".to_string()
    } else {
        format!("{n} steps")
    }
}

struct GroupGraph {
    counts: BTreeMap<String, usize>,
    edges: BTreeMap<(String, String), usize>,
    cross_edges: usize,
    intra_edges: usize,
}

fn group_graph(steps: &[&Step], deps: &HashMap<String, Vec<String>>) -> GroupGraph {
    let group_of: HashMap<String, &str> =
        steps.iter().map(|s| (s.tag(), s.group.as_str())).collect();
    let mut g = GroupGraph {
        counts: BTreeMap::new(),
        edges: BTreeMap::new(),
        cross_edges: 0,
        intra_edges: 0,
    };
    for step in steps {
        *g.counts.entry(step.group.clone()).or_default() += 1;
        for dep in &deps[&step.tag()] {
            let from = group_of[dep];
            if from == step.group {
                g.intra_edges += 1;
            } else {
                g.cross_edges += 1;
                *g.edges
                    .entry((from.to_string(), step.group.clone()))
                    .or_default() += 1;
            }
        }
    }
    g
}

/// Render the DAG collapsed to one line per `group`, as compact ASCII.
///
/// Groups are ordered by the earliest dependency layer any of their steps occupies, then by
/// name. Each line shows the step count and the upstream groups with their merged edge counts.
pub fn to_ascii_groups(cfg: &DagConfig, selected: Option<&HashSet<String>>) -> String {
    let steps = selected_steps(cfg, selected);
    let deps = kept_deps(&steps);
    let depth = layers_of(&steps, &deps);
    let g = group_graph(&steps, &deps);

    let mut first_layer: BTreeMap<String, i64> = BTreeMap::new();
    for step in &steps {
        let d = depth[&step.tag()];
        first_layer
            .entry(step.group.clone())
            .and_modify(|v| *v = (*v).min(d))
            .or_insert(d);
    }
    let mut upstream: BTreeMap<String, Vec<(String, usize)>> = BTreeMap::new();
    for ((from, to), n) in &g.edges {
        upstream
            .entry(to.clone())
            .or_default()
            .push((from.clone(), *n));
    }
    let mut order: Vec<(&String, i64)> = first_layer.iter().map(|(k, v)| (k, *v)).collect();
    order.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(b.0)));

    let width = g
        .counts
        .keys()
        .map(|k| k.chars().count())
        .max()
        .unwrap_or(1);
    let count_width = g
        .counts
        .values()
        .map(|n| step_count_text(*n).chars().count())
        .max()
        .unwrap_or(1);

    let mut out: Vec<String> = vec![
        format!(
            "DAG by group - {} groups, {} steps, {} group edges ({} cross-group step edges merged; {} within groups)",
            g.counts.len(),
            steps.len(),
            g.edges.len(),
            g.cross_edges,
            g.intra_edges
        ),
        String::new(),
    ];
    for (group, _) in order {
        let count = step_count_text(g.counts[group]);
        match upstream.get(group) {
            Some(ups) => {
                let ups: Vec<String> = ups.iter().map(|(u, n)| format!("{u} ({n})")).collect();
                out.push(format!(
                    "  {group:<width$}  {count:<count_width$}  <- {}",
                    ups.join(", ")
                ));
            }
            None => out.push(format!("  {group:<width$}  {count}")),
        }
    }
    out.join("\n") + "\n"
}

/// Longest-dependency-depth layer for each step tag.
fn layers_of(steps: &[&Step], deps: &HashMap<String, Vec<String>>) -> HashMap<String, i64> {
    let mut depth: HashMap<String, i64> = HashMap::new();

    fn visit(
        tag: &str,
        deps: &HashMap<String, Vec<String>>,
        depth: &mut HashMap<String, i64>,
    ) -> i64 {
        if let Some(d) = depth.get(tag) {
            return *d;
        }
        let parents = deps.get(tag).cloned().unwrap_or_default();
        let d = if parents.is_empty() {
            0
        } else {
            1 + parents
                .iter()
                .map(|p| visit(p, deps, depth))
                .max()
                .unwrap_or(0)
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
            let mut res = String::new();
            for (key, value) in &step.hint.resources {
                write!(&mut res, " {{{key}:{value}}}").expect("writing to a String cannot fail");
            }
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
