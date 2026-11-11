//! Credentials that exchange a secret, a refresh token or a CI job's
//! token for access tokens, against local token endpoints.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use fake_server::{FakeServer, Response};
use origincloud_core::{
    CliSession, Credentials, Error, Federated, InstanceMetadata, ServiceAccount,
};

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
/// `expired`, and answering the first credential requests with
/// `credential_statuses` while they last.
struct MetadataService {
    server: FakeServer,
    sessions: Arc<AtomicU32>,
    issued: Arc<AtomicU32>,
    requests: Arc<AtomicU32>,
}

async fn metadata_service(
    expired: &'static [&'static str],
    credential_statuses: &'static [u16],
) -> MetadataService {
    let sessions = Arc::new(AtomicU32::new(0));
    let issued = Arc::new(AtomicU32::new(0));
    let requests = Arc::new(AtomicU32::new(0));
    let (opened, granted, asked) = (
        Arc::clone(&sessions),
        Arc::clone(&issued),
        Arc::clone(&requests),
    );
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
                let n = asked.fetch_add(1, Ordering::SeqCst) as usize;
                let status = credential_statuses.get(n).copied().unwrap_or(200);
                if session.is_empty() || expired.contains(&session) {
                    Response::json("").with_status(401)
                } else if status != 200 {
                    Response::json("").with_status(status)
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
        requests,
    }
}

