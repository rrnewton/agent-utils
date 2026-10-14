//! `#302 root-is-the-app`: the app is the page at `/`, the older page that used to be there is
//! gone, and the app's old address `/voice` forwards to `/` keeping its query string.
//!
//! Through the whole router, so the redirect is checked as a browser receives it: after the bearer
//! check, the access log and the compression layer have all had their turn.
//!
//! What only a browser can show — that the fragment survives the redirect, and that Chromium still
//! finds the page installable and in scope from either address — is `tests/pwa_installability.py`.
//!
//! The last four tests read files instead of the router. Three read the guides: they tell a reader
//! how to open and sign in to the page at `/`, so they must describe the app and not the page that
//! is gone. The last reads the whole tree, so that every reference to this change names its issue
//! by the number main gave it.

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;
use vibe_talk::http::router;

fn app() -> axum::Router {
    let (state, _discord, _voice) = vibe_talk::testing::state_parts();
    router(state)
}

/// `method uri`, and the whole answer.
async fn fetch(app: &axum::Router, method: &str, uri: &str) -> (StatusCode, HeaderMap, Vec<u8>) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("accept-encoding", "identity")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("router responds");
    let status = response.status();
    let headers = response.headers().clone();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes()
        .to_vec();
    (status, headers, body)
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> &'a str {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
}

async fn root_page(app: &axum::Router) -> String {
    let (status, headers, body) = fetch(app, "GET", "/").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(header(&headers, "content-type"), "text/html; charset=utf-8");
    String::from_utf8(body).expect("utf-8")
}

#[tokio::test]
async fn the_root_is_the_app() {
    let app = app();
    let page = root_page(&app).await;
    // The app's own frame and the files only it loads, each by its content-addressed URL.
    for marker in [
        "<title>vibe-talk</title>",
        "id=\"topbar\"",
        "id=\"dock\"",
        "id=\"discord-log\"",
        "viewport-fit=cover",
        "<link rel=\"manifest\" href=\"/manifest.webmanifest\" />",
        "src=\"/contract.js?v=",
        "src=\"/voice.js?v=",
        "href=\"/voice.css?v=",
        "href=\"/style.css?v=",
    ] {
        assert!(page.contains(marker), "/ does not carry {marker:?}");
    }
}

#[tokio::test]
async fn nothing_of_the_old_page_is_served() {
    // The older page that `/` served until this change: three tabs (Talk, Text, Settings), a
    // digest list, a find form, a scrollback list, the vendor's hosted widget loaded from a CDN,
    // and its own script. None of it may come back by way of the shell or a route.
    let app = app();
    let page = root_page(&app).await;
    for marker in [
        "/app.js",
        "role=\"tablist\"",
        "data-tab=",
        "id=\"tab-talk\"",
        "id=\"tab-text\"",
        "id=\"refresh-digest\"",
        "id=\"find-form\"",
        "id=\"scrollback\"",
        "Catch me up",
        "Read the digest aloud",
        "the signed-URL voice page",
        "elevenlabs-convai",
        "unpkg.com",
    ] {
        assert!(
            !page.contains(marker),
            "/ still carries {marker:?} from the old page"
        );
    }
    // Its script is gone by every spelling a cached copy of the old shell could have used.
    for gone in [
        "/app.js",
        "/app.js?v=0123456789abcdef0123456789abcdef",
        "/index.html",
    ] {
        let (status, _, _) = fetch(&app, "GET", gone).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{gone}");
    }
}

