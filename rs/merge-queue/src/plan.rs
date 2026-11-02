//! Order and chunk queued changes into batches.
//!
//! The plan is bound to one exact base commit and one exact head per change. It is built from
//! three kinds of evidence, all probed locally with `git merge-tree` (the landing planner's
//! exact-conflict method): conflicts with the base, pairwise conflicts between queued heads, and
//! ancestry (a change stacked on another must land after it).
//!
//! Without repair, two changes that textually conflict cannot both land this round: once one
//! lands, the other conflicts with the base and needs a rebase. The plan therefore picks a
//! conflict-free subset. Priority decides first; among equal priorities the change with fewer
//! conflicts wins (a greedy maximum-independent-set heuristic, which lands more changes per round
//! than first-come-first-served when one change conflicts with many), then submission time.

use std::collections::{BTreeMap, BTreeSet};

use pr_landing_planner::graph::connected_components;
use pr_landing_planner::model::ConflictEdge;

use crate::git::Git;
use crate::model::{
    now, Conflict, Held, Item, ItemStatus, Plan, PlannedBatch, PlannedItem, PLAN_SCHEMA,
};

/// Locally probed relationships between the base and the queued heads.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Probe {
    /// Current head per change; `None` when the head ref no longer resolves.
    pub heads: BTreeMap<i64, Option<String>>,
    /// Files each change touches relative to the base.
    pub files: BTreeMap<i64, BTreeSet<String>>,
    /// Paths each change conflicts on with the base (absent or empty: clean).
    pub base_conflicts: BTreeMap<i64, Vec<String>>,
    /// Pairwise conflicts between heads.
    pub conflicts: Vec<Conflict>,
    /// `(before, after)`: `before`'s head is an ancestor of `after`'s head.
    pub ancestry: Vec<(i64, i64)>,
}

/// Probe the base and queued heads. `heads` maps each queued change to its current head.
pub fn probe(
    git: &Git,
    base_sha: &str,
    heads: &BTreeMap<i64, Option<String>>,
) -> Result<Probe, String> {
    let mut probe = Probe {
        heads: heads.clone(),
        ..Probe::default()
    };
    let live: Vec<(i64, &str)> = heads
        .iter()
        .filter_map(|(pr, head)| head.as_deref().map(|head| (*pr, head)))
        .collect();
    for (pr, head) in &live {
        probe.files.insert(*pr, git.changed_files(base_sha, head)?);
        let paths = git.merge_conflicts(base_sha, head)?;
        if !paths.is_empty() {
            probe.base_conflicts.insert(*pr, paths);
        }
    }
    for (i, (a, head_a)) in live.iter().enumerate() {
        for (b, head_b) in &live[i + 1..] {
            if head_a == head_b {
                continue;
            }
            if git.is_ancestor(head_a, head_b)? {
                probe.ancestry.push((*a, *b));
                continue;
            }
            if git.is_ancestor(head_b, head_a)? {
                probe.ancestry.push((*b, *a));
                continue;
            }
            // Disjoint file sets cannot conflict textually; skip the probe.
            let disjoint = probe.files[a].is_disjoint(&probe.files[b]);
            if !disjoint {
                let paths = git.merge_conflicts(head_a, head_b)?;
                if !paths.is_empty() {
                    probe.conflicts.push(Conflict {
                        a: (*a).min(*b),
                        b: (*a).max(*b),
                        paths,
                    });
                }
            }
        }
    }
    Ok(probe)
}

/// Planning knobs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlanOptions {
    /// Maximum changes per batch (at least 1).
    pub max_batch: usize,
}

type Rank<'a> = (i64, usize, &'a str, i64);

