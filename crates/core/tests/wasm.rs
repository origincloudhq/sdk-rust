//! The client on `wasm32-unknown-unknown`, run by wasm-bindgen-test in
//! Node.js, whose `fetch`, `setTimeout` and `Date` stand in for a
//! browser's:
//!
//! ```sh
//! CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER=wasm-bindgen-test-runner \
//!   cargo test --target wasm32-unknown-unknown -p origincloud-core --test wasm
//! ```
//!
//! There is no Tokio there: a retry that waited on Tokio's timer, or a
//! credential cache that read `SystemTime::now`, would panic.

#![cfg(target_arch = "wasm32")]

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use js_sys::{Function, Reflect};
use origincloud_core::credentials::TokenFuture;
use origincloud_core::{Client, Credentials, Error, Method, Request, ServiceAccount};
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

#[wasm_bindgen_test]
async fn a_service_account_token_is_cached_by_the_javascript_clock() {
    // Answers every fetch with a token for five minutes, counting them, in
    // place of the identity provider; reqwest calls the global `fetch`.
    let install = Function::new_no_args(
        r#"
        const original = globalThis.fetch;
        globalThis.tokenRequests = 0;
        globalThis.fetch = (request) => {
            globalThis.tokenRequests += 1;
            const response = new Response(
                '{"access_token":"granted","expires_in":300}',
                { status: 200, headers: { "content-type": "application/json" } },
            );
            // A constructed response has no URL, which reqwest reads.
            Object.defineProperty(response, "url", { value: request.url });
            return Promise.resolve(response);
        };
        return () => { globalThis.fetch = original; };
        "#,
    );
    let restore = Function::from(install.call0(&JsValue::NULL).unwrap());
    let account = ServiceAccount::new("client", "secret");

    // The first token is requested, and its expiry set from the clock; the
    // second is the first, which the clock says is still valid.
    let first = account.token().await;
    let second = account.token().await;
    let requests = Reflect::get(&js_sys::global(), &"tokenRequests".into()).unwrap();
    restore.call0(&JsValue::NULL).unwrap();

    assert_eq!(first.unwrap(), "granted");
    assert_eq!(second.unwrap(), "granted");
    assert_eq!(requests.as_f64(), Some(1.0));
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
    shareable::<ServiceAccount>();
}