#[tokio::test]
async fn the_old_address_forwards_to_the_root_keeping_its_query() {
    let app = app();
    let page = root_page(&app).await;
    for (asked, location) in [
        ("/voice", "/"),
        ("/voice?", "/"),
        ("/voice?x=1", "/?x=1"),
        ("/voice/", "/"),
        ("/voice/?x=1", "/?x=1"),
        // Exactly as sent, still percent-encoded: decoding could only change it.
        ("/voice?q=a%20b&r=%E2%9C%93&s", "/?q=a%20b&r=%E2%9C%93&s"),
    ] {
        for method in ["GET", "HEAD"] {
            let (status, headers, _) = fetch(&app, method, asked).await;
            // 302, not 301: see "Why 302 and not 301" in `src/http/assets.rs`. A permanent
            // redirect would be kept by every browser that saw it, past a rollback and past the
            // day `/voice` stops forwarding.
            assert_eq!(status, StatusCode::FOUND, "{method} {asked}");
            assert_eq!(header(&headers, "location"), location, "{method} {asked}");
            assert_eq!(
                header(&headers, "cache-control"),
                "no-cache",
                "{method} {asked}: a forward must be asked for again, never reused"
            );
        }
        // Followed, it is the app.
        let (status, _, body) = fetch(&app, "GET", location).await;
        assert_eq!(status, StatusCode::OK, "{location}");
        assert!(
            body == page.as_bytes(),
            "{asked} forwards to {location}, which is not the app at /"
        );
    }
    // Nothing was ever served below `/voice/`, so nothing there is guessed at.
    let (status, _, _) = fetch(&app, "GET", "/voice/settings").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_manifest_starts_at_the_root_and_keeps_the_installed_apps_identity() {
    let app = app();
    let (status, _, body) = fetch(&app, "GET", "/manifest.webmanifest").await;
    assert_eq!(status, StatusCode::OK);
    let manifest: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
    assert_eq!(
        manifest["start_url"], "/",
        "a new installation opens the app at /"
    );
    assert_eq!(
        manifest["scope"], "/",
        "both addresses must stay inside the scope"
    );
    // The identity of every copy installed while the app lived at `/voice`. Chromium matches an
    // installed app to a manifest by `id`, so changing it would make this a different app and
    // strand those copies on the old `start_url` for good. See `src/http/assets.rs`.
    assert_eq!(manifest["id"], "/voice");
}

const README: &str = include_str!("../README.md");

/// The text between `start` and the next `end` in `haystack`, or a failure naming what was missing.
fn between<'a>(haystack: &'a str, start: &str, end: &str, what: &str) -> &'a str {
    let from = haystack
        .find(start)
        .unwrap_or_else(|| panic!("{what}: {start:?} not found"))
        + start.len();
    let to = haystack[from..]
        .find(end)
        .unwrap_or_else(|| panic!("{what}: no {end:?} after {start:?}"));
    &haystack[from..from + to]
}

#[test]
fn the_readme_signs_in_with_the_apps_own_controls() {
    // The walkthrough's first sign-in once sent the reader to the old page's Settings tab and its
    // "vibe-talk API token" field. It must name the controls the app's sign-in screen has, read
    // out of the shell rather than copied, so relabelling either side alone fails here.
    let shell = include_str!("../web/voice.html");
    let field = between(
        shell,
        "<label for=\"api-token\">",
        "</label>",
        "the sign-in field's label",
    );
    let button = between(
        shell,
        "<button id=\"save-token\" type=\"button\">",
        "</button>",
        "the sign-in button",
    );
    let signin = README
        .split("\n\n")
        .find(|paragraph| paragraph.starts_with("Then open `http://<host>:8080/`"))
        .expect("the README's first sign-in paragraph");
    for control in [field, button] {
        assert!(
            signin.contains(&format!("**{control}**")),
            "the README's first sign-in does not name the app's {control:?}:\n{signin}"
        );
    }
}

#[test]
fn the_readme_describes_nothing_of_the_old_page() {
    // Features and controls only the deleted page had: its Text tab, its find form, its token
    // field's label, its Talk tab's buttons and the route to its Settings tab. The app has none of
    // them, so a guide naming one is describing a page that is not there.
    let readme = README.to_lowercase();
    for marker in [
        "text tab",
        "find-a-message",
        "vibe-talk api token",
        "go to **settings**, paste",
        "catch me up",
        "read the digest aloud",
    ] {
        assert!(
            !readme.contains(marker),
            "the README still describes the old page: {marker:?}"
        );
    }
}

/// Each URL with a host on `line` that names the old address `/voice`, with or without a trailing
/// slash, query or fragment. A URL runs from its authority to the first character that cannot be
/// part of a pasted URL, without trailing punctuation.
fn old_addresses(line: &str) -> Vec<&str> {
    line.match_indices("://")
        .map(|(at, separator)| {
            let rest = &line[at + separator.len()..];
            let end = rest
                .find(|c: char| c.is_whitespace() || "`\"'()[]".contains(c))
                .unwrap_or(rest.len());
            rest[..end].trim_end_matches(['>', '.', ',', ';', ':'])
        })
        .filter(|url| {
            let path = url.find('/').map_or("", |slash| &url[slash..]);
            let path = path.split(['?', '#']).next().unwrap_or_default();
            path.trim_end_matches('/') == "/voice"
        })
        .collect()
}

#[test]
fn no_guide_links_to_the_old_address() {
    // `/voice` only forwards to the app now. An example or a link that names it still works, but
    // it is the old address taught as the current one; the README says what `/voice` does in
    // prose, with no host, and that is the only place it belongs.
    let guides = [
        ("README.md", README),
        ("QUICKSTART.md", include_str!("../QUICKSTART.md")),
        (
            "vibe-talk.example.toml",
            include_str!("../vibe-talk.example.toml"),
        ),
        (
            "vibe-talk.env.example",
            include_str!("../vibe-talk.env.example"),
        ),
    ];
    let mut old = Vec::new();
    for (name, text) in guides {
        for (index, line) in text.lines().enumerate() {
            for url in old_addresses(line) {
                old.push(format!("{name}:{}: {url}", index + 1));
            }
        }
    }
    assert!(
        old.is_empty(),
        "links to the old address:\n{}",
        old.join("\n")
    );
    // Not vacuous: the scan finds the address in the shapes a guide writes it, and only there.
    for (line, found) in [
        ("  --url https://vibe-talk.example.invalid/voice \\", 1),
        ("open <https://h.example/voice?x=1#y>.", 1),
        ("see [the app](http://<host>:8080/voice/)", 1),
        (
            "open `http://<host>:8080/`, or https://h.example/voice.js",
            0,
        ),
        ("`/voice` forwards to `/`", 0),
    ] {
        assert_eq!(old_addresses(line).len(), found, "{line}");
    }
}

