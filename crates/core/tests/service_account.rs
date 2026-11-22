//! A service account authenticated with its key's credentials file: each
//! assertion is checked by a fake security token service as the real one
//! checks it (signature, `kid`, `iss`, `sub`, `aud`, lifetime, a `jti` never
//! seen before), and the API is called with the token it answers.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use fake_server::{FakeServer, Received, Response};
use origincloud_core::{Client, Credentials, Method, Request, ServiceAccount};
use p256::ecdsa::signature::Verifier as _;
use p256::ecdsa::{Signature, SigningKey, VerifyingKey};
use p256::pkcs8::DecodePrivateKey as _;
use serde_json::{Value, json};

const KEY: &str = "//iam.origincloudapis.eu/organizations/o/serviceAccounts/s/keys/k";
const ACCOUNT: &str = "//iam.origincloudapis.eu/organizations/o/serviceAccounts/s";

/// The key the service account holds, for these tests alone.
const PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgyHvHtkzp0fjy/hHK
/yt/j0yihbwhkfpFzHsrSjh6OqOhRANCAARxfaIx32RdN3JO0GPH8ght/CFjaoE3
ao6XiaIxhBdJ6jPPWX/9086Bti4LIfP+qzNTxPn85k7f+yGc6aQoBkr1
-----END PRIVATE KEY-----
";

/// Another P-256 key, which isn't the account's.
const OTHER_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgnHo+nhqwgj8eQ5xs
s9+S+s7oa3u/bj0p7YA3NCVJldChRANCAASUI5ZTzgybmjDVq05Sbi7380jm/Llf
FY6BEN5298Ur/xKirVSlsd/InZKZ3q/WN3oY2hdAmb/mzivDJzHwpn5s
-----END PRIVATE KEY-----
";

/// A security token service that knows the account's public key, answers
/// each assertion it accepts with an access token numbered from 1 that
/// expires after `expires_in` seconds, and refuses every other one with
/// `invalid_grant`, saying why (the real one says nothing).
struct Sts {
    server: FakeServer,
    exchanged: Arc<AtomicU32>,
    refused: Arc<Mutex<Vec<String>>>,
}

async fn sts(expires_in: u64, delay: Duration) -> Sts {
    let public = *SigningKey::from_pkcs8_pem(PRIVATE_KEY)
        .unwrap()
        .verifying_key();
    let audience = Arc::new(OnceLock::<String>::new());
    let exchanged = Arc::new(AtomicU32::new(0));
    let refused = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::new(Mutex::new(HashSet::<String>::new()));
    let (aud, count, refusals) = (
        Arc::clone(&audience),
        Arc::clone(&exchanged),
        Arc::clone(&refused),
    );
    let server = FakeServer::start(move |r| {
        let checked = check(&r, &public, aud.get().unwrap(), &seen);
        let answer = match checked {
            Ok(()) => {
                let n = count.fetch_add(1, Ordering::SeqCst) + 1;
                Response::json(
                    json!({
                        "access_token": format!("token-{n}"),
                        "token_type": "Bearer",
                        "expires_in": expires_in,
                    })
                    .to_string(),
                )
            }
            Err(reason) => {
                refusals.lock().unwrap().push(reason.clone());
                Response::json(
                    json!({"error": "invalid_grant", "error_description": reason}).to_string(),
                )
                .with_status(400)
            }
        };
        async move {
            tokio::time::sleep(delay).await;
            answer
        }
    })
    .await;
    audience
        .set(server.url().trim_end_matches('/').to_owned())
        .unwrap();
    Sts {
        server,
        exchanged,
        refused,
    }
}

/// What the STS checks of a JWT bearer grant.
fn check(
    r: &Received,
    public: &VerifyingKey,
    audience: &str,
    seen: &Mutex<HashSet<String>>,
) -> Result<(), String> {
    if (r.method.as_str(), r.path.as_str()) != ("POST", "/v1/token") {
        return Err(format!("{} {}", r.method, r.path));
    }
    let form = Received {
        query: r.text(),
        ..r.clone()
    };
    let grant = form.query_values("grant_type");
    if grant != ["urn:ietf:params:oauth:grant-type:jwt-bearer"] {
        return Err(format!("grant_type {grant:?}"));
    }
    let [assertion] = form
        .query_values("assertion")
        .try_into()
        .map_err(|_| "no assertion")?;
    let parts: Vec<&str> = assertion.split('.').collect();
    let [header, claims, signature] = parts[..] else {
        return Err("not a JWS".to_owned());
    };
    let decode = |part: &str| -> Result<Value, String> {
        let bytes = URL_SAFE_NO_PAD.decode(part).map_err(|e| e.to_string())?;
        serde_json::from_slice(&bytes).map_err(|e| e.to_string())
    };
    let (header, claims) = (decode(header)?, decode(claims)?);
    let signed = format!(
        "{}.{}",
        assertion.split('.').next().unwrap(),
        assertion.split('.').nth(1).unwrap()
    );
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .ok()
        .and_then(|bytes| Signature::from_slice(&bytes).ok())
        .ok_or("no ES256 signature")?;
    public
        .verify(signed.as_bytes(), &signature)
        .map_err(|_| "the signature doesn't verify")?;
    if header["alg"] != "ES256" || header["kid"] != KEY {
        return Err(format!("header {header}"));
    }
    if claims["iss"] != ACCOUNT || claims["sub"] != ACCOUNT || claims["aud"] != audience {
        return Err(format!("claims {claims}"));
    }
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let (iat, exp) = (
        claims["iat"].as_u64().ok_or("no iat")?,
        claims["exp"].as_u64().ok_or("no exp")?,
    );
    if iat > now + 60 || exp <= now || exp - iat > 3600 {
        return Err(format!("iat {iat}, exp {exp}, now {now}"));
    }
    let jti = claims["jti"].as_str().ok_or("no jti")?;
    if jti.is_empty() || jti.len() > 128 || !seen.lock().unwrap().insert(jti.to_owned()) {
        return Err(format!("jti {jti:?} used before"));
    }
    Ok(())
}

