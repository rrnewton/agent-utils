//! Short labels for links whose text is their own address. `#227 github-link-abbrev`.
//!
//! # Why
//!
//! Agents report what they did with the addresses of what they did it to, and a status line that
//! names a dozen pull requests is a dozen copies of `https://github.com/OWNER/REPO/pull/` around
//! the only part that differs. The owner, reading one: "These PR URLs are annoying. I think we
//! should preprocess our markdown a bit before it goes to html. … Common GitHub link formats should
//! be abbreviated. Just say REPO#123 as a hyperlink instead of the bare url." (`REPO` stands for the
//! repository he named.) So a link whose visible text is its own address is drawn with a label made
//! from that address — `widgets#123`, `widgets@4f21ab0` — and still goes where it went.
//!
//! # What is shortened, and what never is
//!
//! [`crate::render`] asks, for every link it draws, only when the link's text IS its address: a
//! bare address, `<address>`, and the chat services' forms of the same. A link its writer named,
//! `[the fix](address)`, keeps the name it was given; code is never a link at all. The address is
//! untouched — only the words on the screen change — and the full address goes in the link's
//! `title`, so a pointer resting on it still says where it goes. This is the HTML path only: the
//! text a voice reads ([`crate::speakable`]) and the text Copy copies are the message as written.
//!
//! # The labels
//!
//! The repository's name as the address writes it, without the account that owns it, and GitHub's
//! own short forms where GitHub has one: `#` for an issue, a pull request or a discussion, `@` and
//! seven hex digits for a commit. Where GitHub has none, the same `@` names a tag or a branch,
//! and a word names a run. A suffix in parentheses tells apart two links into the same thread, as
//! GitHub's own `(comment)` does.
//!
//! | Address, under `github.com/OWNER/REPO` | Label |
//! |---|---|
//! | `/pull/N`, `/issues/N`, `/discussions/N` | `REPO#N` |
//! | the same `#issuecomment-…`, `#discussion_r…`, `#discussioncomment-…` | `REPO#N (comment)` |
//! | the same `#pullrequestreview-…` | `REPO#N (review)` |
//! | `/pull/N/files`, `/commits`, `/checks` | `REPO#N (files)`, `(commits)`, `(checks)` |
//! | `/commit/SHA`, `/pull/N/commits/SHA` | `REPO@abc1234` |
//! | `/commit/SHA#commitcomment-…` | `REPO@abc1234 (comment)` |
//! | `/compare/A...B`, `/compare/A..B` | `REPO@A...B`, a commit id in it cut to seven |
//! | `/releases/tag/T` | `REPO@T` |
//! | `/tree/REF` | `REPO@REF` |
//! | `/actions/runs/ID` | `REPO run ID` |
//! | `/actions/runs/ID/job/J`, `/attempts/N` | `REPO run ID (job)`, `(attempt N)` |
//! | `/blob/REF/…/FILE`, with `#L10` or `#L10-L20` | `REPO:FILE`, with the same fragment |
//!
//! Anything else is left as it is written: another host, a repository's own page, a user's page, a
//! gist, `/tree/REF/PATH` — where a branch name with a slash in it cannot be told from a
//! directory — and any address whose query holds a parameter not known to change only how the page
//! is shown ([`DISPLAY_ONLY_QUERY`]). A trailing slash, either scheme, `www.` and the host's case
//! change nothing.
//!
//! # A label is never decoded
//!
//! Every part of a label is copied from the address as written, and only when it is made of the
//! characters that part may hold: digits for a number, hex for a commit, letters, digits and `._-+`
//! for a name. A part holding anything else — a percent-escape, a space, a character from another
//! script that could turn the text around it — leaves the whole address as it is. So no label is
//! ever wrong about the address beneath it, and none holds anything that is not plain text.
//!
//! # Other hosts
//!
//! [`HOSTS`] is the table: a host's names and the routes inside it, each a pattern of path segments
//! and the shape of its label. A forge with short references of its own is a row there.

