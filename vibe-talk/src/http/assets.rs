//! The web app's static files, what a browser is told it may keep, and where its old address goes.
//!
//! `#223 asset-caching`. Every one of these used to be answered `Cache-Control: no-store`, so each
//! launch of the installed app downloaded the whole page again — the page script alone is 262 KB
//! on the wire after compression. Now:
//!
//! * **Scripts, stylesheets and icons are content-addressed.** Each file's URL carries the
//!   SHA-256 of its bytes (128 bits of it, in hex), `/voice.js?v=<hash>`, and under that URL it is
//!   answered `public, max-age=31536000, immutable`: a browser never asks for it again, and does
//!   not need to, because different bytes would have a different URL.
//! * **The HTML shell, the app at `/`, is generated, not edited.** At startup the page's `src="…"`
//!   and `href="…"` references to those files are rewritten to the hashed URLs, so no hash is ever
//!   written into a file by hand and none can go stale.
//! * **The shell and the install manifest are revalidated on every load**: `no-cache`, with an
//!   ETag that is the hash of the generated body, and `If-None-Match` answered `304`. A deploy
//!   that changed any file changes the shell that names it, so the next launch gets the new shell
//!   in one small request and then downloads only the files whose hashes moved.
//! * **Old URLs keep working.** An app installed earlier, or a shell already in a cache, asks for
//!   the plain `/voice.js`. That URL is still served — the same current bytes, `no-cache` with the
//!   same ETag — and never 404s. So is a hashed URL whose hash is not the current one, which a page
//!   left open across a deploy can ask for: it gets the current bytes, revalidated, and is never
//!   told they are immutable, because a year-long entry under a URL that does not name its bytes
//!   would outlive a rollback to the version it does name.
//!
//! # Why `?v=<hash>` rather than `/assets/<hash>/voice.js`
//!
//! Every HTTP cache keys a response on its whole target URI, query included (RFC 9111 §2), so the
//! query is exactly as content-addressed as a path segment would be. What the query buys is that
//! the hashed and the plain URL are ONE route with one handler: "old URLs keep working" is then a
//! property of the route rather than a second table to keep in step, and no route was added — the
//! router's paths and methods, and the inventory `tests/scope_first.rs` keeps of them, are what
//! they were. The old objection — that some shared proxies decline to cache a URL with a query —
//! is about intermediaries; the cache a launch depends on is the browser's own, which has no such
//! rule.
//!
//! # Why the ETag is weak
//!
//! [`super::router`] compresses OUTSIDE this handler: one body goes out as Brotli, gzip or
//! identity depending on who asked, and the compression layer leaves `ETag` alone. A strong
//! validator promises byte-identical representations, so one strong tag on all three would be a
//! false promise (nginx's gzip filter weakens ETags for the same reason). A weak tag promises
//! semantic equivalence, which is true of the three, and `If-None-Match` uses the weak comparison
//! anyway (RFC 9110 §13.1.2), so revalidation answers `304` whichever encoding the cached copy was
//! in. The one thing a weak tag cannot validate is a byte range, and nothing here is fetched by
//! range.
//!
//! `Vary: Accept-Encoding` is set HERE, on the `200` and the `304` alike. The compression layer
//! adds it only to a body it considers compressing, a `304` has no body, and RFC 9110 §15.4.5
//! requires the `304` to carry the `Vary` the `200` would have.
//!
//! # Why there is still no service worker
//!
//! HTTP caching is enough for launch speed. Launching the installed app is a navigation to the
//! manifest's `start_url`, `/`: the browser revalidates the shell (a `304` of a few hundred
//! bytes when nothing was deployed) and takes every file it names from its own HTTP cache without
//! asking. A service worker could remove that one round trip and let the app open with no network
//! at all, and it is deliberately not used: CacheStorage outlives sign-out and is served by
//! whatever the worker decides (see "the offline message cache" in `web/voice.js`), and a worker
//! brings an update lifecycle in which a deploy is seen one launch late. The HTTP cache holds only
//! these public files. Every `/api/` and `/mcp` answer stays `no-store`, which
//! [`super::access_layer`] enforces at the edge and `tests/scope_first.rs` checks route by route.
//!
//! # The manifest names its icons by their plain URLs
//!
//! On purpose. The manifest is the installed app's identity, read by the browser's installer and
//! by its periodic update check rather than on each launch, and an icon URL that changed with
//! every edit to the artwork would make each such edit an identity change for every installed
//! copy. The plain icon URLs revalidate like any other plain URL.
//!
//! # `/` is the app, and `/voice` forwards to it
//!
//! `#302 root-is-the-app`. The app used to live at `/voice`, while `/` served an older, smaller
//! page that duplicated part of it. That page is gone and the app is served at `/`. `/voice` and
//! `/voice/` answer [`forward_to_root`]: a `302 Found` to `/`, with the request's query string
//! kept. The fragment needs no help: a browser never sends it, and RFC 9110 §10.2.2 has it carry
//! the original fragment over to a `Location` that has none, so `/voice?x=1#y` lands on
//! `/?x=1#y`. The page reads nothing from its path, query or fragment, so those two spellings are
//! every link to it there ever was; nothing was ever served below `/voice/`.
//!
//! **Why 302 and not 301.** Both keep a bookmark and an installed app working; what differs is
//! how long a browser remembers the answer. A 301 is cacheable by default (RFC 9111 §4.2.2), and
//! browsers keep it indefinitely, so it would outlive the deploy that made it: after a rollback to
//! a version where `/voice` is the app, every browser that had seen the 301 would go on skipping
//! `/voice`. And the pointer is meant to be temporary, which a permanent redirect held in caches
//! nobody can reach would pre-empt. A 302 sent `no-cache` is asked for again on every use, so the
//! server stays the only authority on where `/voice` goes. The cost is one small round trip per
//! launch for an app installed before this change, until the browser's manifest update moves its
//! `start_url` to `/`.
//!
//! **The installed app keeps its identity.** The manifest's `id` stays `/voice` while its
//! `start_url` becomes `/`. Chromium identifies an installed web app by that `id`, resolved against
//! the origin, and falls back to `start_url` only when there is none; so the manifest served now is
//! an UPDATE of the app installed from `/voice`, not a second app, and an installation made before
//! this change takes the new `start_url` at its next update check. Until then it launches `/voice`,
//! is forwarded to `/`, and stays in the app window, because the manifest's `scope` is `/` and a
//! same-origin navigation inside the scope never leaves it. `tests/pwa_installability.py` asks
//! Chromium itself for each of these against the real server.
//!
//! # The app is called what the deployment calls it
//!
//! `#343 pwa-app-name`. Two deployments installed on one phone are two apps — each origin is its
//! own — and both used to be called `vibe-talk`, in the launcher, the app switcher and the tab. So
//! the manifest's `name` and `short_name`, the page's `<title>`, its sign-in heading and its
//! `apple-mobile-web-app-title` are the configuration's `[server] app_name` and `short_name`
//! ([`crate::config::Config::app_name`]). Nothing else in the manifest moves: its `id`, `start_url`
//! and `scope` are the installed app's identity, and renaming an app must not make it another one.
//!
//! The files on disk still say `vibe-talk`, and the server writes the deployment's names over
//! exactly those spellings ([`name_shell`], [`name_manifest`]); a unit test holds that each occurs
//! once. So with the default name the manifest is served byte for byte as written, and its ETag
//! is the one it always had. The name is escaped for where it lands — HTML text and attribute in
//! the shell, a JSON string in the manifest — after the file references are linked, so a name can
//! never be mistaken for one.
//!
//! The documents are built once per pair of names, on first use: a deployment has one pair, so
//! once per process, as before; a test process holding several configurations builds one each.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, PoisonError};

