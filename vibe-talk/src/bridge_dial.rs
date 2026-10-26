//! How this server dials a voice bridge itself: read-aloud sessions, and the capability hello.
//!
//! `#553 voice-ws-relay-auth`. Both used to call `connect_async(url)` and nothing else, which was
//! enough for a bridge on loopback. A bridge reached through ANOTHER vibe-talk's relay needs two
//! things more: that server's voice-only token, sent as `Authorization: Bearer`, and, when the
//! relay's certificate was issued by a private authority, a root to trust it with. Both come from
//! files named in `[read_aloud]` (`websocket_token_file`, `websocket_ca_file`) and are read at each
//! connection, so a rotated token or root takes effect without a restart, and neither is held in
//! memory between calls.
//!
//! The credentials belong to `read_aloud.websocket_url` only: a dial at `conversation.websocket_url`
//! (the fallback when read-aloud names no URL of its own) carries none, so a token meant for one
//! address is never sent to another.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::net::TcpStream;
use tokio_rustls::rustls;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::config::ReadAloudConfig;

/// A connected bridge socket.
pub type BridgeSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// What a dial presents beyond the URL: a bearer token and extra trusted roots, each from a file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Credentials {
    /// File holding the bearer token (surrounding whitespace ignored).
    pub token_file: Option<PathBuf>,
    /// PEM file of certificates trusted in addition to the bundled public roots.
    pub ca_file: Option<PathBuf>,
}

impl Credentials {
    /// The credentials `[read_aloud]` names for its own `websocket_url`, or none when it names no
    /// URL (the dial then goes to `conversation.websocket_url`, which they do not belong to).
    #[must_use]
    pub fn for_read_aloud(read_aloud: &ReadAloudConfig) -> Self {
        let named = read_aloud
            .websocket_url
            .as_deref()
            .is_some_and(|url| !url.trim().is_empty());
        if !named {
            return Self::default();
        }
        Self {
            token_file: read_aloud.websocket_token_file.clone(),
            ca_file: read_aloud.websocket_ca_file.clone(),
        }
    }

    /// Whether there is nothing to present.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.token_file.is_none() && self.ca_file.is_none()
    }
}

/// Why a dial did not get as far as the network, or failed there.
#[derive(Debug, thiserror::Error)]
pub enum DialError {
    /// A credential file could not be read or used.
    #[error("{0}")]
    Credential(String),
    /// The connection or the WebSocket handshake failed.
    #[error(transparent)]
    Connect(#[from] tokio_tungstenite::tungstenite::Error),
}

/// Open a WebSocket to `url`, presenting `credentials`.
///
/// # Errors
///
/// [`DialError::Credential`] when a named file cannot be read or holds no usable token or
/// certificate (said before any connection is made), and [`DialError::Connect`] when the
/// connection or handshake fails, including a relay's 401 for a token it does not accept.
pub async fn connect(url: &str, credentials: &Credentials) -> Result<BridgeSocket, DialError> {
    let mut request = url.into_client_request()?;
    if let Some(path) = credentials.token_file.as_deref() {
        let token = read_token(path)?;
        let value = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
            DialError::Credential(format!(
                "read_aloud.websocket_token_file {}: the token is not a valid header value",
                path.display()
            ))
        })?;
        request.headers_mut().insert(
            tokio_tungstenite::tungstenite::http::header::AUTHORIZATION,
            value,
        );
    }
    let connector = match credentials.ca_file.as_deref() {
        Some(path) => Some(tokio_tungstenite::Connector::Rustls(Arc::new(
            client_config(path)?,
        ))),
        None => None,
    };
    let (socket, _) =
        tokio_tungstenite::connect_async_tls_with_config(request, None, false, connector).await?;
    Ok(socket)
}

fn read_token(path: &Path) -> Result<String, DialError> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        DialError::Credential(format!(
            "read_aloud.websocket_token_file {}: {error}",
            path.display()
        ))
    })?;
    let token = text.trim();
    if token.is_empty() {
        return Err(DialError::Credential(format!(
            "read_aloud.websocket_token_file {} is empty",
            path.display()
        )));
    }
    Ok(token.to_owned())
}

