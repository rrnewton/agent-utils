//! Monotonic resolution of commits of other repositories that this one pins (`pins` in the
//! configuration). A repository can pin one commit of another in several places at once: a
//! submodule gitlink, and files that repeat its hash (dependency manifests, lock files, build
//! files). When a change moves such a pin and the target it is combined onto pins a different
//! commit, the queue takes the descendant at every site, so a pin never moves backwards, and
//! records the decision. It refuses the change instead when the two pins diverged (neither is an
//! ancestor of the other), when the change's pin is not on the pinned repository's branch, and
//! when the change updates the pin at some sites but not others.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::git::Git;
use crate::plan::short;
use crate::state::PinConfig;

/// One pin decision, for the run report and the change's history.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Resolution {
    /// The change that moved the pin.
    pub pr: i64,
    /// The pin's configured name.
    pub pin: String,
    /// The pin the target carried.
    pub target: String,
    /// The pin the change carried.
    pub change: String,
    /// The descendant, now at every site.
    pub chosen: String,
    /// Sites that carry it: the gitlink and the declared files.
    pub sites: usize,
}

impl Resolution {
    /// One line for an event or a history entry.
    pub fn describe(&self) -> String {
        format!(
            "pin {:?}: the target carried {}, #{} carried {}; {} is the descendant and is now at all {} sites",
            self.pin,
            short(&self.target),
            self.pr,
            short(&self.change),
            short(&self.chosen),
            self.sites
        )
    }
}