use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;

use super::api;
use crate::config::DEFAULT_APP_NAME;
use crate::state::AppState;

/// The app's page, served at `/`. Its files keep the `voice` names they had when it lived at
/// `/voice`, so a shell cached before the move still names files that exist.
const VOICE_HTML: &str = include_str!("../../web/voice.html");
/// The app's script.
pub(crate) const VOICE_JS: &str = include_str!("../../web/voice.js");
const CONTRACT_JS: &str = include_str!("../../web/contract.js");
const VOICE_CSS: &str = include_str!("../../web/voice.css");
const STYLE_CSS: &str = include_str!("../../web/style.css");
const MANIFEST: &str = include_str!("../../web/manifest.webmanifest");

const HTML: &str = "text/html; charset=utf-8";
const JAVASCRIPT: &str = "text/javascript; charset=utf-8";
const CSS: &str = "text/css; charset=utf-8";

/// A content-addressed URL: this exact name can only ever mean these exact bytes.
const IMMUTABLE: &str = "public, max-age=31536000, immutable";
/// Keep it, but ask before every use; the ETag makes the asking cheap.
const REVALIDATE: &str = "no-cache";

/// How a path is cached, and whether its body is rewritten.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    /// A script, stylesheet or icon. Named by the shells with its content hash, and immutable
    /// under that name.
    File,
    /// An HTML page. Its references to files are rewritten to their hashed URLs at startup, so is
    /// the app's name ([`name_shell`]), and it is revalidated on every load.
    Shell,
    /// The install manifest: revalidated like a shell, served as written but for the app's name
    /// ([`name_manifest`]).
    Manifest,
}