/// This change's slug, and the number main gave its issue. The work began on a branch whose own
/// issue file took the next free number; main then reserved this one for it and gave the branch's
/// number to an unrelated issue. A reference that kept the branch's number would land pointing a
/// reader at the wrong issue.
const SLUG: &str = "root-is-the-app";
const NUMBER: &str = "302";

/// The number of each `#<number> <SLUG>` reference in `text`, in order, a reference wrapped so the
/// number ends one line and the slug starts the next included, with or without a comment leader on
/// the second line.
fn issue_numbers(text: &str) -> Vec<&str> {
    text.match_indices(SLUG)
        .filter(|(at, _)| {
            // The slug itself, not the start of a longer one.
            !text[at + SLUG.len()..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '-')
        })
        .filter_map(|(at, _)| {
            let mut head = text[..at].trim_end_matches([' ', '\t']);
            if let Some(newline) = head.rfind('\n') {
                if head[newline + 1..]
                    .trim_start()
                    .chars()
                    .all(|c| "/*#".contains(c))
                {
                    head = head[..newline].trim_end();
                }
            }
            let number = head.trim_end_matches(|c: char| c.is_ascii_digit());
            let digits = &head[number.len()..];
            (!digits.is_empty() && number.ends_with('#')).then_some(digits)
        })
        .collect()
}

/// Every file under `dir` that is not build output, a local artifact or a dependency, as a path
/// relative to `root` and its text; files that are not UTF-8 are not prose and are skipped.
fn texts(root: &std::path::Path, dir: &std::path::Path, found: &mut Vec<(String, String)>) {
    for entry in std::fs::read_dir(dir).expect("readable directory") {
        let path = entry.expect("directory entry").path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if name.starts_with('.')
            || ["target", "debug", "node_modules", "__pycache__"].contains(&name)
        {
            continue;
        }
        if path.is_dir() {
            texts(root, &path, found);
        } else if let Ok(text) = std::fs::read_to_string(&path) {
            let relative = path.strip_prefix(root).expect("under the root");
            found.push((relative.display().to_string(), text));
        }
    }
}

#[test]
fn every_reference_to_this_change_names_its_issue_by_mains_number() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    texts(root, root, &mut files);
    let mut wrong = Vec::new();
    let mut referring = Vec::new();
    for (name, text) in &files {
        let numbers = issue_numbers(text);
        if !numbers.is_empty() {
            referring.push(name.as_str());
        }
        for number in numbers.into_iter().filter(|number| *number != NUMBER) {
            wrong.push(format!("{name}: #{number} {SLUG}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "references to {SLUG} that do not name #{NUMBER}:\n{}",
        wrong.join("\n")
    );
    // Not vacuous: the walk reaches the server, the page and its stylesheet, the tests, a script
    // and the guides. The page's script has one reference, wrapped across two comment lines, so
    // finding it there shows a wrapped reference is read.
    for expected in [
        "src/http/assets.rs",
        "web/voice.js",
        "web/style.css",
        "tests/scope_first.rs",
        "scripts/energy_profile.py",
        "README.md",
        "Makefile",
        "tests.dag.yaml",
    ] {
        assert!(
            referring.contains(&expected),
            "the walk found no reference to {SLUG} in {expected}"
        );
    }
    // The shapes a reference is written in, built here so this file holds no reference but its own.
    for (text, numbers) in [
        (format!("`#7 {SLUG}`"), vec!["7"]),
        (format!("(gone since `#7\n// {SLUG}`)"), vec!["7"]),
        (format!("    # since #7\n    # {SLUG}: a"), vec!["7"]),
        (format!(" * until `#7\n * {SLUG}`; the"), vec!["7"]),
        (format!("until `#7\n{SLUG}`, while"), vec!["7"]),
        (format!("#7 {SLUG} and #8 {SLUG}"), vec!["7", "8"]),
        (format!("vt/{SLUG}"), vec![]),
        (format!("#7 {SLUG}-later"), vec![]),
        (format!("issue 7 {SLUG}"), vec![]),
    ] {
        assert_eq!(issue_numbers(&text), numbers, "{text:?}");
    }
}