/// What resolving a combined change's pins decided.
#[derive(Debug, Eq, PartialEq)]
pub enum Outcome {
    /// The tip to continue from (the combined tip, or a commit on it that sets the pins), and
    /// the decisions to record.
    Resolved(String, Vec<Resolution>),
    /// The change cannot land as combined: a pin refusal (`kind` `pin`) or a conflict in a
    /// pinned file outside its pin hashes (`kind` `conflict`), with the reason.
    Refused(&'static str, String),
}

/// A full 40-hex commit hash at `bytes[at..]`, not part of a longer hex run.
fn hash_at(bytes: &[u8], at: usize) -> bool {
    let hex = |b: u8| b.is_ascii_digit() || (b'a'..=b'f').contains(&b);
    at + 40 <= bytes.len()
        && bytes[at..at + 40].iter().all(|b| hex(*b))
        && (at == 0 || !hex(bytes[at - 1]))
        && (at + 40 == bytes.len() || !hex(bytes[at + 40]))
}

/// Every full 40-hex hash in `bytes`.
fn hashes(bytes: &[u8]) -> BTreeSet<String> {
    (0..bytes.len().saturating_sub(39))
        .filter(|at| hash_at(bytes, *at))
        .map(|at| String::from_utf8_lossy(&bytes[at..at + 40]).into_owned())
        .collect()
}

/// Whether `bytes` contains `hash` as a full hash.
fn contains_hash(bytes: &[u8], hash: &str) -> bool {
    let needle = hash.as_bytes();
    (0..bytes.len().saturating_sub(39))
        .any(|at| bytes[at..].starts_with(needle) && hash_at(bytes, at))
}

/// `bytes` with every full occurrence of each of `from` replaced by `to`.
fn replace_hashes(bytes: &[u8], from: &BTreeSet<String>, to: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if hash_at(bytes, at)
            && from
                .iter()
                .any(|hash| bytes[at..].starts_with(hash.as_bytes()))
        {
            out.extend_from_slice(to.as_bytes());
            at += 40;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    out
}

/// Resolve every configured pin for the change at `head`, combined onto `target` as `combined`;
/// `settled` names the paths whose conflicts the combination settled on the target's side. A
/// change that touches none of a pin's sites leaves that pin alone, whatever the target
/// carries: only a change that moves a pin, or edits a pinned file, is checked.
pub fn resolve(
    git: &Git,
    pins: &[PinConfig],
    target: &str,
    head: &str,
    combined: &str,
    settled: &[String],
    pr: i64,
) -> Result<Outcome, String> {
    let mut tip = combined.to_owned();
    let mut resolutions = Vec::new();
    for pin in pins {
        match resolve_one(git, pin, target, head, &tip, settled, pr)? {
            Outcome::Resolved(next, decided) => {
                tip = next;
                resolutions.extend(decided);
            }
            refused => return Ok(refused),
        }
    }
    Ok(Outcome::Resolved(tip, resolutions))
}

fn refused(reason: String) -> Result<Outcome, String> {
    Ok(Outcome::Refused("pin", reason))
}

fn resolve_one(
    git: &Git,
    pin: &PinConfig,
    target: &str,
    head: &str,
    combined: &str,
    settled: &[String],
    pr: i64,
) -> Result<Outcome, String> {
    let untouched = Ok(Outcome::Resolved(combined.to_owned(), Vec::new()));
    let base = git.merge_base(target, head)?;
    let touched: BTreeSet<String> = git
        .diff_files(&base, head)?
        .into_iter()
        .filter(|path| pin.covers(path))
        .collect();
    if touched.is_empty() {
        return untouched;
    }
    let name = &pin.name;
    let gitlink = |commit: &str| git.gitlink(commit, &pin.gitlink);
    let (Some(at_target), Some(at_change)) = (gitlink(target)?, gitlink(head)?) else {
        return refused(format!(
            "pin {name:?}: the gitlink {} is missing on the target or in the change, so the pin cannot be compared",
            pin.gitlink
        ));
    };
    let at_base = gitlink(&base)?;
    let mut known: BTreeSet<String> = [at_target.clone(), at_change.clone()].into();
    known.extend(at_base.clone());
    let sites = |commit: &str| -> Result<Vec<String>, String> {
        Ok(git
            .paths_in(commit)?
            .into_iter()
            .filter(|path| *path != pin.gitlink && pin.covers(path))
            .collect())
    };
    // Every pinned file on a side that carries any pin this decision knows (the target's, the
    // change's, their merge base's) carries that side's pin and no other: a site left at an
    // older pin, or holding two, is a partial update. A declared file with none of them has no
    // pin site.
    let partial = |commit: &str, carried: &str| -> Result<Vec<String>, String> {
        let mut wrong = Vec::new();
        for path in sites(commit)? {
            let Some(bytes) = git.file_bytes(commit, &path)? else {
                continue;
            };
            let here: Vec<&String> = known
                .iter()
                .filter(|hash| contains_hash(&bytes, hash))
                .collect();
            if here.is_empty() {
                continue;
            }
            if let Some(other) = here.iter().find(|hash| hash.as_str() != carried) {
                wrong.push(if here.iter().any(|hash| hash.as_str() == carried) {
                    format!("{path} also carries {}", short(other))
                } else {
                    format!("{path} is at {}", short(other))
                });
            }
        }
        Ok(wrong)
    };
    let mut wrong = partial(head, &at_change)?;
    // A file the change rewrote from a known pin to a new hash and no known pin: it moved those
    // sites to a commit no gitlink names, without moving the gitlink. (One that only dropped
    // its pin lines removed a dependency.)
    for path in touched.iter().filter(|path| **path != pin.gitlink) {
        let carries = |bytes: &[u8]| known.iter().any(|hash| contains_hash(bytes, hash));
        let (Some(before), Some(after)) =
            (git.file_bytes(&base, path)?, git.file_bytes(head, path)?)
        else {
            continue;
        };
        let added = hashes(&after).difference(&hashes(&before)).count() > 0;
        if carries(&before) && !carries(&after) && added {
            wrong.push(format!(
                "{path} no longer carries {}",
                short(at_base.as_deref().unwrap_or(&at_change))
            ));
        }
    }
    if !wrong.is_empty() {
        return refused(format!(
            "pin {name:?}: a partial update: the change's gitlink is {} but {}",
            short(&at_change),
            wrong.join(", ")
        ));
    }
    let moved = at_base.as_deref() != Some(at_change.as_str());
    let chosen = if !moved || at_target == at_change {
        // The change keeps the pin its base had (or already agrees with the target): the
        // target's pin stands, and only an edit to a pinned file needs merging below.
        at_target.clone()
    } else {
        let wrong = partial(target, &at_target)?;
        if !wrong.is_empty() {
            return refused(format!(
                "pin {name:?}: the target's pin is inconsistent: its gitlink is {} but {}",
                short(&at_target),
                wrong.join(", ")
            ));
        }
        match order(git, pin, &at_target, &at_change)? {
            Ok(chosen) => chosen,
            Err(reason) => return refused(reason),
        }
    };
    // The resolved sites: the gitlink at the chosen pin, and each pinned file as a three-way
    // merge of the target's, the base's and the change's versions with every known pin hash
    // set to the chosen one, so the pin hashes never conflict and no other edit is lost.
    let mut entries: Vec<(String, String, String)> = Vec::new();
    if git.gitlink(combined, &pin.gitlink)?.as_deref() != Some(chosen.as_str()) {
        entries.push(("160000".to_owned(), chosen.clone(), pin.gitlink.clone()));
    }
    let mut files: BTreeSet<String> = sites(combined)?.into_iter().collect();
    files.extend(sites(target)?);
    files.extend(sites(head)?);
    let modes = file_modes(git, combined)?;
    let mut carried = 1;
    for path in &files {
        let versions = (
            git.file_bytes(target, path)?,
            git.file_bytes(&base, path)?,
            git.file_bytes(head, path)?,
        );
        // A declared file with no pin in any version is no pin site; it is merged again only
        // when the combination settled a conflict in it (which may have dropped the change's
        // side of a hunk).
        let pinned = [&versions.0, &versions.1, &versions.2]
            .iter()
            .any(|version| {
                version
                    .as_deref()
                    .is_some_and(|bytes| known.iter().any(|hash| contains_hash(bytes, hash)))
            });
        if !pinned && !settled.contains(path) {
            continue;
        }
        let now = git.file_bytes(combined, path)?;
        let normal = |bytes: &[u8]| replace_hashes(bytes, &known, &chosen);
        let conflict = |why: &str| {
            Ok(Outcome::Refused(
                "conflict",
                format!("{path} ({why}, outside the pin {name:?})"),
            ))
        };
        // The file as it should be (`None`: absent), merged with every pin set to the chosen one.
        let wanted = match versions {
            (Some(ours), Some(base_bytes), Some(theirs)) => {
                match git.merge_texts(&normal(&ours), &normal(&base_bytes), &normal(&theirs))? {
                    Some(merged) => Some(merged),
                    None => return conflict("both sides changed it"),
                }
            }
            // Both added it: one version, or a conflict.
            (Some(ours), None, Some(theirs)) => {
                match git.merge_texts(&normal(&ours), b"", &normal(&theirs))? {
                    Some(merged) => Some(merged),
                    None => return conflict("both sides added it"),
                }
            }
            // Deleted on one side: the deletion stands unless the other side changed more than
            // its pins.
            (Some(kept), Some(base_bytes), None) | (None, Some(base_bytes), Some(kept)) => {
                if normal(&kept) != normal(&base_bytes) {
                    return conflict("one side deleted it, the other changed it");
                }
                None
            }
            (Some(only), None, None) | (None, None, Some(only)) => Some(normal(&only)),
            (None, _, None) => None,
        };
        if wanted
            .as_deref()
            .is_some_and(|bytes| contains_hash(bytes, &chosen))
        {
            carried += 1;
        }
        if now != wanted {
            entries.push(match &wanted {
                Some(bytes) => (
                    modes
                        .get(path)
                        .cloned()
                        .unwrap_or_else(|| "100644".to_owned()),
                    git.write_blob(bytes)?,
                    path.clone(),
                ),
                None => ("0".to_owned(), "0".repeat(40), path.clone()),
            });
        }
    }
    let next = if entries.is_empty() {
        combined.to_owned()
    } else {
        let tree = git.tree_with(&git.tree_of(combined)?, &entries)?;
        git.commit_on(
            &tree,
            combined,
            &format!(
                "Pin {name} at {}\n\nmerge-queue: the descendant of the target's {} and #{pr}'s {}, at every pinned site.",
                short(&chosen),
                short(&at_target),
                short(&at_change)
            ),
        )?
    };
    let decided = if moved && at_target != at_change {
        vec![Resolution {
            pr,
            pin: name.clone(),
            target: at_target,
            change: at_change,
            chosen,
            sites: carried,
        }]
    } else {
        Vec::new()
    };
    Ok(Outcome::Resolved(next, decided))
}

/// Modes of the files in `commit`'s tree, by path.
fn file_modes(git: &Git, commit: &str) -> Result<BTreeMap<String, String>, String> {
    Ok(git
        .run(&["ls-tree", "-r", "--full-tree", commit])?
        .lines()
        .filter_map(|line| {
            let (meta, path) = line.split_once('\t')?;
            Some((path.to_owned(), meta.split(' ').next()?.to_owned()))
        })
        .collect())
}

/// The descendant of the two pins, or why they cannot be ordered: diverged, missing from the
/// pinned repository, or (when the change's pin wins) not on its branch. The branch is fetched
/// only when a commit this needs is missing.
fn order(
    git: &Git,
    pin: &PinConfig,
    at_target: &str,
    at_change: &str,
) -> Result<Result<String, String>, String> {
    let name = &pin.name;
    let branch = pin.branch();
    let tracking = format!("refs/merge-queue/pins/{name}");
    let mut fetched = false;
    let mut fetch = |git: &Git| -> Result<(), String> {
        if !fetched {
            fetched = true;
            git.run_net(&[
                "fetch",
                "--quiet",
                "--no-tags",
                "--no-write-fetch-head",
                &pin.url,
                &format!("+refs/heads/{branch}:{tracking}"),
            ])
            .map_err(|error| format!("pin {name:?}: cannot fetch {} {branch}: {error}", pin.url))?;
        }
        Ok(())
    };
    for commit in [at_target, at_change] {
        if !git.has_commit(commit) {
            fetch(git)?;
        }
    }
    for commit in [at_target, at_change] {
        if !git.has_commit(commit) {
            return Ok(Err(format!(
                "pin {name:?}: {} is not on {} {branch}, so it cannot be ordered",
                short(commit),
                pin.url
            )));
        }
    }
    let chosen = if git.is_ancestor(at_target, at_change)? {
        at_change
    } else if git.is_ancestor(at_change, at_target)? {
        at_target
    } else {
        return Ok(Err(format!(
            "pin {name:?}: the target's {} and the change's {} diverged: neither is an ancestor of the other",
            short(at_target),
            short(at_change)
        )));
    };
    if chosen == at_change {
        // The change brings this pin in: it must be on the pinned repository's branch.
        let on_branch = |git: &Git| {
            git.try_rev_parse(&tracking)
                .is_some_and(|tip| git.is_ancestor(chosen, &tip).unwrap_or(false))
        };
        if !on_branch(git) {
            fetch(git)?;
            if !on_branch(git) {
                return Ok(Err(format!(
                    "pin {name:?}: the change's {} is not on {} {branch}",
                    short(chosen),
                    pin.url
                )));
            }
        }
    }
    Ok(Ok(chosen.to_owned()))
}

#[cfg(test)]
mod unit {
    use super::*;

    const A: &str = "1111111111111111111111111111111111111111";
    const B: &str = "2222222222222222222222222222222222222222";

    #[test]
    fn only_whole_hashes_are_pin_sites() {
        let text = format!("rev = \"{A}\"\nlonger = \"{A}0\"\nsource = \"x?rev={A}#{A}\"\n");
        assert!(contains_hash(text.as_bytes(), A));
        assert!(!contains_hash(text.as_bytes(), B));
        let replaced = replace_hashes(text.as_bytes(), &[A.to_owned()].into(), B);
        assert_eq!(
            String::from_utf8(replaced).unwrap(),
            format!("rev = \"{B}\"\nlonger = \"{A}0\"\nsource = \"x?rev={B}#{B}\"\n")
        );
        // A hash inside a longer hex run is not one.
        assert!(!contains_hash(format!("0{A}").as_bytes(), A));
    }
}