/// One embedded file, by the path the router serves it at.
struct Source {
    path: &'static str,
    content_type: &'static str,
    body: &'static [u8],
    role: Role,
}

const fn source(
    path: &'static str,
    content_type: &'static str,
    body: &'static [u8],
    role: Role,
) -> Source {
    Source {
        path,
        content_type,
        body,
        role,
    }
}

/// Everything static the router serves. A path here needs a route in [`super::router`] too; a
/// route without an entry here answers 404.
const SOURCES: &[Source] = &[
    source("/", HTML, VOICE_HTML.as_bytes(), Role::Shell),
    source(
        "/manifest.webmanifest",
        "application/manifest+json; charset=utf-8",
        MANIFEST.as_bytes(),
        Role::Manifest,
    ),
    source("/voice.js", JAVASCRIPT, VOICE_JS.as_bytes(), Role::File),
    // The generated wire-contract validators the page loads before its script.
    source(
        "/contract.js",
        JAVASCRIPT,
        CONTRACT_JS.as_bytes(),
        Role::File,
    ),
    // The page's two stylesheets: `style.css` its colours and components, `voice.css` the fixed
    // application frame (`100dvh`, no page scroll) and everything else on it.
    source("/voice.css", CSS, VOICE_CSS.as_bytes(), Role::File),
    source("/style.css", CSS, STYLE_CSS.as_bytes(), Role::File),
    // Embedded artwork, available before the app is authenticated.
    source(
        "/icons/icon.svg",
        "image/svg+xml",
        include_bytes!("../../web/icons/icon.svg"),
        Role::File,
    ),
    source(
        "/icons/icon-192.png",
        "image/png",
        include_bytes!("../../web/icons/icon-192.png"),
        Role::File,
    ),
    source(
        "/icons/icon-512.png",
        "image/png",
        include_bytes!("../../web/icons/icon-512.png"),
        Role::File,
    ),
    source(
        "/icons/maskable-512.png",
        "image/png",
        include_bytes!("../../web/icons/maskable-512.png"),
        Role::File,
    ),
];

/// One path, ready to answer.
struct Entry {
    content_type: &'static str,
    body: Bytes,
    /// The content hash a [`Role::File`] is named by. `None` for a shell or the manifest, which
    /// keep their plain URLs and are revalidated instead.
    version: Option<String>,
    etag: HeaderValue,
}

/// Every static path and its answer, computed once from [`SOURCES`] for one pair of names.
struct Site {
    names: Names,
    entries: BTreeMap<&'static str, Entry>,
}

/// What the installed app is called: `[server] app_name` and `short_name`. `#343 pwa-app-name`.
#[derive(Debug, PartialEq, Eq)]
struct Names {
    /// The manifest's `name`, the page's title and its sign-in heading.
    name: String,
    /// The manifest's `short_name` and the page's `apple-mobile-web-app-title`: the label under
    /// the icon, on Android and on iOS alike.
    short_name: String,
}

impl Default for Names {
    /// The names the files are written with.
    fn default() -> Self {
        Self {
            name: DEFAULT_APP_NAME.to_owned(),
            short_name: DEFAULT_APP_NAME.to_owned(),
        }
    }
}

/// Every [`Site`] built so far, one per pair of names; see the module documentation. A short list
/// searched in place rather than a map, so asking for the one a deployment has allocates nothing.
static SITES: Mutex<Vec<Arc<Site>>> = Mutex::new(Vec::new());