/// The longest address read for a label, in bytes. Far past any address a person pastes; a longer
/// one is left as written, so the work for one link is bounded whatever a message holds.
pub const MAX_ADDRESS: usize = 2048;

/// The longest name, ref or file name a label shows, in bytes.
const MAX_PART: usize = 100;

/// The query parameters that change only how a page is SHOWN, which a label may leave out: GitHub's
/// whitespace and split-diff switches, a file's plain view, the pull request a run is seen from,
/// and the check-suite focus. A query with anything else is one this does not know, and its address
/// is left as written.
const DISPLAY_ONLY_QUERY: [&str; 5] = ["w", "diff", "plain", "pr", "check_suite_focus"];

/// One host whose addresses have short labels.
struct Host {
    /// The names it is reached by, lowercase. Matched exactly, in any case: a port, a user name or
    /// a longer name that merely begins with one of these is another host.
    names: &'static [&'static str],
    /// The label for an address on it, from its parts.
    label: fn(&Address<'_>) -> Option<String>,
}

/// Every host that has short labels. See the module note on adding one.
const HOSTS: [Host; 1] = [Host {
    names: &["github.com", "www.github.com"],
    label: github,
}];

/// An address cut into the parts a label is read from, each a slice of the address as written.
struct Address<'u> {
    /// The path's segments, without the slashes. A single trailing slash is dropped; any other
    /// empty segment means the address is not one a label is made for.
    segments: Vec<&'u str>,
    /// What follows `?`, up to the fragment.
    query: Option<&'u str>,
    /// What follows `#`.
    fragment: Option<&'u str>,
}

/// The short label for `address`, or `None` when it is not one this knows. See the module note.
#[must_use]
pub fn short_label(address: &str) -> Option<String> {
    if address.len() > MAX_ADDRESS {
        return None;
    }
    let rest = strip_scheme(address)?;
    let host_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (host, rest) = rest.split_at(host_end);
    let entry = HOSTS.iter().find(|entry| {
        entry
            .names
            .iter()
            .any(|name| name.eq_ignore_ascii_case(host))
    })?;
    let (rest, fragment) = match rest.split_once('#') {
        Some((rest, fragment)) => (rest, Some(fragment)),
        None => (rest, None),
    };
    let (path, query) = match rest.split_once('?') {
        Some((path, query)) => (path, Some(query)),
        None => (rest, None),
    };
    let path = path.strip_prefix('/').unwrap_or(path);
    let path = path.strip_suffix('/').unwrap_or(path);
    let segments: Vec<&str> = if path.is_empty() {
        Vec::new()
    } else {
        path.split('/').collect()
    };
    if segments.iter().any(|segment| segment.is_empty()) {
        return None;
    }
    (entry.label)(&Address {
        segments,
        query,
        fragment,
    })
}

/// `text` without a leading `http://` or `https://`, in any case, or `None` when it has neither.
#[must_use]
pub fn strip_scheme(text: &str) -> Option<&str> {
    ["https://", "http://"].iter().find_map(|scheme| {
        text.get(..scheme.len())
            .filter(|head| head.eq_ignore_ascii_case(scheme))
            .map(|_| &text[scheme.len()..])
    })
}

// --- GitHub ----------------------------------------------------------------------------------

