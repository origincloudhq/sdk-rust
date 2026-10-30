//! Calls, retries, errors, read-your-writes and operations, against a
//! local server.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fake_server::{FakeServer, Received, Response};
use origincloud_core::{
    AccessToken, CONSISTENCY_TOKEN_HEADER, Client, Error, Method, Operation, Request, Service,
    longrunning,
};
use serde_json::{Value, json};

const UNAVAILABLE: &str = r#"{"error":{"code":503,"status":"UNAVAILABLE","message":"the read model is behind","details":[
    {"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"SERVICE_UNAVAILABLE","domain":"origincloudapis.eu"},
    {"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"0.010s"}]}}"#;

const BAD_REQUEST: &str = r#"{"error":{"code":400,"status":"INVALID_ARGUMENT","message":"invalid input on field volume.sizeGib: must be at least 1","details":[
    {"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"BAD_REQUEST","domain":"origincloudapis.eu"},
    {"@type":"type.googleapis.com/google.rpc.RequestInfo","requestId":"4bf92f3577b34da6"},
    {"@type":"type.googleapis.com/google.rpc.BadRequest","fieldViolations":[{"field":"volume.sizeGib","description":"must be at least 1"}]}]}}"#;

const OPERATION_NAME: &str = "projects/p/zones/z/operations/op-1";

/// A client sending every service's calls to `server`.
fn client(server: &FakeServer, max_attempts: u32) -> Client {
    Client::builder()
        .endpoint(server.url())
        .credentials(AccessToken::new("test-token"))
        .user_agent("my-tool/1.2")
        .max_attempts(max_attempts)
        .build()
        .unwrap()
}

fn service(server: &FakeServer) -> Service {
    client(server, 5).service("pvs", true)
}

async fn get(service: &Service, path: &str) -> Result<Value, Error> {
    service
        .call(Request::new(Method::GET, path).idempotent())
        .await
}