/// Order and chunk the queued items. `all_items` may include landed and ejected records, which
/// satisfy or block explicit dependencies; only `Queued` items are planned.
pub fn make_plan(
    all_items: &[Item],
    base: &str,
    base_sha: &str,
    probe: &Probe,
    options: PlanOptions,
) -> Plan {
    let max_batch = options.max_batch.max(1);
    let by_pr: BTreeMap<i64, &Item> = all_items.iter().map(|item| (item.pr, item)).collect();
    let queued: Vec<&Item> = all_items
        .iter()
        .filter(|item| item.status == ItemStatus::Queued)
        .collect();
    let mut held: BTreeMap<i64, Held> = BTreeMap::new();
    let hold = |held: &mut BTreeMap<i64, Held>, entry: Held| {
        held.entry(entry.pr).or_insert(entry);
    };

    // 1. Identity and base checks.
    for item in &queued {
        match probe.heads.get(&item.pr).cloned().flatten() {
            None => hold(
                &mut held,
                Held {
                    pr: item.pr,
                    kind: "head-missing".into(),
                    reason: format!(
                        "head {:?} no longer resolves; resubmit or withdraw",
                        item.head_ref
                    ),
                    with: vec![],
                    paths: vec![],
                },
            ),
            Some(current) if current != item.head => hold(
                &mut held,
                Held {
                    pr: item.pr,
                    kind: "head-moved".into(),
                    reason: format!(
                        "head moved since submission ({} -> {}); resubmit the new head",
                        short(&item.head),
                        short(&current)
                    ),
                    with: vec![],
                    paths: vec![],
                },
            ),
            Some(_) => {
                if let Some(paths) = probe.base_conflicts.get(&item.pr).filter(|p| !p.is_empty()) {
                    hold(
                        &mut held,
                        Held {
                            pr: item.pr,
                            kind: "conflicts-with-base".into(),
                            reason: format!(
                                "conflicts with {base} on {}; rebase onto {base} and resubmit",
                                paths.join(", ")
                            ),
                            with: vec![],
                            paths: paths.clone(),
                        },
                    );
                }
            }
        }
    }

    // 2. Dependencies: explicit `after` plus head ancestry.
    let queued_set: BTreeSet<i64> = queued.iter().map(|item| item.pr).collect();
    let mut preds: BTreeMap<i64, BTreeSet<i64>> =
        queued_set.iter().map(|pr| (*pr, BTreeSet::new())).collect();
    let mut dependencies = BTreeSet::new();
    for item in &queued {
        for dep in &item.after {
            match by_pr.get(dep).map(|dep| dep.status) {
                Some(ItemStatus::Landed) => {}
                Some(ItemStatus::Queued) => {
                    preds.entry(item.pr).or_default().insert(*dep);
                    dependencies.insert((*dep, item.pr));
                }
                other => hold(
                    &mut held,
                    Held {
                        pr: item.pr,
                        kind: "waiting-on-dependency".into(),
                        reason: format!(
                            "depends on #{dep}, which is {}; submit it or drop the dependency",
                            other.map_or("not in the queue", ItemStatus::as_str)
                        ),
                        with: vec![*dep],
                        paths: vec![],
                    },
                ),
            }
        }
    }
    for (before, after) in &probe.ancestry {
        if queued_set.contains(before) && queued_set.contains(after) {
            preds.entry(*after).or_default().insert(*before);
            dependencies.insert((*before, *after));
        }
    }

    // 3. Conflict-aware selection in dependency order.
    let mut conflicts: BTreeMap<i64, BTreeMap<i64, Vec<String>>> = BTreeMap::new();
    for edge in &probe.conflicts {
        conflicts
            .entry(edge.a)
            .or_default()
            .insert(edge.b, edge.paths.clone());
        conflicts
            .entry(edge.b)
            .or_default()
            .insert(edge.a, edge.paths.clone());
    }
    let eligible = |held: &BTreeMap<i64, Held>, pr: &i64| !held.contains_key(pr);
    let degree = |held: &BTreeMap<i64, Held>, pr: i64| {
        conflicts.get(&pr).map_or(0, |peers| {
            peers.keys().filter(|peer| eligible(held, peer)).count()
        })
    };
    let degrees: BTreeMap<i64, usize> = queued_set
        .iter()
        .map(|pr| (*pr, degree(&held, *pr)))
        .collect();
    let rank = |pr: i64| -> Rank<'_> {
        let item = by_pr[&pr];
        (item.priority, degrees[&pr], item.submitted_at.as_str(), pr)
    };
    let mut remaining: BTreeSet<i64> = queued_set.clone();
    let mut decided: BTreeSet<i64> = BTreeSet::new();
    let mut selected: Vec<i64> = Vec::new();
    while !remaining.is_empty() {
        let ready = remaining
            .iter()
            .copied()
            .filter(|pr| preds[pr].is_subset(&decided))
            .min_by_key(|pr| rank(*pr));
        let Some(pr) = ready.or_else(|| remaining.iter().copied().min_by_key(|pr| rank(*pr)))
        else {
            break;
        };
        remaining.remove(&pr);
        decided.insert(pr);
        if held.contains_key(&pr) {
            continue;
        }
        let blocked_by: Vec<i64> = preds[&pr]
            .iter()
            .copied()
            .filter(|dep| !selected.contains(dep))
            .collect();
        if !blocked_by.is_empty() {
            hold(
                &mut held,
                Held {
                    pr,
                    kind: "waiting-on-dependency".into(),
                    reason: format!(
                        "must land after {}, which cannot land this round",
                        list(&blocked_by)
                    ),
                    with: blocked_by,
                    paths: vec![],
                },
            );
            continue;
        }
        let clashes: Vec<(i64, Vec<String>)> = conflicts
            .get(&pr)
            .into_iter()
            .flatten()
            .filter(|(peer, _)| selected.contains(peer))
            .map(|(peer, paths)| (*peer, paths.clone()))
            .collect();
        if clashes.is_empty() {
            selected.push(pr);
        } else {
            let with: Vec<i64> = clashes.iter().map(|(peer, _)| *peer).collect();
            let paths: BTreeSet<String> =
                clashes.into_iter().flat_map(|(_, paths)| paths).collect();
            hold(
                &mut held,
                Held {
                    pr,
                    kind: "conflicts-with-queued".into(),
                    reason: format!(
                        "conflicts with {} on {}; rebase after {} lands and resubmit",
                        list(&with),
                        paths.iter().cloned().collect::<Vec<_>>().join(", "),
                        if with.len() == 1 { "it" } else { "they" }
                    ),
                    with,
                    paths: paths.into_iter().collect(),
                },
            );
        }
    }

    // 4. Chunk the selected order into batches.
    let mut batches: Vec<PlannedBatch> = Vec::new();
    let mut current: Vec<i64> = Vec::new();
    let close = |current: &mut Vec<i64>, batches: &mut Vec<PlannedBatch>, note: &str| {
        if !current.is_empty() {
            batches.push(PlannedBatch {
                index: batches.len(),
                prs: std::mem::take(current),
                note: note.to_owned(),
            });
        }
    };
    for pr in &selected {
        if by_pr[pr].solo {
            close(&mut current, &mut batches, "");
            current.push(*pr);
            close(&mut current, &mut batches, "solo");
            continue;
        }
        current.push(*pr);
        if current.len() >= max_batch {
            close(&mut current, &mut batches, "max-batch");
        }
    }
    close(&mut current, &mut batches, "");

    let items = queued
        .iter()
        .map(|item| PlannedItem {
            pr: item.pr,
            head: probe
                .heads
                .get(&item.pr)
                .cloned()
                .flatten()
                .unwrap_or_default(),
            priority: item.priority,
            submitted_at: item.submitted_at.clone(),
            files: probe.files.get(&item.pr).map_or(0, BTreeSet::len),
            conflict_degree: degrees[&item.pr],
        })
        .collect();
    Plan {
        schema: PLAN_SCHEMA.to_owned(),
        created_at: now(),
        base: base.to_owned(),
        base_sha: base_sha.to_owned(),
        items,
        batches,
        held: held.into_values().collect(),
        conflicts: probe.conflicts.clone(),
        dependencies: dependencies.into_iter().collect(),
    }
}