/// A credentials file as IAM returns it, for the key `private_key` and the
/// STS at `sts_endpoint`, written to a directory of its own.
fn credentials_file(private_key: &str, sts_endpoint: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("origincloud-sdk-key-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("credentials.json");
    let file = json!({
        "type": "service_account_key",
        "name": KEY,
        "service_account": ACCOUNT,
        "algorithm": "ES256",
        "private_key": private_key,
        "sts_endpoint": sts_endpoint,
    });
    std::fs::write(&path, serde_json::to_string_pretty(&file).unwrap()).unwrap();
    path
}

fn account(sts: &Sts) -> ServiceAccount {
    let path = credentials_file(PRIVATE_KEY, sts.server.url().trim_end_matches('/'));
    ServiceAccount::from_file(path).unwrap()
}

#[tokio::test]
async fn with_only_a_credentials_file_the_api_is_called_as_the_service_account() {
    let sts = sts(900, Duration::ZERO).await;
    let authorization = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&authorization);
    let api = FakeServer::start(move |r| {
        recorded
            .lock()
            .unwrap()
            .push(r.header("authorization").unwrap_or_default().to_owned());
        async { Response::json("{}") }
    })
    .await;
    let client = Client::builder()
        .endpoint(api.url())
        .credentials(account(&sts))
        .build()
        .unwrap();

    let service = client.service("iam", false);
    for _ in 0..2 {
        let _: Value = service
            .call(Request::new(Method::GET, "/v1beta1/organizations/o"))
            .await
            .unwrap();
    }

    assert_eq!(
        *authorization.lock().unwrap(),
        ["Bearer token-1", "Bearer token-1"]
    );
    assert_eq!(sts.exchanged.load(Ordering::SeqCst), 1);
    assert!(sts.refused.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_token_about_to_expire_is_renewed_before_it_is_sent() {
    // Valid for a second past the minute's margin, then within it.
    let sts = sts(61, Duration::ZERO).await;
    let account = account(&sts);

    assert_eq!(account.token().await.unwrap(), "token-1");
    assert_eq!(account.token().await.unwrap(), "token-1");
    tokio::time::sleep(Duration::from_millis(1200)).await;
    // token-1 still has 59 seconds to live, but isn't sent any more: a new
    // assertion, with a jti of its own, is exchanged first.
    assert_eq!(account.token().await.unwrap(), "token-2");
    assert_eq!(sts.exchanged.load(Ordering::SeqCst), 2);
    assert!(sts.refused.lock().unwrap().is_empty());
}

#[tokio::test]
async fn concurrent_callers_share_one_renewal() {
    let sts = sts(900, Duration::from_millis(200)).await;
    let account = Arc::new(account(&sts));

    let callers: Vec<_> = (0..8)
        .map(|_| {
            let account = Arc::clone(&account);
            tokio::spawn(async move { account.token().await.unwrap() })
        })
        .collect();
    for caller in callers {
        assert_eq!(caller.await.unwrap(), "token-1");
    }
    assert_eq!(sts.exchanged.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn an_assertion_the_sts_refuses_names_the_key() {
    let sts = sts(900, Duration::ZERO).await;
    // Signed with a key that isn't the account's.
    let path = credentials_file(OTHER_KEY, sts.server.url().trim_end_matches('/'));
    let account = ServiceAccount::from_file(path).unwrap();

    let err = account.token().await.unwrap_err().to_string();
    assert!(
        err.contains(&format!(
            "exchanging an assertion of the service account key {KEY}"
        )),
        "{err}"
    );
    assert!(err.contains("HTTP 400"), "{err}");
    assert_eq!(
        *sts.refused.lock().unwrap(),
        ["the signature doesn't verify"]
    );
}

#[test]
fn the_names_are_read_from_the_file() {
    let path = credentials_file(PRIVATE_KEY, "https://sts.origincloudapis.eu");
    let account = ServiceAccount::from_file(&path).unwrap();
    assert_eq!(account.service_account(), ACCOUNT);
    assert_eq!(account.key(), KEY);
    let same = ServiceAccount::from_json(std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(same.key(), KEY);
}
