//! The client on `wasm32-unknown-unknown`, run by wasm-bindgen-test in
//! Node.js, whose `fetch`, `setTimeout` and `Date` stand in for a
//! browser's:
//!
//! ```sh
//! CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
//!   cargo test --target wasm32-unknown-unknown -p origincloud-core \
//!   --features service-account-wasm --test wasm
//! ```
//!
//! (`scripts/wasm.sh` runs them so.)
//!
//! There is no Tokio there: a retry that waited on Tokio's timer, or a
//! credential cache that read `SystemTime::now`, would panic.

#![cfg(target_arch = "wasm32")]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use js_sys::{Function, Reflect};
#[cfg(feature = "service-account-wasm")]
use origincloud_core::ServiceAccount;
use origincloud_core::credentials::TokenFuture;
use origincloud_core::{
    AccessToken, CallOptions, Client, ConsistencyTokenCell, ConsistencyTokenStore, Credentials,
    Error, Method, Request,
};
use serde_json::Value;
use wasm_bindgen::JsValue;
use wasm_bindgen_test::wasm_bindgen_test;

/// Credentials counting the attempts that asked for a token.
#[derive(Debug)]
struct Counting(Arc<AtomicU32>);

impl Credentials for Counting {
    fn token(&self) -> TokenFuture<'_> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok("test-token".to_owned()) })
    }
}

#[wasm_bindgen_test]
async fn a_read_that_cannot_connect_is_sent_again_after_waiting() {
    let attempts = Arc::new(AtomicU32::new(0));
    // Nothing listens on the discard port: each attempt fails to connect.
    let client = Client::builder()
        .endpoint("http://127.0.0.1:9")
        .credentials(Counting(Arc::clone(&attempts)))
        .max_attempts(3)
        .build()
        .unwrap();
    let started = js_sys::Date::now();
    let result: Result<Value, Error> = client
        .service("pvs", true)
        .call(Request::new(Method::GET, "/v1beta1/projects/p/zones/z/volumes").idempotent())
        .await;
    let waited = js_sys::Date::now() - started;

    assert!(matches!(result, Err(Error::Transport(_))), "got {result:?}");
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    // The waits before the second and third attempts are at least half of
    // 200 ms and of 400 ms.
    assert!(waited >= 300.0, "waited {waited} ms");
}

/// Replaces the global `fetch`, which reqwest calls, with one answering
/// every request with `status` and the JSON `body`, until the returned
/// [`Stub`] is dropped; [`Stub::requests`] counts what it answered.
fn stub_fetch(status: u16, body: &str) -> Stub {
    let install = Function::new_with_args(
        "status, body",
        r#"
        const original = globalThis.fetch;
        globalThis.stubbedRequests = 0;
        globalThis.fetch = (request) => {
            globalThis.stubbedRequests += 1;
            const response = new Response(body, {
                status,
                headers: { "content-type": "application/json" },
            });
            // A constructed response has no URL, which reqwest reads.
            Object.defineProperty(response, "url", { value: request.url });
            return Promise.resolve(response);
        };
        return () => { globalThis.fetch = original; };
        "#,
    );
    let restore = install
        .call2(&JsValue::NULL, &status.into(), &body.into())
        .unwrap();
    Stub(Function::from(restore))
}

struct Stub(Function);

impl Stub {
    fn requests(&self) -> f64 {
        Reflect::get(&js_sys::global(), &"stubbedRequests".into())
            .unwrap()
            .as_f64()
            .unwrap()
    }
}

impl Drop for Stub {
    fn drop(&mut self) {
        self.0.call0(&JsValue::NULL).unwrap();
    }
}