/// Conflict clusters (connected components of the pairwise-conflict graph) with two or more
/// members, via the landing planner's component builder.
pub fn clusters(plan: &Plan) -> Vec<Vec<i64>> {
    let numbers: Vec<i64> = plan.items.iter().map(|item| item.pr).collect();
    let edges: Vec<ConflictEdge> = plan
        .conflicts
        .iter()
        .map(|edge| ConflictEdge {
            a: edge.a,
            b: edge.b,
            paths: edge.paths.clone(),
        })
        .collect();
    connected_components(&numbers, &edges)
        .into_iter()
        .filter(|component| component.len() > 1)
        .collect()
}

/// Render a plan for people.
pub fn render(plan: &Plan) -> String {
    let mut out = String::new();
    let queued = plan.items.len();
    let landable: usize = plan.batches.iter().map(|batch| batch.prs.len()).sum();
    out.push_str(&format!(
        "Plan for {} @ {}: {queued} queued, {landable} landable in {} batch(es), {} held\n",
        plan.base,
        short(&plan.base_sha),
        plan.batches.len(),
        plan.held.len()
    ));
    let by_pr: BTreeMap<i64, &PlannedItem> =
        plan.items.iter().map(|item| (item.pr, item)).collect();
    for batch in &plan.batches {
        let members: Vec<String> = batch
            .prs
            .iter()
            .map(|pr| {
                let item = by_pr[pr];
                format!("#{pr} (p{}, {} files)", item.priority, item.files)
            })
            .collect();
        let note = if batch.note.is_empty() {
            String::new()
        } else {
            format!("  [{}]", batch.note)
        };
        out.push_str(&format!(
            "  batch {}: {}{note}\n",
            batch.index + 1,
            members.join(", ")
        ));
    }
    if !plan.held.is_empty() {
        out.push_str("Held:\n");
        for held in &plan.held {
            out.push_str(&format!("  #{} {}: {}\n", held.pr, held.kind, held.reason));
        }
    }
    let clusters = clusters(plan);
    if !clusters.is_empty() {
        out.push_str("Conflict clusters:\n");
        for cluster in clusters {
            out.push_str(&format!("  {}\n", list(&cluster)));
        }
    }
    out
}

