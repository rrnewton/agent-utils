//! `#343 pwa-app-name`: each deployment names its installed app.
//!
//! The owner installs the app from more than one deployment, and every copy was called
//! `vibe-talk`. `[server] app_name` and `short_name` (or `VIBE_TALK_APP_NAME` and
//! `VIBE_TALK_APP_SHORT_NAME`) now name it. Checked here through the whole router, as an installer
//! and a browser see it: the manifest's `name` and `short_name`, the page's title, sign-in heading
//! and iOS home-screen label, and client-config's `app_name` — and that nothing else moves, the
//! manifest's identity least of all. The last test reads the guides instead, against the constants.
//!
//! What the configuration refuses is unit-tested beside it in `src/config.rs`, and that the binary
//! refuses to start on it, naming the key, in `tests/startup.rs`.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use http_body_util::BodyExt as _;
use tower::ServiceExt as _;
use vibe_talk::config::Config;
use vibe_talk::http::router;
use vibe_talk::state::AppState;
use vibe_talk::testing::{config_toml, state_from_toml, state_parts, READ_TOKEN};

/// The manifest as it is written, which a deployment that names nothing serves byte for byte.
const MANIFEST: &str = include_str!("../web/manifest.webmanifest");

const HOME_SCREEN_LABEL: &str = r#"<meta name="apple-mobile-web-app-title" content="#;

