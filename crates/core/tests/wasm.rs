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
use origincloud_core::{AccessToken, Client, Credentials, Error, Method, Request};
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
async fn a_service_account_token_is_cached_by_the_javascript_clock() {
    // The identity provider answers with a token for five minutes.
    let stub = stub_fetch(200, r#"{"access_token":"granted","expires_in":300}"#);
    let account = ServiceAccount::new("client", "secret");

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
}