#[tokio::test]
async fn the_metadata_service_s_credential_is_reused_until_it_is_about_to_expire() {
    let service = metadata_service(&[], &[]).await;
    let credentials = InstanceMetadata::new().with_endpoint(service.server.url());
    for _ in 0..3 {
        assert_eq!(credentials.token().await.unwrap(), "session-1/token-1");
    }
    assert_eq!(service.sessions.load(Ordering::SeqCst), 1);
    assert_eq!(service.issued.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_session_the_metadata_service_no_longer_accepts_is_opened_again() {
    let service = metadata_service(&["session-1"], &[]).await;
    let credentials = InstanceMetadata::new().with_endpoint(service.server.url());
    assert_eq!(credentials.token().await.unwrap(), "session-2/token-1");
    assert_eq!(service.sessions.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_session_refused_twice_is_an_error() {
    let service = metadata_service(&["session-1", "session-2"], &[]).await;
    let credentials = InstanceMetadata::new().with_endpoint(service.server.url());
    let err = credentials.token().await.unwrap_err();
    assert!(err.to_string().contains("HTTP 401"), "{err}");
    assert_eq!(service.sessions.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn an_instance_without_a_service_account_has_no_credential() {
    let service = metadata_service(&[], &[404]).await;
    let credentials = InstanceMetadata::new().with_endpoint(service.server.url());
    let err = credentials.token().await.unwrap_err();
    assert!(
        err.to_string().contains("runs as no service account"),
        "{err}"
    );
}

#[tokio::test]
async fn a_metadata_service_that_cant_sign_right_now_is_asked_again() {
    let service = metadata_service(&[], &[503, 503]).await;
    let credentials = InstanceMetadata::new().with_endpoint(service.server.url());
    assert_eq!(credentials.token().await.unwrap(), "session-1/token-1");
    assert_eq!(service.requests.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn a_metadata_service_that_cant_sign_three_times_is_an_error() {
    let service = metadata_service(&[], &[503, 503, 503]).await;
    let credentials = InstanceMetadata::new().with_endpoint(service.server.url());
    let err = credentials.token().await.unwrap_err();
    assert!(err.to_string().contains("HTTP 503"), "{err}");
    assert_eq!(service.requests.load(Ordering::SeqCst), 3);
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

#[tokio::test]
async fn another_cloud_s_metadata_service_means_no_credentials() {
    // Azure's refuses the session request with 411, AWS's with 400, and
    // Google's with 405 or 404.
    for status in [400, 404, 405, 411] {
        let server =
            FakeServer::start(move |_| async move { Response::json("").with_status(status) }).await;
        let credentials = InstanceMetadata::new().with_endpoint(server.url());
        let err = credentials.token().await.unwrap_err();
        assert!(matches!(err, Error::NoCredentials), "{status}: {err}");
    }
}

/// GitHub's job token endpoint, at `/idtoken`, and the STS, at
/// `/v1/token`, on one server. GitHub answers with job tokens numbered from
/// 1, or `github_status` when it isn't 200; the STS answers its first
/// exchanges with `sts_answers` while they last, and then with access
/// tokens numbered from 1 that expire after `expires_in` seconds.
struct Federation {
    server: FakeServer,
    job_tokens: Arc<AtomicU32>,
    exchanges: Arc<AtomicU32>,
    github: Arc<Mutex<Vec<fake_server::Received>>>,
    subject_tokens: Arc<Mutex<Vec<String>>>,
    form: Arc<Mutex<Vec<(String, String)>>>,
}

async fn federation(
    github_status: u16,
    sts_answers: &'static [(u16, &'static str)],
    expires_in: u64,
) -> Federation {
    let job_tokens = Arc::new(AtomicU32::new(0));
    let exchanges = Arc::new(AtomicU32::new(0));
    let github = Arc::new(Mutex::new(Vec::new()));
    let subject_tokens = Arc::new(Mutex::new(Vec::new()));
    let form = Arc::new(Mutex::new(Vec::new()));
    let (fetched, exchanged, requests, subjects, last) = (
        Arc::clone(&job_tokens),
        Arc::clone(&exchanges),
        Arc::clone(&github),
        Arc::clone(&subject_tokens),
        Arc::clone(&form),
    );
    let server = FakeServer::start(move |r| {
        let answer = match (r.method.as_str(), r.path.as_str()) {
            ("GET", "/idtoken") => {
                requests.lock().unwrap().push(r);
                if github_status == 200 {
                    let n = fetched.fetch_add(1, Ordering::SeqCst) + 1;
                    Response::json(format!(r#"{{"count":1,"value":"job-{n}"}}"#))
                } else {
                    Response::json(
                        r#"{"message":"Unable to get ACTIONS_ID_TOKEN_REQUEST_URL env variable"}"#,
                    )
                    .with_status(github_status)
                }
            }
            ("POST", "/v1/token") => {
                assert_eq!(
                    r.header("content-type"),
                    Some("application/x-www-form-urlencoded")
                );
                let received = fake_server::Received {
                    query: r.text(),
                    ..r
                }
                .query_pairs();
                let subject = received
                    .iter()
                    .find(|(k, _)| k == "subject_token")
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default();
                subjects.lock().unwrap().push(subject);
                *last.lock().unwrap() = received;
                let n = exchanged.fetch_add(1, Ordering::SeqCst) as usize;
                match sts_answers.get(n) {
                    Some((status, body)) => Response::json(*body).with_status(*status),
                    None => Response::json(
                        serde_json::json!({
                            "access_token": format!("access-{}", n + 1 - sts_answers.len()),
                            "issued_token_type": "urn:ietf:params:oauth:token-type:access_token",
                            "token_type": "Bearer",
                            "expires_in": expires_in,
                        })
                        .to_string(),
                    ),
                }
            }
            _ => Response::json("").with_status(404),
        };
        async move {
            // Slow enough for concurrent callers to wait on one refresh.
            tokio::time::sleep(Duration::from_millis(20)).await;
            answer
        }
    })
    .await;
    Federation {
        server,
        job_tokens,
        exchanges,
        github,
        subject_tokens,
        form,
    }
}

impl Federation {
    /// Credentials of a GitHub Actions job whose request URL carries a
    /// query, as GitHub's does.
    fn github_actions(&self) -> Federated {
        Federated::github_actions(
            format!("{}/idtoken?api-version=2.0", self.server.url()),
            "request-token",
        )
        .with_endpoint(format!("{}/", self.server.url()))
    }

    fn form(&self, name: &str) -> String {
        let form = self.form.lock().unwrap();
        form.iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
            .unwrap_or_default()
    }

    fn subject_tokens(&self) -> Vec<String> {
        self.subject_tokens.lock().unwrap().clone()
    }
}

/// Asserts `err` holds none of the tokens the tests hand out.
fn assert_no_token(err: &Error) {
    let message = err.to_string();
    for token in ["request-token", "job-", "file-token", "access-"] {
        assert!(!message.contains(token), "{token} in {message}");
    }
}

fn token_file(contents: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("origincloud-sdk-oidc-{}", uuid::Uuid::new_v4()));
    std::fs::write(&path, contents).unwrap();
    path
}

#[tokio::test]
async fn a_github_actions_job_s_token_is_exchanged_and_reused() {
    let sts = federation(200, &[], 900).await;
    let credentials = sts.github_actions();
    for _ in 0..3 {
        assert_eq!(credentials.token().await.unwrap(), "access-1");
    }
    assert_eq!(sts.job_tokens.load(Ordering::SeqCst), 1);
    assert_eq!(sts.exchanges.load(Ordering::SeqCst), 1);

    let github = sts.github.lock().unwrap()[0].clone();
    assert_eq!(github.header("authorization"), Some("bearer request-token"));
    assert_eq!(
        github.query,
        "api-version=2.0&audience=https%3A%2F%2Fsts.origincloudapis.eu"
    );

    assert_eq!(
        sts.form("grant_type"),
        "urn:ietf:params:oauth:grant-type:token-exchange"
    );
    assert_eq!(sts.form("subject_token"), "job-1");
    assert_eq!(
        sts.form("subject_token_type"),
        "urn:ietf:params:oauth:token-type:jwt"
    );
}

#[tokio::test]
async fn a_request_url_without_a_query_gets_one_for_the_audience() {
    let sts = federation(200, &[], 900).await;
    let credentials = Federated::github_actions(format!("{}/idtoken", sts.server.url()), "t")
        .with_endpoint(sts.server.url())
        .with_audience("https://sts.example/a b");
    assert_eq!(credentials.token().await.unwrap(), "access-1");
    let github = sts.github.lock().unwrap()[0].clone();
    assert_eq!(github.query_values("audience"), ["https://sts.example/a b"]);
    assert!(github.query.starts_with("audience="), "{}", github.query);
}

#[tokio::test]
async fn each_renewal_exchanges_a_fresh_job_token() {
    // A token expiring within the minute's margin is replaced at once.
    let sts = federation(200, &[], 30).await;
    let credentials = sts.github_actions();
    assert_eq!(credentials.token().await.unwrap(), "access-1");
    assert_eq!(credentials.token().await.unwrap(), "access-2");
    assert_eq!(sts.subject_tokens(), ["job-1", "job-2"]);
}

#[tokio::test]
async fn concurrent_callers_share_one_exchange() {
    let sts = federation(200, &[], 900).await;
    let credentials = sts.github_actions();
    let (a, b, c) = tokio::join!(
        credentials.token(),
        credentials.token(),
        credentials.token()
    );
    for token in [a, b, c] {
        assert_eq!(token.unwrap(), "access-1");
    }
    assert_eq!(sts.job_tokens.load(Ordering::SeqCst), 1);
    assert_eq!(sts.exchanges.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_token_file_is_read_again_at_every_exchange() {
    let sts = federation(200, &[], 30).await;
    let path = token_file("file-token-1\n");
    let credentials = Federated::from_file(&path).with_endpoint(sts.server.url());
    assert_eq!(credentials.token().await.unwrap(), "access-1");
    std::fs::write(&path, "file-token-2").unwrap();
    assert_eq!(credentials.token().await.unwrap(), "access-2");
    assert_eq!(sts.subject_tokens(), ["file-token-1", "file-token-2"]);
    assert_eq!(sts.job_tokens.load(Ordering::SeqCst), 0);
    std::fs::remove_file(&path).unwrap();
}

#[tokio::test]
async fn a_missing_or_empty_token_file_is_an_error() {
    let sts = federation(200, &[], 900).await;
    let missing = std::env::temp_dir().join("origincloud-sdk-no-such-oidc-token");
    let err = Federated::from_file(&missing)
        .with_endpoint(sts.server.url())
        .token()
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("reading the OIDC token file"),
        "{err}"
    );

    let empty = token_file(" \n");
    let err = Federated::from_file(&empty)
        .with_endpoint(sts.server.url())
        .token()
        .await
        .unwrap_err();
    assert!(err.to_string().contains("is empty"), "{err}");
    assert_eq!(sts.exchanges.load(Ordering::SeqCst), 0);
    std::fs::remove_file(&empty).unwrap();
}

/// An unsigned JWT with the claims `claims`, as a CI job's token would be
/// but for its signature, which only the STS checks.
fn jwt(claims: &serde_json::Value) -> String {
    use base64::Engine as _;
    let encode = |part: String| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(part);
    format!(
        "{}.{}.",
        encode(r#"{"alg":"none","typ":"JWT"}"#.to_owned()),
        encode(claims.to_string())
    )
}

fn unix_seconds(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs()
}

#[tokio::test]
async fn a_given_token_is_exchanged() {
    let sts = federation(200, &[], 900).await;
    let token = jwt(&serde_json::json!({
        "sub": "project_path:group/project:ref_type:branch:ref:main",
        "exp": unix_seconds(SystemTime::now() + Duration::from_secs(300)),
    }));
    let credentials = Federated::from_token(token.clone()).with_endpoint(sts.server.url());
    for _ in 0..3 {
        assert_eq!(credentials.token().await.unwrap(), "access-1");
    }
    assert_eq!(sts.subject_tokens(), [token]);
    assert_eq!(
        sts.form("subject_token_type"),
        "urn:ietf:params:oauth:token-type:jwt"
    );
    assert_eq!(sts.job_tokens.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn each_renewal_exchanges_a_given_token_again() {
    // A token expiring within the minute's margin is replaced at once.
    let sts = federation(200, &[], 30).await;
    let token = jwt(&serde_json::json!({
        "exp": unix_seconds(SystemTime::now() + Duration::from_secs(300)),
    }));
    let credentials = Federated::from_token(token.clone()).with_endpoint(sts.server.url());
    assert_eq!(credentials.token().await.unwrap(), "access-1");
    assert_eq!(credentials.token().await.unwrap(), "access-2");
    assert_eq!(credentials.token().await.unwrap(), "access-3");
    assert_eq!(sts.subject_tokens(), [token.clone(), token.clone(), token]);
}

#[tokio::test]
async fn an_expired_given_token_is_not_exchanged() {
    let sts = federation(200, &[], 900).await;
    let token = jwt(&serde_json::json!({ "exp": 1577836800 }));
    let err = Federated::from_token(token.clone())
        .with_endpoint(sts.server.url())
        .token()
        .await
        .unwrap_err();
    assert!(matches!(err, Error::Credentials(_)), "{err:?}");
    let message = err.to_string();
    assert!(
        message.contains("the CI job's OIDC token expired at 2020-01-01T00:00:00Z"),
        "{message}"
    );
    assert!(message.contains("can't be renewed"), "{message}");
    assert!(message.contains("ORIGINCLOUD_OIDC_TOKEN_FILE"), "{message}");
    // Given through the API, not the variable, which isn't named.
    assert!(!message.contains("in ORIGINCLOUD_OIDC_TOKEN"), "{message}");
    let claims = token.split('.').nth(1).unwrap();
    assert!(!message.contains(claims), "{message}");
    assert_eq!(sts.exchanges.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_given_token_whose_claims_cant_be_read_is_left_to_the_sts() {
    let sts = federation(200, &[], 900).await;
    for token in ["opaque-token", "a.not-base64!.c", "e30.bm90IGpzb24."] {
        let credentials = Federated::from_token(token).with_endpoint(sts.server.url());
        assert!(credentials.token().await.is_ok(), "{token}");
    }
    assert_eq!(
        sts.subject_tokens(),
        ["opaque-token", "a.not-base64!.c", "e30.bm90IGpzb24."]
    );
}

#[test]
fn a_given_token_is_not_shown() {
    let credentials = Federated::from_token("given-job-token");
    let debug = format!("{credentials:?}");
    assert!(!debug.contains("given-job-token"), "{debug}");
    assert!(debug.contains(r#"source: "token""#), "{debug}");
}

const UNAVAILABLE: (u16, &str) = (503, r#"{"error":"temporarily_unavailable"}"#);

#[tokio::test]
async fn an_sts_that_cant_check_the_token_right_now_is_asked_again() {
    let sts = federation(200, &[UNAVAILABLE, UNAVAILABLE], 900).await;
    let credentials = sts.github_actions();
    let started = std::time::Instant::now();
    assert_eq!(credentials.token().await.unwrap(), "access-1");
    assert!(
        started.elapsed() >= Duration::from_millis(600),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(sts.exchanges.load(Ordering::SeqCst), 3);
    assert_eq!(sts.subject_tokens(), ["job-1", "job-1", "job-1"]);
}

#[tokio::test]
async fn an_sts_that_cant_check_the_token_three_times_is_an_error() {
    let sts = federation(200, &[UNAVAILABLE, UNAVAILABLE, UNAVAILABLE], 900).await;
    let err = sts.github_actions().token().await.unwrap_err();
    assert!(err.to_string().contains("HTTP 503"), "{err}");
    assert_eq!(sts.exchanges.load(Ordering::SeqCst), 3);
    assert_no_token(&err);
}

#[tokio::test]
async fn a_job_token_the_sts_refuses_says_what_to_check() {
    let sts = federation(200, &[(400, r#"{"error":"invalid_grant"}"#)], 900).await;
    let err = sts.github_actions().token().await.unwrap_err();
    let message = err.to_string();
    assert!(message.contains("refused the CI job's token"), "{message}");
    assert!(
        message.contains("issuer, audience and conditions against the token's claims"),
        "{message}"
    );
    assert_eq!(sts.exchanges.load(Ordering::SeqCst), 1);
    assert_no_token(&err);
}

#[tokio::test]
async fn a_malformed_exchange_says_why() {
    let sts = federation(
        200,
        &[(
            400,
            r#"{"error":"invalid_request","error_description":"subject_token_type must be a JWT"}"#,
        )],
        900,
    )
    .await;
    let err = sts.github_actions().token().await.unwrap_err();
    assert!(
        err.to_string().contains("subject_token_type must be a JWT"),
        "{err}"
    );
    assert!(err.to_string().contains("HTTP 400"), "{err}");
    assert_no_token(&err);
}

#[tokio::test]
async fn a_failed_github_token_request_names_the_permission() {
    let sts = federation(403, &[], 900).await;
    let err = sts.github_actions().token().await.unwrap_err();
    let message = err.to_string();
    assert!(
        message.contains("permissions: id-token: write"),
        "{message}"
    );
    assert!(message.contains("HTTP 403"), "{message}");
    assert_eq!(sts.exchanges.load(Ordering::SeqCst), 0);
    assert_no_token(&err);
}

#[test]
fn federated_credentials_show_no_token() {
    let credentials = Federated::github_actions("https://github.invalid/idtoken", "request-token");
    let debug = format!("{credentials:?}");
    assert!(!debug.contains("request-token"), "{debug}");
    assert!(debug.contains("https://sts.origincloudapis.eu"), "{debug}");
}