/// `GET uri` with `headers`, and the whole answer, uncompressed.
async fn fetch(
    app: &axum::Router,
    uri: &str,
    headers: &[(&str, &str)],
) -> (StatusCode, HeaderMap, Vec<u8>) {
    let mut builder = Request::builder()
        .method("GET")
        .uri(uri)
        .header("accept-encoding", "identity");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = app
        .clone()
        .oneshot(builder.body(Body::empty()).expect("request"))
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

fn etag(headers: &HeaderMap) -> String {
    headers
        .get("etag")
        .and_then(|value| value.to_str().ok())
        .expect("an ETag")
        .to_owned()
}

async fn page(app: &axum::Router) -> String {
    let (status, _, body) = fetch(app, "/", &[]).await;
    assert_eq!(status, StatusCode::OK);
    String::from_utf8(body).expect("utf-8")
}

async fn manifest(app: &axum::Router) -> serde_json::Value {
    let (status, _, body) = fetch(app, "/manifest.webmanifest", &[]).await;
    assert_eq!(status, StatusCode::OK);
    serde_json::from_slice(&body).expect("the manifest is JSON")
}

async fn client_config(app: &axum::Router) -> serde_json::Value {
    let bearer = format!("Bearer {READ_TOKEN}");
    let (status, _, body) = fetch(
        app,
        "/api/v1/client-config",
        &[("authorization", bearer.as_str())],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    serde_json::from_slice(&body).expect("client-config is JSON")
}

/// The test configuration with `lines` added to its `[server]` table.
fn server_toml(lines: &[(&str, &str)]) -> String {
    let mut table = String::from("[server]\n");
    for (key, value) in lines {
        let value = toml::Value::String((*value).to_owned());
        table.push_str(&format!("{key} = {value}\n"));
    }
    let text = config_toml().replacen("[server]\n", &table, 1);
    assert_ne!(
        text,
        config_toml(),
        "the test configuration has no [server] table"
    );
    text
}

fn named(lines: &[(&str, &str)]) -> axum::Router {
    let (state, _discord, _voice) = state_from_toml(&server_toml(lines));
    router(state)
}

fn unnamed() -> axum::Router {
    let (state, _discord, _voice) = state_parts();
    router(state)
}

/// Every content-addressed file URL a page's `src` and `href` attributes name.
fn file_urls(page: &str) -> Vec<&str> {
    let mut urls = Vec::new();
    for attribute in ["src=\"/", "href=\"/"] {
        for (at, _) in page.match_indices(attribute) {
            let start = at + attribute.len() - 1;
            let end = start + page[start..].find('"').expect("a closed attribute");
            let url = &page[start..end];
            if url.contains("?v=") {
                urls.push(url);
            }
        }
    }
    urls
}

#[tokio::test]
async fn a_deployment_that_names_nothing_serves_the_manifest_as_written() {
    let app = unnamed();
    let (status, _, body) = fetch(&app, "/manifest.webmanifest", &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body == MANIFEST.as_bytes(),
        "the default manifest must be the file, byte for byte"
    );

    let page = page(&app).await;
    for marker in [
        "<title>vibe-talk</title>",
        "<h1>vibe-talk</h1>",
        r#"<meta name="apple-mobile-web-app-title" content="vibe-talk" />"#,
    ] {
        assert_eq!(page.matches(marker).count(), 1, "{marker}");
    }
    assert_eq!(client_config(&app).await["app_name"], "vibe-talk");
}

#[tokio::test]
async fn a_named_deployment_serves_its_names_and_keeps_its_identity() {
    let name = "vibe-talk at \"work\" & <co>";
    let app = named(&[("app_name", name), ("short_name", "Work")]);

    let written: serde_json::Value = serde_json::from_str(MANIFEST).expect("JSON");
    let mut served = manifest(&app).await;
    assert_eq!(served["name"], name);
    assert_eq!(served["short_name"], "Work");
    // The identity an installed copy is matched by does not move with its name: renaming an app
    // must not make it another one.
    assert_eq!(served["id"], "/voice");
    assert_eq!(served["start_url"], "/");
    assert_eq!(served["scope"], "/");
    served["name"] = written["name"].clone();
    served["short_name"] = written["short_name"].clone();
    assert_eq!(
        served, written,
        "a manifest member besides the two names moved"
    );

    let page = page(&app).await;
    let escaped = "vibe-talk at &quot;work&quot; &amp; &lt;co&gt;";
    assert!(
        page.contains(&format!("<title>{escaped}</title>")),
        "the title"
    );
    assert!(
        page.contains(&format!("<h1>{escaped}</h1>")),
        "the sign-in heading"
    );
    assert!(
        page.contains(&format!("{HOME_SCREEN_LABEL}\"Work\" />")),
        "the iOS home-screen label is the short name"
    );
    for gone in ["<title>vibe-talk</title>", "<h1>vibe-talk</h1>", "<co>"] {
        assert!(!page.contains(gone), "{gone}");
    }

    assert_eq!(client_config(&app).await["app_name"], name);
}

#[tokio::test]
async fn the_short_name_follows_the_name_unless_it_is_set() {
    let app = named(&[("app_name", "Home")]);
    let served = manifest(&app).await;
    assert_eq!(served["name"], "Home");
    assert_eq!(served["short_name"], "Home");
    assert!(page(&app)
        .await
        .contains(&format!("{HOME_SCREEN_LABEL}\"Home\" />")));
}

#[tokio::test]
async fn the_environment_names_the_app_over_the_file() {
    let text = server_toml(&[("app_name", "from the file")]);
    let env = BTreeMap::from([("VIBE_TALK_APP_NAME".to_owned(), "from the env".to_owned())]);
    let config = Config::from_toml_and_env(&text, &env).expect("valid configuration");
    let (base, _discord, _voice) = state_parts();
    let app = router(AppState {
        config: Arc::new(config),
        ..base
    });
    let served = manifest(&app).await;
    assert_eq!(served["name"], "from the env");
    assert_eq!(served["short_name"], "from the env");
    assert!(page(&app).await.contains("<title>from the env</title>"));
    assert_eq!(client_config(&app).await["app_name"], "from the env");
}

#[tokio::test]
async fn two_deployments_side_by_side_each_revalidate_their_own_documents() {
    // Two servers in one process, the way two deployments sit on one phone: each answers with its
    // own names and its own validators, and the files both name are the same bytes at the same URLs.
    let work = named(&[("app_name", "Work")]);
    let home = unnamed();

    let (_, work_headers, _) = fetch(&work, "/manifest.webmanifest", &[]).await;
    let (_, home_headers, _) = fetch(&home, "/manifest.webmanifest", &[]).await;
    let (work_tag, home_tag) = (etag(&work_headers), etag(&home_headers));
    assert_ne!(
        work_tag, home_tag,
        "a renamed manifest kept the old one's ETag"
    );
    let (status, _, _) = fetch(
        &work,
        "/manifest.webmanifest",
        &[("if-none-match", work_tag.as_str())],
    )
    .await;
    assert_eq!(status, StatusCode::NOT_MODIFIED);
    // A copy installed under the old name is sent the new one.
    let (status, _, body) = fetch(
        &work,
        "/manifest.webmanifest",
        &[("if-none-match", home_tag.as_str())],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let renamed: serde_json::Value = serde_json::from_slice(&body).expect("JSON");
    assert_eq!(renamed["name"], "Work");

    let (_, work_shell, _) = fetch(&work, "/", &[]).await;
    let (_, home_shell, _) = fetch(&home, "/", &[]).await;
    assert_ne!(etag(&work_shell), etag(&home_shell));

    let (work_page, home_page) = (page(&work).await, page(&home).await);
    assert!(work_page.contains("<title>Work</title>"));
    assert!(home_page.contains("<title>vibe-talk</title>"));
    let files = file_urls(&home_page);
    assert!(files.len() >= 4, "{files:?}");
    assert_eq!(file_urls(&work_page), files);
}

#[test]
fn the_guides_state_the_keys_the_default_and_the_limit_the_code_has() {
    // Two separate artifacts, compared: prose nothing generated, and the constants the server
    // runs with. A changed default or limit names the guide that still states the old one.
    use vibe_talk::config::{
        DEFAULT_APP_NAME, ENV_APP_NAME, ENV_APP_SHORT_NAME, MAX_APP_NAME_CHARS,
    };
    let readme = include_str!("../README.md");
    let row = readme
        .lines()
        .find(|line| line.starts_with("| App name |"))
        .expect("the README's configuration table has an App name row");
    let example = include_str!("../vibe-talk.example.toml");
    let server = example
        .split("\n[server]\n")
        .nth(1)
        .and_then(|rest| rest.split("\n[").next())
        .expect("the example has a [server] table");
    let limit = format!("At most {MAX_APP_NAME_CHARS} characters");
    for (guide, text, default) in [
        (
            "the README's row",
            row,
            format!("default `{DEFAULT_APP_NAME}`"),
        ),
        (
            "the example's [server] table",
            server,
            format!("Both default to \"{DEFAULT_APP_NAME}\""),
        ),
    ] {
        for needle in [
            "app_name",
            "short_name",
            ENV_APP_NAME,
            ENV_APP_SHORT_NAME,
            default.as_str(),
            limit.as_str(),
        ] {
            assert!(
                text.contains(needle),
                "{guide} does not say {needle:?}:\n{text}"
            );
        }
    }

    // The example's two settings, uncommented, are ones this server accepts and serves.
    let settings: Vec<&str> = server
        .lines()
        .filter_map(|line| line.strip_prefix("# "))
        .filter(|line| line.starts_with("app_name = ") || line.starts_with("short_name = "))
        .collect();
    assert_eq!(settings.len(), 2, "{settings:?}");
    let text = config_toml().replacen(
        "[server]\n",
        &format!("[server]\n{}\n", settings.join("\n")),
        1,
    );
    let config = Config::from_toml_and_env(&text, &BTreeMap::new()).expect("the example parses");
    assert_ne!(
        config.app_name, DEFAULT_APP_NAME,
        "the example shows a name of its own"
    );
    assert_ne!(config.app_short_name, config.app_name);
}
