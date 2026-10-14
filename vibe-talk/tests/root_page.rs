//! `#302 root-is-the-app`: the app is the page at `/`, the older page that used to be there is
//! gone, and the app's old address `/voice` forwards to `/` keeping its query string.
//!
//! Through the whole router, so the redirect is checked as a browser receives it: after the bearer
//! check, the access log and the compression layer have all had their turn.
//!
//! What only a browser can show — that the fragment survives the redirect, and that Chromium still
//! finds the page installable and in scope from either address — is `tests/pwa_installability.py`.

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