/// The site whose documents carry `name` and `short_name`, built the first time they are asked for.
///
/// Built under the lock, so two first requests build it once. The bytes are compiled in and the
/// names come from configuration that does not change while the process runs, so a site is never
/// stale.
fn site(name: &str, short_name: &str) -> Arc<Site> {
    let mut sites = SITES.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(site) = sites
        .iter()
        .find(|site| site.names.name == name && site.names.short_name == short_name)
    {
        return Arc::clone(site);
    }
    let names = Names {
        name: name.to_owned(),
        short_name: short_name.to_owned(),
    };
    let site = Arc::new(Site::new(SOURCES, names));
    sites.push(Arc::clone(&site));
    site
}

/// `GET` any static path: the shell, the manifest, the scripts and stylesheets, and the icons.
///
/// See the module documentation for what each is told about caching, and why, and for where the
/// app's name comes from.
pub async fn serve(State(state): State<AppState>, uri: Uri, headers: HeaderMap) -> Response {
    let site = site(&state.config.app_name, &state.config.app_short_name);
    match site.answer(&uri, &headers) {
        Some(response) => response,
        None => api::not_found().await.into_response(),
    }
}

/// `GET /voice`: the app's old address, forwarded to `/` with its query string.
///
/// A `302`, revalidated, rather than a `301`; see "`/` is the app" in the module documentation for
/// why, and for how the fragment survives without the server ever seeing it.
pub async fn forward_to_root(uri: Uri) -> Response {
    (
        StatusCode::FOUND,
        [
            (header::LOCATION, root_keeping_query(&uri)),
            (header::CACHE_CONTROL, HeaderValue::from_static(REVALIDATE)),
        ],
    )
        .into_response()
}

/// `/`, with `uri`'s query string when it has a non-empty one.
///
/// The query is copied exactly as it arrived, still percent-encoded: it is already a valid URI
/// query, so it is already a valid header value, and decoding it here could only change it.
fn root_keeping_query(uri: &Uri) -> HeaderValue {
    match uri.query().filter(|query| !query.is_empty()) {
        Some(query) => HeaderValue::try_from(format!("/?{query}"))
            .unwrap_or_else(|_| HeaderValue::from_static("/")),
        None => HeaderValue::from_static("/"),
    }
}

impl Site {
    fn new(sources: &[Source], names: Names) -> Self {
        // Every file's hash first: a shell cannot be generated until each file it may name has one.
        let versions: BTreeMap<&'static str, String> = sources
            .iter()
            .filter(|source| source.role == Role::File)
            .map(|file| (file.path, digest(file.body)))
            .collect();
        let entries = sources
            .iter()
            .map(|source| {
                let (body, version) = match source.role {
                    Role::File => (
                        Bytes::from_static(source.body),
                        versions.get(source.path).cloned(),
                    ),
                    // Linked first, then named, so a name can never be read as a file reference.
                    Role::Shell => (
                        Bytes::from(name_shell(
                            &link(&String::from_utf8_lossy(source.body), &versions),
                            &names,
                        )),
                        None,
                    ),
                    Role::Manifest => (
                        Bytes::from(name_manifest(&String::from_utf8_lossy(source.body), &names)),
                        None,
                    ),
                };
                (source.path, Entry::new(source.content_type, body, version))
            })
            .collect();
        Self { names, entries }
    }

    /// The answer for `uri`, or `None` when no static file lives at its path.
    fn answer(&self, uri: &Uri, headers: &HeaderMap) -> Option<Response> {
        let entry = self.entries.get(uri.path())?;
        // Immutable only under EXACTLY the URL the current shell names. No query, an older hash, or
        // anything added to it gets the same bytes revalidated instead; see the module comment.
        let named = entry
            .version
            .as_deref()
            .is_some_and(|version| uri.query() == Some(format!("v={version}").as_str()));
        let cache_control = HeaderValue::from_static(if named { IMMUTABLE } else { REVALIDATE });
        let validators = [
            (header::ETAG, entry.etag.clone()),
            (header::CACHE_CONTROL, cache_control),
            (header::VARY, HeaderValue::from_static("accept-encoding")),
        ];
        if already_held(headers, &entry.etag) {
            return Some((StatusCode::NOT_MODIFIED, validators).into_response());
        }
        Some(
            (
                validators,
                [(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static(entry.content_type),
                )],
                entry.body.clone(),
            )
                .into_response(),
        )
    }
}

