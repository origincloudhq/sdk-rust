//! A secret is created, given a version and read back as `latest` through the
//! generated client, against a fake transport that keeps what it is sent.

use std::sync::{Arc, Mutex};

use fake_server::{FakeServer, Received, Response};
use origincloud_core::{AccessToken, Client};
use origincloud_secretmanager::v1beta1::{
    AccessSecretVersionRequest, AddSecretVersionRequest, CreateSecretRequest, SecretManagerClient,
    SecretPayload, secret_version,
};
use serde_json::{Value, json};

const REGION: &str = "projects/p/regions/eu-central1";
const SECRET: &str = "projects/p/regions/eu-central1/secrets/billing-db-password";
/// `hunter2`, as the JSON of a bytes field carries it.
const HUNTER2_BASE64: &str = "aHVudGVyMg==";

/// What the fake service keeps: the payloads of the secret's versions.
#[derive(Default)]
struct Store {
    versions: Vec<String>,
    calls: Vec<Received>,
}

fn answer(store: &Mutex<Store>, r: Received) -> Response {
    let mut store = store.lock().unwrap();
    let body: Value = if r.body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&r.body).unwrap()
    };
    let response = match (r.method.as_str(), r.path.as_str()) {
        ("POST", p) if p == format!("/v1beta1/{REGION}/secrets") => {
            assert_eq!(r.query_values("secretId"), ["billing-db-password"]);
            json!({"name": SECRET, "state": "ACTIVE"})
        }
        ("POST", p) if p == format!("/v1beta1/{SECRET}:addVersion") => {
            store
                .versions
                .push(body["payload"]["data"].as_str().unwrap().to_owned());
            json!({
                "name": format!("{SECRET}/versions/{}", store.versions.len()),
                "state": "ENABLED"
            })
        }
        ("GET", p) if p == format!("/v1beta1/{SECRET}/versions/latest:access") => {
            let number = store.versions.len();
            json!({
                "secretVersion":
                    format!("//secretmanager.origincloudapis.eu/{SECRET}/versions/{number}"),
                "payload": {"data": store.versions.last().unwrap()}
            })
        }
        _ => {
            return Response::json(r#"{"code":"NOT_FOUND","message":"no such route"}"#)
                .with_status(404);
        }
    };
    store.calls.push(r);
    Response::json(response.to_string())
}

#[tokio::test]
async fn a_secret_is_created_given_a_version_and_read_as_latest() {
    let store = Arc::new(Mutex::new(Store::default()));
    let kept = Arc::clone(&store);
    let server = FakeServer::start(move |r| {
        let response = answer(&kept, r);
        async move { response }
    })
    .await;
    let client = Client::builder()
        .endpoint(server.url())
        .credentials(AccessToken::new("t"))
        .build()
        .unwrap();
    let secrets = SecretManagerClient::new(&client);

    let created = secrets
        .create_secret(CreateSecretRequest {
            parent: REGION.to_owned(),
            secret_id: "billing-db-password".to_owned(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(created.name, SECRET);

    let version = secrets
        .add_secret_version(AddSecretVersionRequest {
            parent: SECRET.to_owned(),
            payload: Some(SecretPayload {
                data: b"hunter2".to_vec(),
            }),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(version.name, format!("{SECRET}/versions/1"));
    assert_eq!(version.state(), secret_version::State::Enabled);

    let accessed = secrets
        .access_secret_version(AccessSecretVersionRequest {
            name: format!("{SECRET}/versions/latest"),
        })
        .await
        .unwrap();
    assert_eq!(
        accessed.secret_version,
        format!("//secretmanager.origincloudapis.eu/{SECRET}/versions/1")
    );
    assert_eq!(accessed.payload.unwrap().data, b"hunter2");

    let store = store.lock().unwrap();
    assert_eq!(store.versions, [HUNTER2_BASE64]);
    assert_eq!(store.calls.len(), 3);
}
