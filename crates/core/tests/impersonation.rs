//! Impersonation: the caller's token is exchanged at a fake security token
//! service for a service account's, as the real one takes the request (RFC
//! 8693 token exchange, the account named in `service_account`), and the
//! API is called with the account's token, never with the caller's.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fake_server::{FakeServer, Received, Response};
use origincloud_core::credentials::TokenFuture;
use origincloud_core::{AccessToken, Client, Credentials, Error, Impersonated, Method, Request};
use serde_json::{Value, json};

const ACCOUNT: &str = "//iam.origincloudapis.eu/organizations/o/serviceAccounts/deployer";
const OTHER_ACCOUNT: &str = "//iam.origincloudapis.eu/organizations/o/serviceAccounts/other";

/// A security token service that lets the person whose tokens start with
/// `person-` impersonate `ACCOUNT` alone. It answers each exchange it takes
/// with the account's token numbered from 1, expiring after `expires_in`
/// seconds, and refuses every other one with `invalid_grant`, as the real
/// one refuses a caller without the binding.
struct Sts {
    server: FakeServer,
    exchanged: Arc<AtomicU32>,
    forms: Arc<Mutex<Vec<Form>>>,
}

/// One token request's form fields.
type Form = Vec<(String, String)>;

async fn sts(expires_in: u64) -> Sts {
    let exchanged = Arc::new(AtomicU32::new(0));
    let forms = Arc::new(Mutex::new(Vec::new()));
    let (count, seen) = (Arc::clone(&exchanged), Arc::clone(&forms));
    let server = FakeServer::start(move |r: Received| {
        assert_eq!((r.method.as_str(), r.path.as_str()), ("POST", "/v1/token"));
        assert_eq!(
            r.header("content-type"),
            Some("application/x-www-form-urlencoded")
        );
        let form = Received {
            query: r.text(),
            ..r
        };
        seen.lock().unwrap().push(form.query_pairs());
        let one = |name: &str| -> String {
            let values = form.query_values(name);
            match values.as_slice() {
                [value] => value.clone(),
                _ => String::new(),
            }
        };
        let bound =
            one("service_account") == ACCOUNT && one("subject_token").starts_with("person-");
        let answer = if bound {
            let n = count.fetch_add(1, Ordering::SeqCst) + 1;
            Response::json(
                json!({
                    "access_token": format!("account-{n}"),
                    "issued_token_type": "urn:ietf:params:oauth:token-type:access_token",
                    "token_type": "Bearer",
                    "expires_in": expires_in,
                })
                .to_string(),
            )
        } else {
            Response::json(json!({"error": "invalid_grant"}).to_string()).with_status(400)
        };
        async move { answer }
    })
    .await;
    Sts {
        server,
        exchanged,
        forms,
    }
}

impl Sts {
    fn url(&self) -> String {
        self.server.url().trim_end_matches('/').to_owned()
    }

    fn subjects(&self) -> Vec<String> {
        self.forms
            .lock()
            .unwrap()
            .iter()
            .map(|form| {
                form.iter()
                    .find(|(k, _)| k == "subject_token")
                    .map(|(_, v)| v.clone())
                    .unwrap_or_default()
            })
            .collect()
    }
}

/// An API that records the `Authorization` header of every call.
async fn api() -> (FakeServer, Arc<Mutex<Vec<String>>>) {
    let authorization = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&authorization);
    let server = FakeServer::start(move |r| {
        recorded
            .lock()
            .unwrap()
            .push(r.header("authorization").unwrap_or_default().to_owned());
        async { Response::json("{}") }
    })
    .await;
    (server, authorization)
}

/// A person's credentials whose token changes at each request, as a
/// renewed login's does: `person-1`, `person-2`, …
#[derive(Debug, Default)]
struct Person(AtomicU32);

impl Credentials for Person {
    fn token(&self) -> TokenFuture<'_> {
        let n = self.0.fetch_add(1, Ordering::SeqCst) + 1;
        Box::pin(async move { Ok(format!("person-{n}")) })
    }
}

fn impersonated(sts: &Sts, base: impl Credentials + 'static, account: &str) -> Impersonated {
    Impersonated::new(Arc::new(base), account)
        .unwrap()
        .with_endpoint(sts.url())
}

#[tokio::test]
async fn the_api_is_called_with_the_account_s_token() {
    let sts = sts(300).await;
    let (api, authorization) = api().await;
    let client = Client::builder()
        .endpoint(api.url())
        .credentials(impersonated(&sts, AccessToken::new("person-1"), ACCOUNT))
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
        ["Bearer account-1", "Bearer account-1"]
    );
    assert_eq!(sts.exchanged.load(Ordering::SeqCst), 1);
    // The exchange the STS takes directly: the caller's token as an access
    // token, the account by its full name, no actor and no audience (the
    // API's is the only one it issues an impersonated token for).
    let mut form = sts.forms.lock().unwrap()[0].clone();
    form.sort();
    let expected: Vec<(String, String)> = [
        (
            "grant_type",
            "urn:ietf:params:oauth:grant-type:token-exchange",
        ),
        ("service_account", ACCOUNT),
        ("subject_token", "person-1"),
        (
            "subject_token_type",
            "urn:ietf:params:oauth:token-type:access_token",
        ),
    ]
    .iter()
    .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
    .collect();
    assert_eq!(form, expected);
}