/// First 10 characters of a commit ID.
pub fn short(sha: &str) -> &str {
    &sha[..sha.len().min(10)]
}

fn list(prs: &[i64]) -> String {
    prs.iter()
        .map(|pr| format!("#{pr}"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod unit {
    use super::*;

    fn item(pr: i64, priority: i64, at: &str) -> Item {
        Item {
            pr,
            head_ref: format!("b{pr}"),
            head: format!("h{pr}"),
            priority,
            submitted_at: at.to_owned(),
            submitted_by: String::new(),
            title: String::new(),
            after: vec![],
            solo: false,
            claim: None,
            status: ItemStatus::Queued,
            landed_as: String::new(),
            history: vec![],
        }
    }

    fn probe_for(items: &[Item]) -> Probe {
        Probe {
            heads: items
                .iter()
                .map(|item| (item.pr, Some(item.head.clone())))
                .collect(),
            files: items
                .iter()
                .map(|item| (item.pr, [format!("f{}", item.pr)].into()))
                .collect(),
            ..Probe::default()
        }
    }

    fn conflict(a: i64, b: i64) -> Conflict {
        Conflict {
            a,
            b,
            paths: vec!["shared".into()],
        }
    }

    fn opts(max_batch: usize) -> PlanOptions {
        PlanOptions { max_batch }
    }

    #[test]
    fn orders_by_priority_then_age_and_chunks() {
        let items = vec![item(1, 100, "t3"), item(2, 50, "t9"), item(3, 100, "t1")];
        let plan = make_plan(&items, "main", "B", &probe_for(&items), opts(2));
        let batches: Vec<_> = plan.batches.iter().map(|b| b.prs.clone()).collect();
        assert_eq!(batches, vec![vec![2, 3], vec![1]]);
        assert!(plan.held.is_empty());
    }

    #[test]
    fn hub_conflict_loses_to_its_independent_peers() {
        // #1 conflicts with #2, #3 and #4, which are mutually clean. FIFO would land only #1;
        // fewest-conflicts-first lands three.
        let items = vec![
            item(1, 100, "t1"),
            item(2, 100, "t2"),
            item(3, 100, "t3"),
            item(4, 100, "t4"),
        ];
        let mut probe = probe_for(&items);
        probe.conflicts = vec![conflict(1, 2), conflict(1, 3), conflict(1, 4)];
        let plan = make_plan(&items, "main", "B", &probe, opts(4));
        assert_eq!(plan.batches[0].prs, vec![2, 3, 4]);
        assert_eq!(plan.held.len(), 1);
        assert_eq!(plan.held[0].kind, "conflicts-with-queued");
        assert_eq!(plan.held[0].with, vec![2, 3, 4]);
        assert_eq!(clusters(&plan), vec![vec![1, 2, 3, 4]]);
    }

    #[test]
    fn priority_beats_conflict_degree() {
        let items = vec![item(1, 10, "t1"), item(2, 100, "t2"), item(3, 100, "t3")];
        let mut probe = probe_for(&items);
        probe.conflicts = vec![conflict(1, 2), conflict(1, 3)];
        let plan = make_plan(&items, "main", "B", &probe, opts(4));
        assert_eq!(plan.batches[0].prs, vec![1]);
        assert_eq!(plan.held.len(), 2);
    }

    #[test]
    fn identity_and_base_holds() {
        let items = vec![item(1, 100, "t1"), item(2, 100, "t2"), item(3, 100, "t3")];
        let mut probe = probe_for(&items);
        probe.heads.insert(1, Some("other".into()));
        probe.heads.insert(2, None);
        probe.base_conflicts.insert(3, vec!["x".into()]);
        let plan = make_plan(&items, "main", "B", &probe, opts(4));
        assert!(plan.batches.is_empty());
        let kinds: Vec<_> = plan.held.iter().map(|h| h.kind.as_str()).collect();
        assert_eq!(kinds, ["head-moved", "head-missing", "conflicts-with-base"]);
    }

    #[test]
    fn stacked_change_follows_its_parent_and_inherits_its_hold() {
        let items = vec![item(1, 100, "t2"), item(2, 50, "t1"), item(3, 100, "t3")];
        let mut probe = probe_for(&items);
        probe.ancestry = vec![(1, 2)];
        let plan = make_plan(&items, "main", "B", &probe, opts(4));
        assert_eq!(plan.batches[0].prs, vec![1, 2, 3]);

        probe.base_conflicts.insert(1, vec!["x".into()]);
        let plan = make_plan(&items, "main", "B", &probe, opts(4));
        assert_eq!(plan.batches[0].prs, vec![3]);
        let waiting = plan.held.iter().find(|h| h.pr == 2).unwrap();
        assert_eq!(waiting.kind, "waiting-on-dependency");
    }

    #[test]
    fn explicit_dependencies_and_solo() {
        let mut items = vec![item(1, 100, "t1"), item(2, 100, "t2"), item(3, 100, "t3")];
        items[0].after = vec![3];
        items[1].solo = true;
        let mut landed = item(9, 100, "t0");
        landed.status = ItemStatus::Landed;
        items[2].after = vec![9];
        items.push(landed);
        let plan = make_plan(&items, "main", "B", &probe_for(&items), opts(4));
        let batches: Vec<_> = plan.batches.iter().map(|b| b.prs.clone()).collect();
        assert_eq!(batches, vec![vec![2], vec![3, 1]]);
        assert_eq!(plan.batches[0].note, "solo");

        items[2].after = vec![42];
        let plan = make_plan(&items, "main", "B", &probe_for(&items), opts(4));
        let held: Vec<_> = plan.held.iter().map(|h| (h.pr, h.kind.as_str())).collect();
        assert_eq!(
            held,
            [(1, "waiting-on-dependency"), (3, "waiting-on-dependency")]
        );
    }

    #[test]
    fn probe_finds_real_conflicts_and_stacks() {
        use crate::git::testrepo::*;
        let tmp = TempDir::new("probe");
        let git = init(&tmp.0);
        write(&tmp.0, "shared.txt", "1\n2\n3\n");
        git_commit(&tmp.0);
        let base = git.rev_parse("main").unwrap();
        let a = branch(&tmp.0, "a", "main", &[("shared.txt", "A\n2\n3\n")]);
        let b = branch(&tmp.0, "b", "main", &[("shared.txt", "B\n2\n3\n")]);
        let c = branch(&tmp.0, "c", "main", &[("shared.txt", "1\n2\nC\n")]);
        let d = branch(&tmp.0, "d", "a", &[("d.txt", "d\n")]);
        let heads = [(1, Some(a)), (2, Some(b)), (3, Some(c)), (4, Some(d))].into();
        let probe = probe(&git, &base, &heads).unwrap();
        // a/b conflict on the same line; c touches another hunk of the same file (clean);
        // d is stacked on a, and so conflicts with b too.
        let pairs: Vec<_> = probe.conflicts.iter().map(|c| (c.a, c.b)).collect();
        assert_eq!(pairs, vec![(1, 2), (2, 4)]);
        assert_eq!(probe.ancestry, vec![(1, 4)]);
        assert!(probe.base_conflicts.is_empty());
    }

    fn git_commit(dir: &std::path::Path) {
        use crate::git::testrepo::git;
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "more"]);
    }
}
