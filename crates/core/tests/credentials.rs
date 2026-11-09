//! Credentials that exchange a secret or a refresh token for access
//! tokens, against a local token endpoint.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use fake_server::{FakeServer, Response};
use origincloud_core::{CliSession, Credentials, Error, InstanceMetadata, ServiceAccount};

/// A token endpoint answering with access tokens numbered from 1 that
/// expire after `expires_in` seconds, recording the last form it got.
struct TokenEndpoint {
    server: FakeServer,
    issued: Arc<AtomicU32>,
    form: Arc<Mutex<Vec<(String, String)>>>,
}

async fn token_endpoint(expires_in: u64) -> TokenEndpoint {
    let issued = Arc::new(AtomicU32::new(0));
    let form = Arc::new(Mutex::new(Vec::new()));
    let (count, last) = (Arc::clone(&issued), Arc::clone(&form));
    let server = FakeServer::start(move |r| {
        assert_eq!(r.header("content-type"), Some("application/x-www-form-urlencoded"));
        let received = fake_server::Received {
            query: r.text(),
            ..r
        }
        .query_pairs();
        let refused = received.iter().any(|(k, v)| k == "client_secret" && v == "wrong");
        *last.lock().unwrap() = received;
        let n = if refused { 0 } else { count.fetch_add(1, Ordering::SeqCst) + 1 };
        async move {
            if refused {
                return Response::json(r#"{"error":"unauthorized_client","error_description":"Invalid client secret"}"#)
                    .with_status(401);
            }
            Response::json(
                serde_json::json!({
                    "access_token": format!("token-{n}"),
                    "expires_in": expires_in,
                    "refresh_token": format!("refresh-{n}"),
                    "refresh_expires_in": 1800,
                })
                .to_string(),
            )
        }
    })
    .await;
    TokenEndpoint {
        server,
        issued,
        form,
    }
}

impl TokenEndpoint {
    fn form(&self, name: &str) -> String {
        let form = self.form.lock().unwrap();
        form.iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }
}

#[tokio::test]
async fn a_service_account_token_is_reused_until_it_is_about_to_expire() {
    let endpoint = token_endpoint(300).await;
    let credentials = ServiceAccount::new("id", "secret").with_token_url(endpoint.server.url());
    for _ in 0..3 {
        assert_eq!(credentials.token().await.unwrap(), "token-1");
    }
    assert_eq!(endpoint.issued.load(Ordering::SeqCst), 1);
    assert_eq!(endpoint.form("grant_type"), "client_credentials");
    assert_eq!(endpoint.form("client_id"), "id");
    assert_eq!(endpoint.form("client_secret"), "secret");

    // A token within the margin of its expiry is replaced.
    let endpoint = token_endpoint(10).await;
    let expiring = ServiceAccount::new("id", "secret").with_token_url(endpoint.server.url());
    assert_eq!(expiring.token().await.unwrap(), "token-1");
    assert_eq!(expiring.token().await.unwrap(), "token-2");
}

#[tokio::test]
async fn a_refused_service_account_says_why() {
    let endpoint = token_endpoint(300).await;
    let credentials = ServiceAccount::new("id", "wrong").with_token_url(endpoint.server.url());
    let err = credentials.token().await.unwrap_err();
    assert!(err.to_string().contains("Invalid client secret"), "{err}");
    assert!(err.to_string().contains("HTTP 401"), "{err}");
}

fn rfc3339(t: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339()
}

#[tokio::test]
async fn an_expired_cli_session_is_renewed_without_writing_its_file() {
    let dir = std::env::temp_dir().join(format!("origincloud-sdk-session-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("session.json");
    let now = SystemTime::now();
    let stored = serde_json::json!({
        "version": 1,
        "access_token": "stored-access",
        "expires": rfc3339(now - Duration::from_secs(60)),
        "refresh_token": "stored-refresh",
        "refresh_expires": rfc3339(now + Duration::from_secs(3600)),
        "token_type": "Bearer",
    })
    .to_string();
    std::fs::write(&path, &stored).unwrap();
    let endpoint = token_endpoint(300).await;

    let session = CliSession::load_from(&path)
        .unwrap()
        .unwrap()
        .with_token_url(endpoint.server.url());
    assert_eq!(session.token().await.unwrap(), "token-1");
    assert_eq!(endpoint.form("grant_type"), "refresh_token");
    assert_eq!(endpoint.form("client_id"), "origin-cli");
    assert_eq!(endpoint.form("refresh_token"), "stored-refresh");
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        stored,
        "the CLI's session file was written"
    );

    // The renewed token is kept, in memory.
    assert_eq!(session.token().await.unwrap(), "token-1");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_missing_cli_session_is_none() {
    let path = std::env::temp_dir().join("origincloud-sdk-no-such-session.json");
    assert!(CliSession::load_from(path).unwrap().is_none());
}

/// A metadata service granting sessions numbered from 1 and credentials
/// numbered from 1 that live five minutes, refusing the sessions in
/// `expired`, and answering a credential request with `credential_status`
/// when it isn't 200.
struct MetadataService {
    server: FakeServer,
    sessions: Arc<AtomicU32>,
    issued: Arc<AtomicU32>,
}

async fn metadata_service(
    expired: &'static [&'static str],
    credential_status: u16,
) -> MetadataService {
    let sessions = Arc::new(AtomicU32::new(0));
    let issued = Arc::new(AtomicU32::new(0));
    let (opened, granted) = (Arc::clone(&sessions), Arc::clone(&issued));
    let server = FakeServer::start(move |r| {
        let answer = match (r.method.as_str(), r.path.as_str()) {
            ("PUT", "/latest/api/token") => {
                assert_eq!(
                    r.header("x-origincloud-metadata-token-ttl-seconds"),
                    Some("21600")
                );
                let n = opened.fetch_add(1, Ordering::SeqCst) + 1;
                Response {
                    status: 200,
                    headers: Vec::new(),
                    body: format!("session-{n}").into_bytes(),
                }
            }
            ("GET", "/latest/meta-data/service-account/token") => {
                let session = r.header("x-origincloud-metadata-token").unwrap_or_default();
                if session.is_empty() || expired.contains(&session) {
                    Response::json("").with_status(401)
                } else if credential_status != 200 {
                    Response::json("").with_status(credential_status)
                } else {
                    let n = granted.fetch_add(1, Ordering::SeqCst) + 1;
                    let expires_at: chrono::DateTime<chrono::Utc> =
                        (SystemTime::now() + Duration::from_secs(300)).into();
                    Response::json(
                        serde_json::json!({
                            "access_token": format!("{session}/token-{n}"),
                            "token_type": "Bearer",
                            "expires_at": expires_at.to_rfc3339(),
                        })
                        .to_string(),
                    )
                }
            }
            _ => Response::json("").with_status(404),
        };
        async move { answer }
    })
    .await;
    MetadataService {
        server,
        sessions,
        issued,
    }
}

#[tokio::test]
async fn the_metadata_service_s_credential_is_reused_until_it_is_about_to_expire() {
    let service = metadata_service(&[], 200).await;
    let credentials = InstanceMetadata::new().with_endpoint(service.server.url());
    for _ in 0..3 {
        assert_eq!(credentials.token().await.unwrap(), "session-1/token-1");
    }
    assert_eq!(service.sessions.load(Ordering::SeqCst), 1);
    assert_eq!(service.issued.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_session_the_metadata_service_no_longer_accepts_is_opened_again() {
    let service = metadata_service(&["session-1"], 200).await;
    let credentials = InstanceMetadata::new().with_endpoint(service.server.url());
    assert_eq!(credentials.token().await.unwrap(), "session-2/token-1");
    assert_eq!(service.sessions.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_session_refused_twice_is_an_error() {
    let service = metadata_service(&["session-1", "session-2"], 200).await;
    let credentials = InstanceMetadata::new().with_endpoint(service.server.url());
    let err = credentials.token().await.unwrap_err();
    assert!(err.to_string().contains("HTTP 401"), "{err}");
    assert_eq!(service.sessions.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn an_instance_without_a_service_account_has_no_credential() {
    let service = metadata_service(&[], 404).await;
    let credentials = InstanceMetadata::new().with_endpoint(service.server.url());
    let err = credentials.token().await.unwrap_err();
    assert!(
        err.to_string().contains("runs as no service account"),
        "{err}"
    );
}

#[tokio::test]
async fn a_metadata_service_that_cant_sign_is_an_error() {
    let service = metadata_service(&[], 503).await;
    let credentials = InstanceMetadata::new().with_endpoint(service.server.url());
    let err = credentials.token().await.unwrap_err();
    assert!(err.to_string().contains("HTTP 503"), "{err}");
}

#[tokio::test]
async fn without_a_metadata_service_there_are_no_credentials_and_quickly() {
    // A port nothing listens on: bound, then released.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let credentials = InstanceMetadata::new().with_endpoint(format!("http://127.0.0.1:{port}"));
    let started = std::time::Instant::now();
    let err = credentials.token().await.unwrap_err();
    assert!(matches!(err, Error::NoCredentials), "{err}");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}
