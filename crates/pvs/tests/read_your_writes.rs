//! A read right after a write sees it, against a front door whose read
//! model lags behind the writes.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fake_server::{FakeServer, Received, Response};
use origincloud_core::{AccessToken, CONSISTENCY_TOKEN_HEADER, Client};
use origincloud_pvs::v1beta1::{
    CreateVolumeRequest, GetVolumeRequest, Volume, VolumeClient, volume,
};
use serde_json::{Value, json};

const ZONE: &str = "projects/p/zones/z";

/// A front door whose read model trails each write by `lag`, as the API's
/// does: a read presenting a token waits up to `bound` for the read model
/// to reach it, then answers UNAVAILABLE; a read without one answers what
/// the read model has, NOT_FOUND for a volume it hasn't seen yet.
struct LaggingFrontDoor {
    lag: Duration,
    bound: Duration,
    /// When each write, by position from 1, reaches the read model.
    visible: Mutex<Vec<Instant>>,
}

impl LaggingFrontDoor {
    /// The position the read model has reached.
    fn read_model(&self) -> usize {
        let now = Instant::now();
        self.visible
            .lock()
            .unwrap()
            .iter()
            .take_while(|t| **t <= now)
            .count()
    }

    async fn answer(&self, r: Received) -> Response {
        let presented = position(r.header(CONSISTENCY_TOKEN_HEADER).unwrap_or_default());
        let volumes = format!("/v1beta1/{ZONE}/volumes");
        match (r.method.as_str(), r.path.as_str()) {
            ("POST", path) if path == volumes => {
                let body: Value = serde_json::from_slice(&r.body).unwrap();
                assert_eq!(body["sizeGib"], 1, "{body}");
                assert!(
                    !r.query_values("requestId").is_empty(),
                    "no request id: {r:?}"
                );
                let written = {
                    let mut visible = self.visible.lock().unwrap();
                    visible.push(Instant::now() + self.lag);
                    visible.len()
                };
                Response::json(json!({ "name": operation_name(written) }).to_string())
                    .with_header(CONSISTENCY_TOKEN_HEADER, token(presented.max(written)))
            }
            ("POST", path) if path.ends_with(":wait") => {
                let written: usize = path
                    .rsplit("op-")
                    .next()
                    .and_then(|rest| rest.strip_suffix(":wait"))
                    .and_then(|n| n.parse().ok())
                    .expect("an operation of this front door");
                let mut response = volume_json(written);
                response["@type"] = json!("type.googleapis.com/origincloud.pvs.v1beta1.Volume");
                Response::json(
                    json!({ "name": operation_name(written), "done": true, "response": response })
                        .to_string(),
                )
                .with_header(CONSISTENCY_TOKEN_HEADER, token(presented.max(written)))
            }
            ("GET", path) if path.starts_with(&format!("{volumes}/vol-")) => {
                let wanted: usize = path.rsplit("vol-").next().unwrap().parse().unwrap();
                let deadline = Instant::now() + self.bound;
                while self.read_model() < presented {
                    if Instant::now() > deadline {
                        return Response::json(
                            r#"{"error":{"code":503,"status":"UNAVAILABLE","message":"the read model hasn't reached the token yet","details":[
                                {"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"SERVICE_UNAVAILABLE","domain":"origincloudapis.eu"},
                                {"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"0.020s"}]}}"#,
                        )
                        .with_status(503);
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                if self.read_model() < wanted {
                    return Response::json(
                        r#"{"error":{"code":404,"status":"NOT_FOUND","message":"volume not found","details":[
                            {"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"NOT_FOUND","domain":"origincloudapis.eu"}]}}"#,
                    )
                    .with_status(404);
                }
                Response::json(volume_json(wanted).to_string())
            }
            _ => panic!("unexpected {} {}", r.method, r.path),
        }
    }
}

fn token(position: usize) -> String {
    format!("v1.{position}")
}

fn position(token: &str) -> usize {
    token
        .strip_prefix("v1.")
        .and_then(|n| n.parse().ok())
        .unwrap_or_default()
}

fn operation_name(position: usize) -> String {
    format!("{ZONE}/operations/op-{position}")
}

fn volume_json(position: usize) -> Value {
    json!({ "name": format!("{ZONE}/volumes/vol-{position}"), "state": "AVAILABLE", "sizeGib": 1 })
}

fn new_client(server: &FakeServer) -> VolumeClient {
    let client = Client::builder()
        .endpoint(server.url())
        .credentials(AccessToken::new("t"))
        .build()
        .unwrap();
    VolumeClient::new(&client)
}

/// A get right after a create's operation is done finds the volume, every
/// time, although the read model is behind the create.
#[tokio::test(flavor = "multi_thread")]
async fn a_get_after_a_create_is_done_finds_the_volume() {
    let front_door = Arc::new(LaggingFrontDoor {
        lag: Duration::from_millis(150),
        bound: Duration::from_millis(50),
        visible: Mutex::default(),
    });
    let handler = Arc::clone(&front_door);
    let server = FakeServer::start(move |r| {
        let front_door = Arc::clone(&handler);
        async move { front_door.answer(r).await }
    })
    .await;
    let volumes = new_client(&server);

    for i in 0..20 {
        let created = volumes
            .create_volume(CreateVolumeRequest {
                parent: ZONE.to_owned(),
                volume: Some(Volume {
                    size_gib: 1,
                    ..Default::default()
                }),
                ..Default::default()
            })
            .await
            .unwrap_or_else(|e| panic!("create {i}: {e}"))
            .wait()
            .await
            .unwrap_or_else(|e| panic!("wait {i}: {e}"));
        assert_eq!(created.state(), volume::State::Available);

        // The control: a client that didn't write doesn't see it yet, so
        // the read model really is behind.
        let err = new_client(&server)
            .get_volume(GetVolumeRequest {
                name: created.name.clone(),
            })
            .await
            .expect_err("the read model is behind the create");
        assert_eq!(
            err.reason(),
            Some("NOT_FOUND"),
            "get {i} without the token: {err}"
        );

        let got = volumes
            .get_volume(GetVolumeRequest {
                name: created.name.clone(),
            })
            .await
            .unwrap_or_else(|e| panic!("get {i} after its create: {e}"));
        assert_eq!(got.name, created.name, "get {i}");
    }
}