#[wasm_bindgen_test]
async fn a_retry_delay_longer_than_a_browser_timer_still_waits() {
    // setTimeout fires at once for more than 2^31 - 1 ms (about 24.8
    // days), and gloo-timers panics past u32: ten years must wait the
    // longest a timer can, not retry at once.
    let stub = stub_fetch(
        503,
        r#"{"error":{"code":503,"status":"UNAVAILABLE","message":"down","details":[
            {"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"SERVICE_UNAVAILABLE","domain":"origincloudapis.eu"},
            {"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"315360000s"}]}}"#,
    );
    let client = Client::builder()
        .endpoint("http://127.0.0.1:9")
        .credentials(AccessToken::new("test-token"))
        .max_attempts(2)
        .build()
        .unwrap();
    let (call, waiting) = futures_util::future::abortable(async move {
        let _: Result<Value, Error> = client
            .service("pvs", true)
            .call(Request::new(Method::GET, "/v1beta1/projects/p").idempotent())
            .await;
    });
    wasm_bindgen_futures::spawn_local(async {
        let _ = call.await;
    });
    gloo_timers::future::sleep(Duration::from_millis(200)).await;
    let requests = stub.requests();
    // Dropping the call clears its timer, which would outlive the test.
    waiting.abort();

    assert_eq!(requests, 1.0);
}

#[cfg(feature = "service-account-wasm")]
#[wasm_bindgen_test]
async fn a_service_account_s_token_is_cached_by_the_javascript_clock() {
    // The STS answers with a token for five minutes.
    let stub = stub_fetch(200, r#"{"access_token":"granted","expires_in":300}"#);
    let account = ServiceAccount::from_json(
        serde_json::json!({
            "type": "service_account_key",
            "name": "//iam.origincloudapis.eu/organizations/o/serviceAccounts/s/keys/k",
            "service_account": "//iam.origincloudapis.eu/organizations/o/serviceAccounts/s",
            "algorithm": "ES256",
            "private_key": "-----BEGIN PRIVATE KEY-----\nMIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgyHvHtkzp0fjy/hHK\n/yt/j0yihbwhkfpFzHsrSjh6OqOhRANCAARxfaIx32RdN3JO0GPH8ght/CFjaoE3\nao6XiaIxhBdJ6jPPWX/9086Bti4LIfP+qzNTxPn85k7f+yGc6aQoBkr1\n-----END PRIVATE KEY-----\n",
            "sts_endpoint": "https://sts.origincloudapis.eu",
        })
        .to_string(),
    )
    .unwrap();

    // The first token is requested, and its expiry set from the clock; the
    // second is the first, which the clock says is still valid.
    assert_eq!(account.token().await.unwrap(), "granted");
    assert_eq!(account.token().await.unwrap(), "granted");
    assert_eq!(stub.requests(), 1.0);
}

#[wasm_bindgen_test]
fn without_credentials_a_client_has_none() {
    let built = Client::builder().build();
    assert!(matches!(built, Err(Error::NoCredentials)), "got {built:?}");
}

#[wasm_bindgen_test]
fn a_client_can_be_shared_by_reactive_state() {
    // Leptos keeps memo values `Send + Sync` in a browser too.
    fn shareable<T: Send + Sync>() {}
    shareable::<Client>();
    shareable::<origincloud_core::Service>();
    #[cfg(feature = "service-account-wasm")]
    shareable::<ServiceAccount>();
    shareable::<ConsistencyTokenCell>();
    shareable::<Arc<dyn ConsistencyTokenStore>>();
    shareable::<CallOptions>();
}

/// Replaces the global `fetch`, which reqwest calls, with one that records
/// each request's headers in `globalThis.sent` and answers with what
/// `answer(headers)`, a JavaScript expression, makes of them: a status and
/// a body, with the token `v1.answered` when the status is 200. It returns
/// the function putting the original back.
fn stub_fetch_by_headers(answer: &str) -> Function {
    let install = Function::new_no_args(&format!(
        r#"
        const original = globalThis.fetch;
        globalThis.sent = [];
        globalThis.fetch = (request) => {{
            const headers = Object.fromEntries(request.headers.entries());
            globalThis.sent.push(headers);
            const [status, body] = (({answer}))(headers);
            const responseHeaders = {{ "content-type": "application/json" }};
            if (status === 200) responseHeaders["origin-consistency-token"] = "v1.answered";
            const response = new Response(body, {{ status, headers: responseHeaders }});
            Object.defineProperty(response, "url", {{ value: request.url }});
            return Promise.resolve(response);
        }};
        return () => {{ globalThis.fetch = original; }};
        "#
    ));
    Function::from(install.call0(&JsValue::NULL).unwrap())
}

/// The headers of each request the stub got, as JSON.
fn sent() -> Value {
    let sent = Reflect::get(&js_sys::global(), &"sent".into()).unwrap();
    serde_json::from_str(&String::from(js_sys::JSON::stringify(&sent).unwrap())).unwrap()
}

fn stubbed_client() -> Client {
    Client::builder()
        .endpoint("http://api.test")
        .credentials(AccessToken::new("test-token"))
        .build()
        .unwrap()
}

/// An anonymous call carries only `Accept`, which a browser sends to
/// another origin without a CORS preflight; an authenticated one its
/// credentials, `User-Agent` and consistency token.
#[wasm_bindgen_test]
async fn an_anonymous_call_needs_no_preflight() {
    let restore = stub_fetch_by_headers("() => [200, '{}']");
    let client = stubbed_client();
    client.set_consistency_token("v1.held");
    let billing = client.service("billing", false);

    let anonymous: Result<Value, Error> = billing
        .call(
            Request::new(Method::GET, "/v1beta1/legalDocuments")
                .idempotent()
                .unauthenticated(),
        )
        .await;
    let authenticated: Result<Value, Error> = billing
        .call(Request::new(Method::GET, "/v1beta1/legalDocuments").idempotent())
        .await;
    let sent = sent();
    restore.call0(&JsValue::NULL).unwrap();

    anonymous.unwrap();
    authenticated.unwrap();
    assert_eq!(sent[0], serde_json::json!({"accept": "application/json"}));
    assert_eq!(sent[1]["authorization"], "Bearer test-token");
    assert_eq!(sent[1]["origin-consistency-token"], "v1.held");
    assert!(sent[1]["user-agent"].is_string(), "{}", sent[1]);
}

/// A token the API refuses to read is dropped, and the call sent again
/// once without it.
#[wasm_bindgen_test]
async fn a_refused_token_is_dropped_in_a_browser_too() {
    let restore = stub_fetch_by_headers(
        r#"(h) => h["origin-consistency-token"] === "v1.stale"
            ? [400, JSON.stringify({error: {code: 400, status: "INVALID_ARGUMENT",
                message: "the Origin-Consistency-Token header must be a token a response of this API carried"}})]
            : [200, "{}"]"#,
    );
    let store = Arc::new(ConsistencyTokenCell::new());
    store.set("v1.stale".to_owned());
    let client = Client::builder()
        .endpoint("http://api.test")
        .credentials(AccessToken::new("test-token"))
        .consistency_token_store(store.clone())
        .build()
        .unwrap();

    let result: Result<Value, Error> = client
        .service("pvs", true)
        .with_options(CallOptions::new().max_attempts(1))
        .call(Request::new(
            Method::POST,
            "/v1beta1/projects/p/zones/z/volumes",
        ))
        .await;
    let sent = sent();
    restore.call0(&JsValue::NULL).unwrap();

    result.unwrap();
    assert_eq!(sent.as_array().unwrap().len(), 2);
    assert_eq!(sent[0]["origin-consistency-token"], "v1.stale");
    assert!(
        sent[1].get("origin-consistency-token").is_none(),
        "{}",
        sent[1]
    );
    assert_eq!(store.get(), "v1.answered");
}