/// One segment of a route's path, after `OWNER/REPO`.
#[derive(Clone, Copy)]
enum Segment {
    /// This word, exactly as GitHub writes it: lowercase.
    Word(&'static str),
    /// A number — an issue, a pull request, a run, a job — captured.
    Number,
    /// A commit id, seven to sixty-four hex digits, captured.
    Commit,
    /// Any one segment, captured but checked only by the shape that shows it.
    Any,
    /// One segment or more, to the end of the path, captured with its slashes. Last in a route.
    Rest,
}

/// How a route's label is written from the repository's name and what its path captured.
#[derive(Clone, Copy)]
enum Shape {
    /// `REPO#N`, from the first capture, with the suffix its fragment calls for.
    Thread,
    /// `REPO#N (part)`: one tab of a pull request.
    ThreadPart(&'static str),
    /// `REPO@abc1234`, from the last capture.
    Commit,
    /// `REPO@A...B`, from a `compare` path.
    Compare,
    /// `REPO@REF`, a tag or a branch as written, from the last capture.
    Ref,
    /// `REPO run ID`, from the first capture.
    Run,
    /// `REPO run ID (job)`.
    RunJob,
    /// `REPO run ID (attempt N)`, from the first and second captures.
    RunAttempt,
    /// `REPO:FILE`, the last segment of the last capture, and a line fragment if there is one.
    File,
}

/// One kind of page inside a repository.
struct Route {
    /// Its path after `OWNER/REPO`, segment by segment.
    path: &'static [Segment],
    /// How its label is written.
    shape: Shape,
}

/// GitHub's pages that have labels, as GitHub routes them. Matched whole: a path with a segment
/// more or a segment less than a route is not that route.
const GITHUB_ROUTES: [Route; 15] = {
    use Segment::{Any, Commit, Number, Rest, Word};
    [
        Route {
            path: &[Word("pull"), Number],
            shape: Shape::Thread,
        },
        Route {
            path: &[Word("issues"), Number],
            shape: Shape::Thread,
        },
        Route {
            path: &[Word("discussions"), Number],
            shape: Shape::Thread,
        },
        Route {
            path: &[Word("pull"), Number, Word("files")],
            shape: Shape::ThreadPart("files"),
        },
        Route {
            path: &[Word("pull"), Number, Word("commits")],
            shape: Shape::ThreadPart("commits"),
        },
        Route {
            path: &[Word("pull"), Number, Word("checks")],
            shape: Shape::ThreadPart("checks"),
        },
        Route {
            path: &[Word("pull"), Number, Word("commits"), Commit],
            shape: Shape::Commit,
        },
        Route {
            path: &[Word("commit"), Commit],
            shape: Shape::Commit,
        },
        Route {
            path: &[Word("compare"), Rest],
            shape: Shape::Compare,
        },
        Route {
            path: &[Word("releases"), Word("tag"), Rest],
            shape: Shape::Ref,
        },
        Route {
            path: &[Word("tree"), Any],
            shape: Shape::Ref,
        },
        Route {
            path: &[Word("actions"), Word("runs"), Number],
            shape: Shape::Run,
        },
        Route {
            path: &[Word("actions"), Word("runs"), Number, Word("job"), Number],
            shape: Shape::RunJob,
        },
        Route {
            path: &[
                Word("actions"),
                Word("runs"),
                Number,
                Word("attempts"),
                Number,
            ],
            shape: Shape::RunAttempt,
        },
        Route {
            path: &[Word("blob"), Any, Rest],
            shape: Shape::File,
        },
    ]
};

/// The label for an address on GitHub.
fn github(address: &Address<'_>) -> Option<String> {
    let [owner, repo, route @ ..] = address.segments.as_slice() else {
        return None;
    };
    if !owner_name(owner) || !repo_name(repo) {
        return None;
    }
    if !address.query.is_none_or(display_only) {
        return None;
    }
    GITHUB_ROUTES.iter().find_map(|candidate| {
        let captures = captured(candidate.path, route)?;
        written(candidate.shape, repo, &captures, address.fragment)
    })
}

/// What `segments` captures under `pattern`, when it matches the whole of it.
fn captured(pattern: &[Segment], segments: &[&str]) -> Option<Vec<String>> {
    let mut captures = Vec::new();
    let mut at = 0;
    for (index, expected) in pattern.iter().enumerate() {
        if matches!(expected, Segment::Rest) {
            // Last in a route by construction; anything after it would never match.
            if index + 1 != pattern.len() || at >= segments.len() {
                return None;
            }
            captures.push(segments[at..].join("/"));
            return Some(captures);
        }
        let segment = *segments.get(at)?;
        at += 1;
        match expected {
            Segment::Word(word) if segment == *word => {}
            Segment::Number if number(segment) => captures.push(segment.to_owned()),
            Segment::Commit if commit_id(segment) => captures.push(segment.to_owned()),
            Segment::Any => captures.push(segment.to_owned()),
            _ => return None,
        }
    }
    (at == segments.len()).then_some(captures)
}

/// The label of `shape`, or `None` when what was captured cannot be shown as it.
fn written(
    shape: Shape,
    repo: &str,
    captures: &[String],
    fragment: Option<&str>,
) -> Option<String> {
    let first = captures.first()?;
    let last = captures.last()?;
    Some(match shape {
        Shape::Thread => format!("{repo}#{first}{}", thread_suffix(fragment)),
        Shape::ThreadPart(part) => format!("{repo}#{first} ({part})"),
        Shape::Commit => {
            let comment = fragment.is_some_and(|f| numbered(f, "commitcomment-"));
            let suffix = if comment { " (comment)" } else { "" };
            format!("{repo}@{}{suffix}", short_commit(last))
        }
        Shape::Compare => format!("{repo}@{}", comparison(last)?),
        Shape::Ref => {
            if !ref_name(last) {
                return None;
            }
            format!("{repo}@{last}")
        }
        Shape::Run => format!("{repo} run {first}"),
        Shape::RunJob => format!("{repo} run {first} (job)"),
        Shape::RunAttempt => format!("{repo} run {first} (attempt {})", captures.get(1)?),
        Shape::File => {
            let file = last.rsplit('/').next()?;
            if !plain_name(file) {
                return None;
            }
            let lines = fragment.filter(|f| line_fragment(f));
            match lines {
                Some(lines) => format!("{repo}:{file}#{lines}"),
                None => format!("{repo}:{file}"),
            }
        }
    })
}

/// What a fragment on an issue, a pull request or a discussion says it points at, as a suffix.
/// One this does not know points somewhere on the same page, and the label is the thread's.
fn thread_suffix(fragment: Option<&str>) -> &'static str {
    let Some(fragment) = fragment else {
        return "";
    };
    if ["issuecomment-", "discussion_r", "discussioncomment-"]
        .iter()
        .any(|prefix| numbered(fragment, prefix))
    {
        " (comment)"
    } else if numbered(fragment, "pullrequestreview-") {
        " (review)"
    } else {
        ""
    }
}

/// `A...B` or `A..B`, each side a ref, a commit id among them cut to seven, or `None`.
fn comparison(spec: &str) -> Option<String> {
    let separator = if spec.contains("...") { "..." } else { ".." };
    let (base, head) = spec.split_once(separator)?;
    Some(format!("{}{separator}{}", compared(base)?, compared(head)?))
}

/// One side of a comparison as a label shows it: a ref, or another fork's branch, `owner:branch`.
fn compared(side: &str) -> Option<String> {
    let name = match side.split_once(':') {
        Some((owner, branch)) if owner_name(owner) => branch,
        Some(_) => return None,
        None => side,
    };
    // `..` inside a ref is not a ref git allows, so it is never a second separator.
    (ref_name(name) && !name.contains("..")).then(|| abbreviated_ref(side))
}

/// A ref as a label shows it: a commit id cut to seven, anything else as written.
///
/// A commit id here is eight hex digits or more with at least one LETTER among them, so a tag that
/// is all digits — a date, a build number — is shown whole rather than cut.
fn abbreviated_ref(name: &str) -> String {
    let letters = name.bytes().any(|byte| byte.is_ascii_alphabetic());
    if name.len() > 7 && commit_id(name) && letters {
        short_commit(name)
    } else {
        name.to_owned()
    }
}

/// A commit id as GitHub shows one: its first seven hex digits, lowercase.
fn short_commit(id: &str) -> String {
    id.get(..7).unwrap_or(id).to_ascii_lowercase()
}

/// Whether every parameter of `query` only changes how the page is shown.
fn display_only(query: &str) -> bool {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .all(|pair| DISPLAY_ONLY_QUERY.contains(&pair.split_once('=').map_or(pair, |(key, _)| key)))
}

/// Whether `fragment` is `prefix` and then a number.
fn numbered(fragment: &str, prefix: &str) -> bool {
    fragment.strip_prefix(prefix).is_some_and(number)
}

/// A line or a range of lines as GitHub writes one in a file's fragment: `L10`, `L10-L20`, and
/// either with a column, `L10C3`.
fn line_fragment(fragment: &str) -> bool {
    let line = |part: &str| {
        let Some(rest) = part.strip_prefix('L') else {
            return false;
        };
        let (line, column) = rest.split_once('C').unwrap_or((rest, "1"));
        number(line) && number(column)
    };
    match fragment.split_once('-') {
        Some((from, to)) => line(from) && line(to),
        None => line(fragment),
    }
}

/// One to twenty ASCII digits.
fn number(text: &str) -> bool {
    (1..=20).contains(&text.len()) && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// Seven to sixty-four ASCII hex digits: a commit id, short or whole, SHA-1 or SHA-256.
fn commit_id(text: &str) -> bool {
    (7..=64).contains(&text.len()) && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// An account as GitHub allows one: letters, digits, hyphens, and the underscore of a managed
/// account. Checked, never shown.
fn owner_name(text: &str) -> bool {
    (1..=MAX_PART).contains(&text.len())
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// A repository's name as GitHub allows one: letters, digits, `.`, `_` and `-`, and not a path's
/// `.` or `..`.
fn repo_name(text: &str) -> bool {
    (1..=MAX_PART).contains(&text.len())
        && text != "."
        && text != ".."
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// A file's name as a label shows it: letters, digits and `._-+`.
fn plain_name(text: &str) -> bool {
    (1..=MAX_PART).contains(&text.len())
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+'))
}

/// A tag or a branch as a label shows it: [`plain_name`]s joined by single slashes.
fn ref_name(text: &str) -> bool {
    text.len() <= MAX_PART && text.split('/').all(plain_name)
}

#[cfg(test)]
mod tests {
    use super::{short_label, strip_scheme, MAX_ADDRESS};

    const REPO: &str = "https://github.com/octo/widgets";
    const SHA: &str = "4f21ab0c9d8e7f6a5b4c3d2e1f0a9b8c7d6e5f4a";

    fn label(address: &str) -> Option<String> {
        short_label(address)
    }

    #[track_caller]
    fn labels(cases: &[(String, &str)]) {
        for (address, expected) in cases {
            assert_eq!(
                label(address).as_deref(),
                Some(*expected),
                "{address} is not labelled {expected:?}"
            );
        }
    }

    #[track_caller]
    fn unlabelled(addresses: &[String]) {
        for address in addresses {
            assert_eq!(label(address), None, "{address} was given a label");
        }
    }

    #[test]
    fn a_pull_request_an_issue_and_a_discussion_are_repo_hash_number() {
        labels(&[
            (format!("{REPO}/pull/41"), "widgets#41"),
            (format!("{REPO}/issues/7"), "widgets#7"),
            (format!("{REPO}/discussions/3"), "widgets#3"),
            // The owner is dropped and the name kept as written.
            (
                "https://github.com/Some-Org/Gizmo.rs/pull/3871".to_owned(),
                "Gizmo.rs#3871",
            ),
        ]);
    }

    #[test]
    fn either_scheme_www_any_case_of_the_host_and_a_trailing_slash_change_nothing() {
        for address in [
            "https://github.com/octo/widgets/pull/41",
            "http://github.com/octo/widgets/pull/41",
            "https://www.github.com/octo/widgets/pull/41",
            "HTTPS://GitHub.COM/octo/widgets/pull/41",
            "Http://WWW.GITHUB.COM/octo/widgets/pull/41/",
            "https://github.com/octo/widgets/pull/41/#issuecomment-1",
        ] {
            assert!(
                label(address).is_some_and(|label| label.starts_with("widgets#41")),
                "{address}"
            );
        }
    }

    #[test]
    fn a_fragment_into_a_thread_says_what_it_points_at() {
        labels(&[
            (
                format!("{REPO}/pull/41#issuecomment-1234567"),
                "widgets#41 (comment)",
            ),
            (
                format!("{REPO}/issues/7#issuecomment-99"),
                "widgets#7 (comment)",
            ),
            (
                format!("{REPO}/pull/41#discussion_r1234567"),
                "widgets#41 (comment)",
            ),
            (
                format!("{REPO}/discussions/3#discussioncomment-55"),
                "widgets#3 (comment)",
            ),
            (
                format!("{REPO}/pull/41#pullrequestreview-777"),
                "widgets#41 (review)",
            ),
            // One this does not know is somewhere on the same page.
            (format!("{REPO}/pull/41#event-12"), "widgets#41"),
            (format!("{REPO}/issues/7#"), "widgets#7"),
            (format!("{REPO}/pull/41#issuecomment-x"), "widgets#41"),
        ]);
    }

    #[test]
    fn a_tab_of_a_pull_request_is_named_after_its_number() {
        labels(&[
            (format!("{REPO}/pull/41/files"), "widgets#41 (files)"),
            (
                format!("{REPO}/pull/41/files#diff-0a1b2c3d"),
                "widgets#41 (files)",
            ),
            (format!("{REPO}/pull/41/files?w=1"), "widgets#41 (files)"),
            (format!("{REPO}/pull/41/commits"), "widgets#41 (commits)"),
            (format!("{REPO}/pull/41/checks/"), "widgets#41 (checks)"),
        ]);
    }

    #[test]
    fn a_commit_is_repo_at_seven_hex_digits_however_it_is_reached() {
        labels(&[
            (format!("{REPO}/commit/{SHA}"), "widgets@4f21ab0"),
            (format!("{REPO}/commit/4f21ab0"), "widgets@4f21ab0"),
            (format!("{REPO}/commit/4F21AB0C9D"), "widgets@4f21ab0"),
            (
                format!("{REPO}/commit/{SHA}#diff-9f8e7d"),
                "widgets@4f21ab0",
            ),
            (format!("{REPO}/commit/{SHA}?diff=split"), "widgets@4f21ab0"),
            (
                format!("{REPO}/commit/{SHA}#commitcomment-31"),
                "widgets@4f21ab0 (comment)",
            ),
            // One commit seen from its pull request is the commit.
            (format!("{REPO}/pull/41/commits/{SHA}"), "widgets@4f21ab0"),
            (
                format!("{REPO}/pull/41/commits/{SHA}#r1"),
                "widgets@4f21ab0",
            ),
        ]);
    }

    #[test]
    fn a_comparison_cuts_only_its_commit_ids() {
        labels(&[
            (
                format!("{REPO}/compare/main...feature"),
                "widgets@main...feature",
            ),
            (format!("{REPO}/compare/v1.0...v1.1"), "widgets@v1.0...v1.1"),
            (format!("{REPO}/compare/v1.0..v1.1"), "widgets@v1.0..v1.1"),
            (
                format!("{REPO}/compare/{SHA}...0123456789abcdef"),
                "widgets@4f21ab0...0123456",
            ),
            (
                format!("{REPO}/compare/main...user/topic-branch"),
                "widgets@main...user/topic-branch",
            ),
            (
                format!("{REPO}/compare/main...other-fork:feature"),
                "widgets@main...other-fork:feature",
            ),
            // A tag of digits alone is a date or a build, not a commit, and is shown whole.
            (
                format!("{REPO}/compare/20261001...20261008"),
                "widgets@20261001...20261008",
            ),
        ]);
        unlabelled(&[
            format!("{REPO}/compare/feature"),
            format!("{REPO}/compare/main...a..b"),
            format!("{REPO}/compare/main...a b"),
            format!("{REPO}/compare/main...%2e%2e"),
            format!("{REPO}/compare/...feature"),
            format!("{REPO}/compare/main..."),
            format!("{REPO}/compare/main...bad owner:x"),
            format!("{REPO}/compare/main...:x"),
        ]);
    }

    #[test]
    fn a_release_and_a_branch_are_repo_at_their_name() {
        labels(&[
            (format!("{REPO}/releases/tag/v1.2.0"), "widgets@v1.2.0"),
            (
                format!("{REPO}/releases/tag/release/2026-10"),
                "widgets@release/2026-10",
            ),
            (format!("{REPO}/tree/main"), "widgets@main"),
            (format!("{REPO}/tree/v2.x/"), "widgets@v2.x"),
        ]);
        unlabelled(&[
            format!("{REPO}/releases/tag/v1%2F2"),
            format!("{REPO}/releases/tag"),
            format!("{REPO}/releases"),
            // A branch with a slash cannot be told from a directory under a branch.
            format!("{REPO}/tree/main/docs"),
            format!("{REPO}/tree/user/topic"),
            format!("{REPO}/tree"),
        ]);
    }

    #[test]
    fn a_run_of_actions_is_repo_run_and_its_number() {
        labels(&[
            (
                format!("{REPO}/actions/runs/12345678901"),
                "widgets run 12345678901",
            ),
            (
                format!("{REPO}/actions/runs/123456/job/789"),
                "widgets run 123456 (job)",
            ),
            (
                format!("{REPO}/actions/runs/123456/attempts/2"),
                "widgets run 123456 (attempt 2)",
            ),
            (
                format!("{REPO}/actions/runs/123456?pr=41#step:4:12"),
                "widgets run 123456",
            ),
        ]);
        unlabelled(&[
            format!("{REPO}/actions"),
            format!("{REPO}/actions/runs"),
            format!("{REPO}/actions/runs/latest"),
            format!("{REPO}/actions/runs/1/job"),
            format!("{REPO}/actions/workflows/ci.yml"),
            format!("{REPO}/runs/987654"),
        ]);
    }

    #[test]
    fn a_file_is_repo_colon_its_name_and_its_lines() {
        labels(&[
            (
                format!("{REPO}/blob/main/src/render.rs#L10"),
                "widgets:render.rs#L10",
            ),
            (
                format!("{REPO}/blob/{SHA}/src/render.rs#L10-L20"),
                "widgets:render.rs#L10-L20",
            ),
            (
                format!("{REPO}/blob/main/src/a.rs#L3C5-L4C1"),
                "widgets:a.rs#L3C5-L4C1",
            ),
            (format!("{REPO}/blob/main/README.md"), "widgets:README.md"),
            // A heading's fragment, or a ref with a slash, leaves the file's name as it is.
            (
                format!("{REPO}/blob/main/README.md#install"),
                "widgets:README.md",
            ),
            (
                format!("{REPO}/blob/user/topic/docs/guide.md?plain=1#L5"),
                "widgets:guide.md#L5",
            ),
            (format!("{REPO}/blob/main/c++/x.c++"), "widgets:x.c++"),
        ]);
        unlabelled(&[
            format!("{REPO}/blob/main"),
            format!("{REPO}/blob/main/docs/My%20Notes.md"),
            format!("{REPO}/blob/main/docs/caf\u{e9}.md"),
            format!("{REPO}/blob/main/docs/a\u{202e}dm.exe"),
            format!("{REPO}/blob/main/docs/guide.md?raw=true"),
        ]);
    }

    #[test]
    fn what_is_not_one_of_these_pages_is_left_as_written() {
        unlabelled(&[
            // GitHub, but no page with a short form.
            REPO.to_owned(),
            format!("{REPO}/"),
            format!("{REPO}/pulls"),
            format!("{REPO}/issues"),
            format!("{REPO}/wiki/Home"),
            format!("{REPO}/commits/main"),
            format!("{REPO}/pull/new/feature"),
            format!("{REPO}/pull/41.diff"),
            format!("{REPO}/pull/41/files/{SHA}"),
            format!("{REPO}/commit/abc12"),
            format!("{REPO}/commit/xyz1234"),
            format!("{REPO}/Pull/41"),
            format!("{REPO}/pull/41/extra/segments"),
            "https://github.com".to_owned(),
            "https://github.com/".to_owned(),
            "https://github.com/octo".to_owned(),
            "https://github.com/pulls".to_owned(),
            "https://github.com/octo/widgets//pull/41".to_owned(),
            // A query this does not know, wherever it is.
            format!("{REPO}/pull/41?notification_referrer_id=x"),
            format!("{REPO}/issues/7?ref=pull/41"),
            format!("{REPO}/pull/41?w=1&utm_source=x"),
            // Other hosts, however much they look like it.
            "https://example.com/octo/widgets/pull/41".to_owned(),
            "https://github.com.example.com/octo/widgets/pull/41".to_owned(),
            "https://notgithub.com/octo/widgets/pull/41".to_owned(),
            "https://api.github.com/repos/octo/widgets/pulls/41".to_owned(),
            "https://gist.github.com/octo/0123456789abcdef".to_owned(),
            "https://github.com:443/octo/widgets/pull/41".to_owned(),
            "https://user@github.com/octo/widgets/pull/41".to_owned(),
            "https://gitlab.example.com/octo/widgets/-/merge_requests/4".to_owned(),
            // Not a web address at all.
            "ftp://github.com/octo/widgets/pull/41".to_owned(),
            "github.com/octo/widgets/pull/41".to_owned(),
            "mailto:octo@github.com".to_owned(),
            String::new(),
        ]);
    }

    #[test]
    fn an_owner_or_a_name_with_characters_neither_may_hold_is_left_as_written() {
        unlabelled(&[
            "https://github.com/oc%74o/widgets/pull/41".to_owned(),
            "https://github.com/octo/wid%67ets/pull/41".to_owned(),
            "https://github.com/octo/wid gets/pull/41".to_owned(),
            "https://github.com/octo/wid<b>gets/pull/41".to_owned(),
            "https://github.com/octo/widg\u{202e}ets/pull/41".to_owned(),
            "https://github.com/octo/../pull/41".to_owned(),
            "https://github.com/octo/./pull/41".to_owned(),
            format!("https://github.com/octo/{}/pull/41", "w".repeat(101)),
            "https://github.com/octo/widgets/pull/4%31".to_owned(),
            "https://github.com/octo/widgets/pull/\u{661}".to_owned(),
            format!("https://github.com/octo/widgets/pull/{}", "9".repeat(21)),
        ]);
        // Whatever an address holds, a label is ASCII letters, digits and a few signs.
        let label = label("https://github.com/octo/widgets/pull/41").expect("labelled");
        assert!(label.bytes().all(|b| b.is_ascii_graphic() || b == b' '));
    }

    #[test]
    fn an_address_past_the_bound_is_not_read() {
        let long = format!("{REPO}/blob/main/{}/x.rs", "d/".repeat(MAX_ADDRESS));
        assert_eq!(label(&long), None);
        let fits = format!("{REPO}/blob/main/{}x.rs", "d/".repeat(100));
        assert!(fits.len() <= MAX_ADDRESS);
        assert_eq!(label(&fits).as_deref(), Some("widgets:x.rs"));
    }

    #[test]
    fn the_scheme_comes_off_in_any_case_and_nothing_else_does() {
        assert_eq!(strip_scheme("https://a.test/x"), Some("a.test/x"));
        assert_eq!(strip_scheme("HTTP://a.test"), Some("a.test"));
        assert_eq!(strip_scheme("www.a.test"), None);
        assert_eq!(strip_scheme("ftp://a.test"), None);
        assert_eq!(strip_scheme("http:/a.test"), None);
        assert_eq!(strip_scheme("h\u{e9}"), None);
    }
}