/// A TLS client configuration trusting the bundled public roots and every certificate in `path`.
fn client_config(path: &Path) -> Result<rustls::ClientConfig, DialError> {
    use rustls::pki_types::pem::PemObject as _;
    use rustls::pki_types::CertificateDer;

    let named = |detail: String| {
        DialError::Credential(format!(
            "read_aloud.websocket_ca_file {}: {detail}",
            path.display()
        ))
    };
    let pem = std::fs::read(path).map_err(|error| named(error.to_string()))?;
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut added = 0_usize;
    for certificate in CertificateDer::pem_slice_iter(&pem) {
        let certificate = certificate.map_err(|error| named(format!("not PEM: {error}")))?;
        roots
            .add(certificate)
            .map_err(|error| named(format!("a certificate was refused: {error}")))?;
        added += 1;
    }
    if added == 0 {
        return Err(named("holds no certificate".to_owned()));
    }
    rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|error| named(error.to_string()))
        .map(|builder| builder.with_root_certificates(roots).with_no_client_auth())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TempDir;
    use futures_util::StreamExt as _;

    fn file(dir: &TempDir, name: &str, contents: &str) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, contents).expect("write a test file");
        path
    }

    fn read_aloud(url: Option<&str>) -> ReadAloudConfig {
        ReadAloudConfig {
            websocket_url: url.map(str::to_owned),
            websocket_token_file: Some(PathBuf::from("/token")),
            websocket_ca_file: Some(PathBuf::from("/ca.pem")),
            ..ReadAloudConfig::default()
        }
    }

    #[test]
    fn credentials_belong_to_read_alouds_own_url_only() {
        let own = Credentials::for_read_aloud(&read_aloud(Some("wss://relay.example.test/x")));
        assert_eq!(own.token_file.as_deref(), Some(Path::new("/token")));
        assert_eq!(own.ca_file.as_deref(), Some(Path::new("/ca.pem")));
        assert!(Credentials::for_read_aloud(&read_aloud(None)).is_empty());
        assert!(Credentials::for_read_aloud(&read_aloud(Some("  "))).is_empty());
    }

    // The handshake callback's error type is `http::Response<Option<String>>`, fixed by
    // tungstenite's `Callback` trait, so clippy's suggestion (boxing it) is not available.
    #[allow(clippy::result_large_err)]
    #[tokio::test]
    async fn the_token_is_sent_as_a_bearer_header() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let seen = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut header = None;
            let socket = tokio_tungstenite::accept_hdr_async(
                stream,
                |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
                    header = request
                        .headers()
                        .get("authorization")
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_owned);
                    Ok(response)
                },
            )
            .await
            .unwrap();
            drop(socket);
            header
        });
        let dir = TempDir::new("dial-token");
        let credentials = Credentials {
            token_file: Some(file(&dir, "token", "  voice-token-that-is-long-enough\n")),
            ca_file: None,
        };
        let mut socket = connect(&format!("ws://{address}/ws"), &credentials)
            .await
            .unwrap();
        let _ = socket.next().await;
        assert_eq!(
            seen.await.unwrap().as_deref(),
            Some("Bearer voice-token-that-is-long-enough")
        );
    }

    #[tokio::test]
    async fn an_unusable_credential_is_named_before_any_connection() {
        let missing = Credentials {
            token_file: Some(PathBuf::from("/nonexistent/vibe-talk-token")),
            ca_file: None,
        };
        let error = connect("ws://127.0.0.1:9/ws", &missing).await.unwrap_err();
        assert!(matches!(error, DialError::Credential(ref why)
            if why.contains("websocket_token_file") && why.contains("/nonexistent/vibe-talk-token")));

        let dir = TempDir::new("dial-unusable");
        let blank = Credentials {
            token_file: Some(file(&dir, "empty", "\n")),
            ca_file: None,
        };
        let error = connect("ws://127.0.0.1:9/ws", &blank).await.unwrap_err();
        assert!(matches!(error, DialError::Credential(ref why) if why.ends_with("is empty")));

        let roots = Credentials {
            token_file: None,
            ca_file: Some(file(&dir, "ca.pem", "this is not a certificate")),
        };
        let error = connect("wss://127.0.0.1:9/ws", &roots).await.unwrap_err();
        assert!(matches!(error, DialError::Credential(ref why)
            if why.contains("websocket_ca_file") && why.contains("holds no certificate")));
    }
}