#[tokio::test]
async fn the_account_s_token_is_renewed_before_it_expires() {
    // Valid for a second past the minute's margin, then within it.
    let sts = sts(61).await;
    let account = impersonated(&sts, Person::default(), ACCOUNT);

    assert_eq!(account.token().await.unwrap(), "account-1");
    assert_eq!(account.token().await.unwrap(), "account-1");
    tokio::time::sleep(Duration::from_millis(1200)).await;
    // account-1 still has 59 seconds to live, but isn't sent any more: the
    // caller's current token is exchanged again first.
    assert_eq!(account.token().await.unwrap(), "account-2");
    assert_eq!(sts.exchanged.load(Ordering::SeqCst), 2);
    assert_eq!(sts.subjects(), ["person-1", "person-2"]);
}

#[tokio::test]
async fn concurrent_callers_share_one_exchange() {
    let sts = sts(300).await;
    let account = Arc::new(impersonated(&sts, Person::default(), ACCOUNT));

    let callers: Vec<_> = (0..8)
        .map(|_| {
            let account = Arc::clone(&account);
            tokio::spawn(async move { account.token().await.unwrap() })
        })
        .collect();
    for caller in callers {
        assert_eq!(caller.await.unwrap(), "account-1");
    }
    assert_eq!(sts.exchanged.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_refused_exchange_is_an_error_naming_the_account_and_nothing_is_sent() {
    let sts = sts(300).await;
    let (api, authorization) = api().await;
    let client = Client::builder()
        .endpoint(api.url())
        .max_attempts(1)
        .credentials(impersonated(
            &sts,
            AccessToken::new("person-1"),
            OTHER_ACCOUNT,
        ))
        .build()
        .unwrap();

    let err = client
        .service("iam", false)
        .call::<Value>(Request::new(Method::GET, "/v1beta1/organizations/o"))
        .await
        .unwrap_err();

    let Error::Credentials(message) = &err else {
        panic!("{err:?}");
    };
    assert!(message.contains(OTHER_ACCOUNT), "{message}");
    assert!(message.contains("invalid_grant"), "{message}");
    assert!(
        message.contains("roles/iam.service-account-token-creator"),
        "{message}"
    );
    // The caller's own token is never sent in the account's place.
    assert!(authorization.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_refusal_is_asked_again_not_remembered_as_a_token() {
    let sts = sts(300).await;
    // A token the STS doesn't take as a bound person's.
    let account = impersonated(&sts, AccessToken::new("robot-1"), ACCOUNT);
    for _ in 0..2 {
        assert!(account.token().await.is_err());
    }
    assert_eq!(sts.subjects(), ["robot-1", "robot-1"]);
}

#[tokio::test]
async fn the_caller_s_token_never_travels_in_the_clear() {
    let account = Impersonated::new(Arc::new(AccessToken::new("person-1")), ACCOUNT)
        .unwrap()
        .with_endpoint("http://sts.example.com");
    let err = account.token().await.unwrap_err().to_string();
    assert!(err.contains("not an https URL"), "{err}");
    assert!(err.contains(ACCOUNT), "{err}");
}

#[test]
fn a_name_that_isn_t_a_service_account_s_is_refused() {
    for name in [
        "deployer",
        "organizations/o/serviceAccounts/deployer",
        "//iam.origincloudapis.eu/users/u",
        "//iam.origincloudapis.eu/organizations/o",
        "//pvs.origincloudapis.eu/organizations/o/serviceAccounts/deployer",
    ] {
        let refused = Impersonated::new(Arc::new(AccessToken::new("t")), name);
        assert!(matches!(refused, Err(Error::Config(_))), "{name:?}");
        let built = Client::builder()
            .credentials(AccessToken::new("t"))
            .impersonate_service_account(name)
            .build();
        assert!(matches!(built, Err(Error::Config(_))), "{name:?}");
    }
}

#[test]
fn the_builder_wraps_the_credentials_and_an_empty_name_impersonates_no_one() {
    let client = Client::builder()
        .credentials(AccessToken::new("t"))
        .impersonate_service_account(ACCOUNT)
        .build()
        .unwrap();
    let debug = format!("{client:?}");
    assert!(debug.contains("Impersonated"), "{debug}");
    assert!(debug.contains(ACCOUNT), "{debug}");
    assert!(debug.contains("AccessToken"), "{debug}");

    let client = Client::builder()
        .credentials(AccessToken::new("t"))
        .impersonate_service_account("")
        .build()
        .unwrap();
    assert!(!format!("{client:?}").contains("Impersonated"));
}