#[tokio::test]
async fn a_request_carries_its_credentials_agent_query_and_body() {
    let seen = Arc::new(Mutex::new(None::<Received>));
    let recorded = Arc::clone(&seen);
    let server = FakeServer::start(move |r| {
        *recorded.lock().unwrap() = Some(r);
        async { Response::json("{}") }
    })
    .await;

    let _: Value = service(&server)
        .call(
            Request::new(Method::POST, "/v1beta1/projects/p/zones/z/volumes")
                .with_query(vec![("pageSize".to_owned(), "10".to_owned())])
                .with_body(br#"{"sizeGib":5}"#.to_vec()),
        )
        .await
        .unwrap();

    let got = seen.lock().unwrap().clone().unwrap();
    assert_eq!(got.header("authorization"), Some("Bearer test-token"));
    assert!(
        got.header("user-agent")
            .unwrap()
            .starts_with("my-tool/1.2 origincloud-sdk-rust/"),
        "{:?}",
        got.header("user-agent")
    );
    assert_eq!(got.header("content-type"), Some("application/json"));
    assert_eq!(got.text(), r#"{"sizeGib":5}"#);
    assert_eq!(got.method, "POST");
    assert_eq!(got.path, "/v1beta1/projects/p/zones/z/volumes");
    assert_eq!(got.query, "pageSize=10");
}

#[tokio::test]
async fn an_idempotent_call_is_retried_while_the_answer_is_retryable() {
    let calls = Arc::new(AtomicU32::new(0));
    let counted = Arc::clone(&calls);
    let server = FakeServer::start(move |_| {
        let n = counted.fetch_add(1, Ordering::SeqCst) + 1;
        async move {
            if n < 3 {
                Response::json(UNAVAILABLE).with_status(503)
            } else {
                Response::json("{}")
            }
        }
    })
    .await;

    get(&service(&server), "/x").await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn a_call_that_isnt_idempotent_is_sent_once() {
    let calls = Arc::new(AtomicU32::new(0));
    let counted = Arc::clone(&calls);
    let server = FakeServer::start(move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
        async { Response::json(UNAVAILABLE).with_status(503) }
    })
    .await;

    let err = service(&server)
        .call::<Value>(Request::new(Method::POST, "/x"))
        .await
        .unwrap_err();
    assert_eq!(err.reason(), Some("SERVICE_UNAVAILABLE"));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn retries_stop_at_the_most_attempts() {
    let calls = Arc::new(AtomicU32::new(0));
    let counted = Arc::clone(&calls);
    let server = FakeServer::start(move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
        async { Response::json(UNAVAILABLE).with_status(503) }
    })
    .await;

    let service = client(&server, 2).service("pvs", true);
    let err = get(&service, "/x").await.unwrap_err();
    assert!(err.is_retryable(), "{err}");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn an_error_that_isnt_retryable_is_returned_at_once() {
    let calls = Arc::new(AtomicU32::new(0));
    let counted = Arc::clone(&calls);
    let server = FakeServer::start(move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
        async { Response::json(BAD_REQUEST).with_status(400) }
    })
    .await;

    let err = get(&service(&server), "/x").await.unwrap_err();
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let api = err.api().expect("an API error");
    assert_eq!(api.http_status, Some(400));
    assert_eq!(api.status, "INVALID_ARGUMENT");
    assert_eq!(api.reason, "BAD_REQUEST");
    assert_eq!(api.request_id, "4bf92f3577b34da6");
    assert_eq!(api.field_violations.len(), 1);
    assert_eq!(api.field_violations[0].field, "volume.sizeGib");
    assert_eq!(
        err.to_string(),
        "origincloud: BAD_REQUEST: invalid input on field volume.sizeGib: must be at least 1 (request 4bf92f3577b34da6)"
    );
}

#[tokio::test]
async fn a_proxy_error_without_a_status_is_retried() {
    let calls = Arc::new(AtomicU32::new(0));
    let counted = Arc::clone(&calls);
    let server = FakeServer::start(move |_| {
        let n = counted.fetch_add(1, Ordering::SeqCst) + 1;
        async move {
            if n == 1 {
                Response::json("<html>bad gateway</html>").with_status(502)
            } else {
                Response::json("{}")
            }
        }
    })
    .await;

    get(&service(&server), "/x").await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn a_connection_that_fails_is_retried_for_an_idempotent_call() {
    // Nothing listens at the endpoint: every attempt fails to connect.
    let server = FakeServer::start(|_| async { Response::json("{}") }).await;
    let url = server.url();
    drop(server);
    tokio::time::sleep(Duration::from_millis(50)).await;
    let client = Client::builder()
        .endpoint(url)
        .credentials(AccessToken::new("t"))
        .max_attempts(2)
        .build()
        .unwrap();

    let started = Instant::now();
    let err = get(&client.service("pvs", true), "/x").await.unwrap_err();
    assert!(matches!(err, Error::Transport(_)), "{err}");
    // One retry, after at least half of the first delay.
    assert!(
        started.elapsed() >= Duration::from_millis(100),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn the_consistency_token_is_kept_and_sent_back() {
    let presented = Arc::new(Mutex::new(Vec::<String>::new()));
    let log = Arc::clone(&presented);
    let server = FakeServer::start(move |r| {
        log.lock().unwrap().push(
            r.header(CONSISTENCY_TOKEN_HEADER)
                .unwrap_or_default()
                .to_owned(),
        );
        async move {
            match r.path.as_str() {
                "/mutate" => {
                    Response::json("{}").with_header(CONSISTENCY_TOKEN_HEADER, "v1.after-mutation")
                }
                // A failed mutation may have applied a command all the same.
                "/fail" => Response::json(BAD_REQUEST)
                    .with_status(400)
                    .with_header(CONSISTENCY_TOKEN_HEADER, "v1.after-failure"),
                _ => Response::json("{}"),
            }
        }
    })
    .await;
    let client = client(&server, 1);
    let service = client.service("pvs", true);

    for path in ["/read", "/mutate", "/read", "/fail", "/read"] {
        let _ = get(&service, path).await;
    }

    assert_eq!(
        *presented.lock().unwrap(),
        [
            "",
            "",
            "v1.after-mutation",
            "v1.after-mutation",
            "v1.after-failure"
        ]
    );
    assert_eq!(client.consistency_token(), "v1.after-failure");
}

#[tokio::test]
async fn a_name_of_the_wrong_form_is_refused_before_sending() {
    let err =
        origincloud_core::path_value("name", "vol-1", "projects/*/zones/*/volumes/*").unwrap_err();
    assert!(matches!(err, Error::InvalidRequest(_)));
    assert!(
        err.to_string()
            .contains("name must have the form projects/*/zones/*/volumes/*"),
        "{err}"
    );
}

fn operation(done: bool, response: Option<Value>) -> Value {
    let mut op = json!({ "name": OPERATION_NAME });
    if done {
        op["done"] = json!(true);
    }
    if let Some(response) = response {
        op["response"] = response;
    }
    op
}

fn running(service: &Service) -> Operation<Value> {
    Operation::new(
        service.clone(),
        serde_json::from_value(operation(false, None)).unwrap(),
        "x.Volume",
    )
}

#[tokio::test]
async fn wait_waits_until_the_operation_is_done() {
    let waits = Arc::new(AtomicU32::new(0));
    let counted = Arc::clone(&waits);
    let server = FakeServer::start(move |r| {
        assert_eq!(r.method, "POST");
        assert_eq!(r.path, format!("/v1beta1/{OPERATION_NAME}:wait"));
        assert_eq!(r.text(), r#"{"timeout":"60s"}"#);
        let n = counted.fetch_add(1, Ordering::SeqCst) + 1;
        async move {
            if n < 3 {
                Response::json(operation(false, None).to_string())
            } else {
                let result = json!({"@type": "type.googleapis.com/x.Volume", "name": "projects/p/zones/z/volumes/vol-1"});
                Response::json(operation(true, Some(result)).to_string())
                    .with_header(CONSISTENCY_TOKEN_HEADER, "v1.done")
            }
        }
    })
    .await;
    let client = client(&server, 5);
    let mut op = running(&client.service("pvs", true));

    let got = op.wait().await.unwrap();
    assert_eq!(got["name"], "projects/p/zones/z/volumes/vol-1");
    assert_eq!(waits.load(Ordering::SeqCst), 3);
    assert!(op.done());
    assert_eq!(client.consistency_token(), "v1.done");
}

#[tokio::test]
async fn a_failed_operation_returns_its_error() {
    let server = FakeServer::start(|_| async {
        Response::json(
            json!({
                "name": OPERATION_NAME,
                "done": true,
                "error": {
                    "code": 13,
                    "message": "the storage couldn't be created",
                    "details": [{"@type": "type.googleapis.com/google.rpc.ErrorInfo", "reason": "INTERNAL_SERVER_ERROR", "domain": "origincloudapis.eu"}],
                },
            })
            .to_string(),
        )
    })
    .await;
    let mut op = running(&service(&server));

    let err = op.wait().await.unwrap_err();
    let api = err.api().expect("an API error");
    assert_eq!(api.status, "INTERNAL");
    assert_eq!(api.reason, "INTERNAL_SERVER_ERROR");
    assert_eq!(api.message, "the storage couldn't be created");
    assert_eq!(api.http_status, None);
}

#[tokio::test]
async fn an_operation_answered_done_is_not_waited_for() {
    let server =
        FakeServer::start(|r| async move { panic!("unexpected call {} {}", r.method, r.path) })
            .await;
    let done: longrunning::Operation = serde_json::from_value(operation(
        true,
        Some(json!({"@type": "type.googleapis.com/google.protobuf.Empty", "value": {}})),
    ))
    .unwrap();
    let mut op: Operation<origincloud_core::protobuf::Empty> =
        Operation::new(service(&server), done, "google.protobuf.Empty");
    op.wait().await.unwrap();
}

#[tokio::test]
async fn the_result_of_an_operation_that_isnt_done() {
    let server = FakeServer::start(|_| async { Response::json("{}") }).await;
    let op = running(&service(&server));
    assert!(matches!(op.result(), Err(Error::NotDone(_))));
}

#[tokio::test]
async fn a_result_of_another_type_is_refused() {
    let server = FakeServer::start(|_| async { Response::json("{}") }).await;
    let done = serde_json::from_value(operation(
        true,
        Some(json!({"@type": "type.googleapis.com/x.Snapshot", "name": "n"})),
    ))
    .unwrap();
    let op: Operation<Value> = Operation::new(service(&server), done, "x.Volume");
    let err = op.result().unwrap_err();
    assert!(
        err.to_string().contains("is a x.Snapshot, not a x.Volume"),
        "{err}"
    );
}

fn done_volume() -> String {
    let result = json!({"@type": "type.googleapis.com/x.Volume", "name": "projects/p/zones/z/volumes/vol-1"});
    operation(true, Some(result)).to_string()
}

#[tokio::test]
async fn a_wait_call_that_times_out_counts_as_not_done() {
    let waits = Arc::new(AtomicU32::new(0));
    let counted = Arc::clone(&waits);
    let server = FakeServer::start(move |_| {
        let n = counted.fetch_add(1, Ordering::SeqCst) + 1;
        async move {
            if n < 3 {
                // Longer than the HTTP client waits.
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            Response::json(done_volume())
        }
    })
    .await;
    // The HTTP client's own timeout, shorter than the minute the API is
    // asked to wait, and no retries: a timeout must not use any up.
    let http = reqwest::Client::builder()
        .timeout(Duration::from_millis(300))
        .build()
        .unwrap();
    let client = Client::builder()
        .endpoint(server.url())
        .credentials(AccessToken::new("t"))
        .http_client(http)
        .max_attempts(1)
        .build()
        .unwrap();
    let mut op = running(&client.service("pvs", true));

    let got = op.wait().await.unwrap();
    assert_eq!(got["name"], "projects/p/zones/z/volumes/vol-1");
    assert_eq!(waits.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn the_wait_per_call_is_configurable_and_kept_below_the_timeout() {
    let bodies = Arc::new(Mutex::new(Vec::<String>::new()));
    let log = Arc::clone(&bodies);
    let server = FakeServer::start(move |r| {
        log.lock().unwrap().push(r.text());
        async { Response::json(done_volume()) }
    })
    .await;
    let cases: [(Option<u64>, Option<u64>, &str); 5] = [
        (None, None, "60s"),
        (Some(10_000), None, "10s"),
        (None, Some(5_000), "4s"),
        (Some(10_000), Some(3_000), "2s"),
        (None, Some(1_500), "1s"),
    ];
    for (wait, timeout, _) in cases {
        let mut builder = Client::builder()
            .endpoint(server.url())
            .credentials(AccessToken::new("t"));
        if let Some(ms) = wait {
            builder = builder.operation_wait(Duration::from_millis(ms));
        }
        if let Some(ms) = timeout {
            builder = builder.timeout(Duration::from_millis(ms));
        }
        let client = builder.build().unwrap();
        running(&client.service("pvs", true)).wait().await.unwrap();
    }
    let want: Vec<String> = cases
        .iter()
        .map(|(_, _, w)| format!(r#"{{"timeout":"{w}"}}"#))
        .collect();
    assert_eq!(*bodies.lock().unwrap(), want);
}

#[tokio::test]
async fn an_attempt_longer_than_the_timeout_fails_and_is_retried() {
    let calls = Arc::new(AtomicU32::new(0));
    let counted = Arc::clone(&calls);
    let server = FakeServer::start(move |_| {
        let n = counted.fetch_add(1, Ordering::SeqCst) + 1;
        async move {
            if n == 1 {
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            Response::json("{}")
        }
    })
    .await;
    let client = Client::builder()
        .endpoint(server.url())
        .credentials(AccessToken::new("t"))
        .timeout(Duration::from_millis(300))
        .build()
        .unwrap();
    get(&client.service("pvs", true), "/x").await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