impl Entry {
    fn new(content_type: &'static str, body: Bytes, version: Option<String>) -> Self {
        // A shell's tag is the hash of the body as GENERATED, so it moves whenever any file the
        // shell names does. A file's is its own content hash, the same one its URL carries.
        let tag = version.clone().unwrap_or_else(|| digest(&body));
        let etag = HeaderValue::try_from(format!("W/\"{tag}\""))
            .expect("hex digits in quotes are a valid header value");
        Self {
            content_type,
            body,
            version,
            etag,
        }
    }
}

/// Rewrite a page's `src="/x"` and `href="/x"` references to the hashed URL of `/x`.
///
/// Only those exact attribute forms, quoted, are touched: a quoted path elsewhere in the page —
/// prose, an inline script — is not a reference, and a bare `/a` link is not `/a.js`.
fn link(template: &str, versions: &BTreeMap<&'static str, String>) -> String {
    let mut page = template.to_owned();
    for (path, version) in versions {
        for attribute in ["src", "href"] {
            page = page.replace(
                &format!("{attribute}=\"{path}\""),
                &format!("{attribute}=\"{path}?v={version}\""),
            );
        }
    }
    page
}

/// The shell's spellings of the app's name, as `web/voice.html` writes them: the title, the sign-in
/// heading, and the iOS home-screen label. Each is written with the default name, and each occurs
/// exactly once (a unit test holds both), so a reworded page fails that test rather than quietly
/// going on calling every deployment `vibe-talk`. `#343 pwa-app-name`.
fn shell_spellings(names: &Names) -> [(String, String); 3] {
    let (name, short_name) = (html_escape(&names.name), html_escape(&names.short_name));
    [
        (
            format!("<title>{DEFAULT_APP_NAME}</title>"),
            format!("<title>{name}</title>"),
        ),
        (
            format!("<h1>{DEFAULT_APP_NAME}</h1>"),
            format!("<h1>{name}</h1>"),
        ),
        (
            format!(r#"<meta name="apple-mobile-web-app-title" content="{DEFAULT_APP_NAME}" />"#),
            format!(r#"<meta name="apple-mobile-web-app-title" content="{short_name}" />"#),
        ),
    ]
}

/// The manifest's two names, as `web/manifest.webmanifest` writes them, and what they become.
///
/// `"name": ` cannot match inside `"short_name": `, whose `name` follows an underscore rather than
/// a quote, and a replacement cannot match either needle, because a JSON string escapes its quotes.
fn manifest_spellings(names: &Names) -> [(String, String); 2] {
    let json = |value: &str| serde_json::Value::from(value).to_string();
    [
        (
            format!(r#""name": {}"#, json(DEFAULT_APP_NAME)),
            format!(r#""name": {}"#, json(&names.name)),
        ),
        (
            format!(r#""short_name": {}"#, json(DEFAULT_APP_NAME)),
            format!(r#""short_name": {}"#, json(&names.short_name)),
        ),
    ]
}

/// The shell, calling the app what this deployment calls it. See [`shell_spellings`].
///
/// HTML-escaped, so a replacement holds no `<` or `"` and cannot match a later needle.
fn name_shell(page: &str, names: &Names) -> String {
    respell(page, &shell_spellings(names))
}

/// The manifest, with this deployment's `name` and `short_name` and every other byte as written.
/// See [`manifest_spellings`]. With the default names it is the file itself.
fn name_manifest(manifest: &str, names: &Names) -> String {
    respell(manifest, &manifest_spellings(names))
}

fn respell(text: &str, spellings: &[(String, String)]) -> String {
    spellings
        .iter()
        .fold(text.to_owned(), |text, (written, served)| {
            text.replacen(written.as_str(), served, 1)
        })
}

/// `text` escaped for HTML text and for a double- or single-quoted attribute value alike.
fn html_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            other => escaped.push(other),
        }
    }
    escaped
}

/// Whether the client already holds the representation tagged `etag`.
///
/// RFC 9110 §13.1.2: `If-None-Match` uses the WEAK comparison, so `W/` on either side is ignored,
/// and `*` matches any current representation — which every path here has.
fn already_held(headers: &HeaderMap, etag: &HeaderValue) -> bool {
    let ours = opaque(etag.to_str().unwrap_or_default());
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .any(|tag| tag == "*" || opaque(tag) == ours)
}

fn opaque(tag: &str) -> &str {
    tag.strip_prefix("W/").unwrap_or(tag)
}

/// The first 128 bits of the SHA-256 of `bytes`, in lowercase hex.
fn digest(bytes: &[u8]) -> String {
    let hash = ring::digest::digest(&ring::digest::SHA256, bytes);
    let mut hex = String::with_capacity(32);
    for byte in &hash.as_ref()[..16] {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

#[cfg(test)]
mod tests {
    use super::*;

    // That every reference in the REAL shells is rewritten, and served immutable under it, is
    // checked through the whole router in `tests/asset_caching.rs`.

    fn get(site: &Site, uri: &str, if_none_match: Option<&str>) -> Response {
        let mut headers = HeaderMap::new();
        if let Some(tag) = if_none_match {
            headers.insert(
                header::IF_NONE_MATCH,
                HeaderValue::from_str(tag).expect("tag"),
            );
        }
        site.answer(&uri.parse().expect("uri"), &headers)
            .expect("a static path")
    }

    fn header_of(response: &Response, name: header::HeaderName) -> &str {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
    }

    const PAGE: &[u8] =
        br#"<link href="/a.css"><script src="/a.js"></script><a href="/a">"/a.js"</a>"#;

    fn site(script: &'static [u8]) -> Site {
        Site::new(
            &[
                source("/a", HTML, PAGE, Role::Shell),
                source("/a.js", JAVASCRIPT, script, Role::File),
                source("/a.css", CSS, b"body {}", Role::File),
            ],
            Names::default(),
        )
    }

    fn names(name: &str, short_name: &str) -> Names {
        Names {
            name: name.to_owned(),
            short_name: short_name.to_owned(),
        }
    }

    #[test]
    fn each_spelling_of_the_name_is_in_the_files_exactly_once() {
        // `#343 pwa-app-name`. Zero would leave every deployment called `vibe-talk` there without a
        // word; two would rename the first and leave the second.
        let custom = names("Work", "W");
        for (written, _) in shell_spellings(&custom) {
            assert_eq!(VOICE_HTML.matches(written.as_str()).count(), 1, "{written}");
        }
        for (written, _) in manifest_spellings(&custom) {
            assert_eq!(MANIFEST.matches(written.as_str()).count(), 1, "{written}");
        }
    }

    #[test]
    fn with_the_default_names_the_documents_are_the_files_as_written() {
        assert_eq!(name_manifest(MANIFEST, &Names::default()), MANIFEST);
        assert_eq!(name_shell(VOICE_HTML, &Names::default()), VOICE_HTML);
    }

    #[test]
    fn a_configured_name_changes_the_two_names_in_the_manifest_and_nothing_else() {
        let written: serde_json::Value = serde_json::from_str(MANIFEST).expect("JSON");
        let named = name_manifest(MANIFEST, &names("Work \"chat\" \\ ü", "Wörk"));
        let mut served: serde_json::Value = serde_json::from_str(&named).expect("still JSON");
        assert_eq!(served["name"], "Work \"chat\" \\ ü");
        assert_eq!(served["short_name"], "Wörk");
        // The identity an installed app is matched by is untouched.
        for member in ["id", "start_url", "scope"] {
            assert_eq!(served[member], written[member], "{member}");
        }
        served["name"] = written["name"].clone();
        served["short_name"] = written["short_name"].clone();
        assert_eq!(served, written, "a member other than the names moved");
    }

    #[test]
    fn a_configured_name_is_escaped_into_the_shell() {
        let page = name_shell(VOICE_HTML, &names("<b>Work</b> & 'co'", "W\"k"));
        assert!(
            page.contains("<title>&lt;b&gt;Work&lt;/b&gt; &amp; &#39;co&#39;</title>"),
            "the title"
        );
        assert!(
            page.contains("<h1>&lt;b&gt;Work&lt;/b&gt; &amp; &#39;co&#39;</h1>"),
            "the sign-in heading"
        );
        assert!(
            page.contains(r#"<meta name="apple-mobile-web-app-title" content="W&quot;k" />"#),
            "the home-screen label is the short name"
        );
        assert!(!page.contains("<b>Work</b>"), "the name was not escaped");
        assert!(!page.contains("<title>vibe-talk</title>"));
    }

    #[test]
    fn one_site_is_built_per_pair_of_names() {
        let first = super::site("one-site-per-names", "a");
        let again = super::site("one-site-per-names", "a");
        let other = super::site("one-site-per-names", "b");
        assert!(
            Arc::ptr_eq(&first, &again),
            "the same names built a second site"
        );
        assert!(
            !Arc::ptr_eq(&first, &other),
            "a different short name shared a site"
        );
        assert_eq!(other.names, names("one-site-per-names", "b"));
        // The files are the same bytes under the same URLs whatever the app is called.
        assert_eq!(
            first.entries["/voice.js"].version,
            other.entries["/voice.js"].version
        );
        assert_ne!(first.entries["/"].etag, other.entries["/"].etag);
        assert_ne!(
            first.entries["/manifest.webmanifest"].etag,
            other.entries["/manifest.webmanifest"].etag
        );
    }

    #[test]
    fn a_shell_names_each_file_by_its_hash_and_moves_when_a_file_does() {
        let before = site(b"one();");
        let after = site(b"two();");
        let shell =
            |site: &Site| String::from_utf8(site.entries["/a"].body.to_vec()).expect("utf-8");
        let (old, new) = (shell(&before), shell(&after));
        let script = |site: &Site| site.entries["/a.js"].version.clone().expect("a file");
        assert!(
            old.contains(&format!("src=\"/a.js?v={}\"", script(&before))),
            "{old}"
        );
        assert!(
            new.contains(&format!("src=\"/a.js?v={}\"", script(&after))),
            "{new}"
        );
        assert_ne!(script(&before), script(&after));
        // The stylesheet did not change, so neither did its URL: a deploy re-downloads only what
        // moved.
        assert_eq!(
            before.entries["/a.css"].version,
            after.entries["/a.css"].version
        );
        // A page link is not a file reference, and neither is the same path quoted in prose.
        assert!(new.contains(r#"<a href="/a">"/a.js"</a>"#), "{new}");
        assert_ne!(before.entries["/a"].etag, after.entries["/a"].etag);
    }

    #[test]
    fn only_the_current_hash_is_immutable_and_every_other_spelling_revalidates() {
        let site = site(b"one();");
        let version = site.entries["/a.js"].version.clone().expect("a file");
        let named = get(&site, &format!("/a.js?v={version}"), None);
        assert_eq!(named.status(), StatusCode::OK);
        assert_eq!(header_of(&named, header::CACHE_CONTROL), IMMUTABLE);
        for uri in [
            "/a.js".to_owned(),
            "/a.js?v=0123456789abcdef0123456789abcdef".to_owned(),
            format!("/a.js?v={version}&x=1"),
            format!("/a.js?x=1&v={version}"),
        ] {
            let response = get(&site, &uri, None);
            assert_eq!(response.status(), StatusCode::OK, "{uri}");
            assert_eq!(
                header_of(&response, header::CACHE_CONTROL),
                REVALIDATE,
                "{uri}"
            );
            assert_eq!(
                header_of(&response, header::ETAG),
                header_of(&named, header::ETAG),
                "{uri}"
            );
        }
        // A shell is never immutable, whatever it is asked with.
        assert_eq!(
            header_of(
                &get(&site, &format!("/a?v={version}"), None),
                header::CACHE_CONTROL
            ),
            REVALIDATE
        );
    }

    #[test]
    fn if_none_match_is_the_weak_comparison() {
        let site = site(b"one();");
        let etag = site.entries["/a"].etag.to_str().expect("ascii").to_owned();
        assert!(etag.starts_with("W/\"") && etag.ends_with('"'), "{etag}");
        let strong = etag.trim_start_matches("W/").to_owned();
        for held in [
            etag.clone(),
            strong,
            "*".to_owned(),
            format!("\"other\", {etag}"),
        ] {
            let response = get(&site, "/a", Some(&held));
            assert_eq!(response.status(), StatusCode::NOT_MODIFIED, "{held}");
            assert_eq!(header_of(&response, header::ETAG), etag);
            assert_eq!(header_of(&response, header::CACHE_CONTROL), REVALIDATE);
            assert_eq!(header_of(&response, header::VARY), "accept-encoding");
            assert!(response.headers().get(header::CONTENT_TYPE).is_none());
        }
        for held in ["W/\"other\"", "\"\"", ""] {
            assert_eq!(
                get(&site, "/a", Some(held)).status(),
                StatusCode::OK,
                "{held}"
            );
        }
    }
}
