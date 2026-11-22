//! The IAM clients call the global host, send each method as its HTTP
//! binding says, and refuse a key's create repeated after its credentials
//! file was returned.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fake_server::{FakeServer, Received, Response};
use origincloud_core::{AccessToken, Client};
use origincloud_iam::v1beta1::{
    AuditLogClient, CreateRoleBindingRequest, CreateServiceAccountKeyRequest,
    CreateServiceAccountRequest, GetServiceAccountRequest, PermissionClient, RoleBinding,
    RoleBindingClient, RoleClient, ServiceAccount, ServiceAccountClient, ServiceAccountKey,
    ServiceAccountKeyClient, UserClient, service_account,
};
use serde_json::{Value, json};

/// A client whose calls are recorded and answered with `answer`.
async fn recording(answer: &'static str) -> (FakeServer, Client, Arc<Mutex<Vec<Received>>>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&calls);
    let server = FakeServer::start(move |r| {
        log.lock().unwrap().push(r);
        async move { Response::json(answer) }
    })
    .await;
    let client = Client::builder()
        .endpoint(server.url())
        .credentials(AccessToken::new("t"))
        .build()
        .unwrap();
    (server, client, calls)
}

fn is_uuid(s: &str) -> bool {
    s.len() == 36 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

fn json_of(r: &Received) -> Value {
    serde_json::from_slice(&r.body).unwrap()
}

#[test]
fn a_regional_client_still_calls_the_regionless_host() {
    let client = Client::builder()
        .region("eu-central1")
        .credentials(AccessToken::new("t"))
        .build()
        .unwrap();
    for base in [
        ServiceAccountClient::new(&client).service().base_url(),
        RoleBindingClient::new(&client).service().base_url(),
        RoleClient::new(&client).service().base_url(),
        PermissionClient::new(&client).service().base_url(),
        UserClient::new(&client).service().base_url(),
        AuditLogClient::new(&client).service().base_url(),
    ] {
        assert_eq!(
            base.map(|url| url.as_str()),
            Some("https://iam.origincloudapis.eu/")
        );
    }
}

#[tokio::test]
async fn get_service_account_reads_it() {
    let (_server, client, calls) = recording(
        r#"{"name":"organizations/o/serviceAccounts/s","displayName":"CI","member":"serviceAccount:s","state":"ACTIVE"}"#,
    )
    .await;

    let account = ServiceAccountClient::new(&client)
        .get_service_account(GetServiceAccountRequest {
            name: "organizations/o/serviceAccounts/s".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(account.member, "serviceAccount:s");
    assert_eq!(account.state(), service_account::State::Active);

    let calls = calls.lock().unwrap();
    let [get] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    assert_eq!(get.method, "GET");
    assert_eq!(get.path, "/v1beta1/organizations/o/serviceAccounts/s");
    assert!(get.query.is_empty() && get.body.is_empty(), "{get:?}");
}

/// A service account is created with no credential: its keys are created
/// apart from it.
#[tokio::test]
async fn create_service_account_sends_it_as_the_body() {
    let (_server, client, calls) = recording(
        r#"{"name":"organizations/o/serviceAccounts/s","displayName":"CI","member":"serviceAccount:s","state":"ACTIVE"}"#,
    )
    .await;

    let account = ServiceAccountClient::new(&client)
        .create_service_account(CreateServiceAccountRequest {
            parent: "organizations/o".to_owned(),
            service_account: Some(ServiceAccount {
                display_name: "CI".to_owned(),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(account.name, "organizations/o/serviceAccounts/s");

    let calls = calls.lock().unwrap();
    let [created] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    // body: "service_account": the service account is the body, the request
    // id goes in the query, filled in as it was left empty.
    assert_eq!(created.method, "POST");
    assert_eq!(created.path, "/v1beta1/organizations/o/serviceAccounts");
    assert_eq!(json_of(created), json!({"displayName": "CI"}));
    let [request_id] = created.query_values("requestId").try_into().unwrap();
    assert!(is_uuid(&request_id), "{created:?}");
    assert_eq!(created.query_pairs().len(), 1, "{created:?}");
}

/// A key's create whose answer was lost is sent again with the same
/// request id. The key exists by then, but its credentials file went to the
/// lost answer, so the API refuses the repeat and names the key; the
/// refusal is not sent again.
#[tokio::test]
async fn a_key_s_create_repeated_after_its_answer_was_lost_names_the_key() {
    let calls = Arc::new(Mutex::new(Vec::<Received>::new()));
    let log = Arc::clone(&calls);
    let attempts = Arc::new(AtomicU32::new(0));
    let counted = Arc::clone(&attempts);
    let server = FakeServer::start(move |r| {
        log.lock().unwrap().push(r);
        let n = counted.fetch_add(1, Ordering::SeqCst) + 1;
        async move {
            if n == 1 {
                // Created, but the answer doesn't arrive in time.
                tokio::time::sleep(Duration::from_secs(5)).await;
                return Response::json(
                    r#"{"name":"organizations/o/serviceAccounts/s/keys/k","credentialsFile":"lost"}"#,
                );
            }
            Response::json(
                r#"{"error":{"code":409,"status":"FAILED_PRECONDITION","message":"the service account key organizations/o/serviceAccounts/s/keys/k was created by an earlier request with this request_id; its credentials file was returned only to that request. Delete that key and create a new one","details":[
                    {"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"CONFLICT","domain":"origincloudapis.eu","metadata":{"serviceAccountKey":"organizations/o/serviceAccounts/s/keys/k"}},
                    {"@type":"type.googleapis.com/google.rpc.RequestInfo","requestId":"4bf92f3577b34da6"}]}}"#,
            )
            .with_status(409)
        }
    })
    .await;
    let client = Client::builder()
        .endpoint(server.url())
        .credentials(AccessToken::new("t"))
        .timeout(Duration::from_millis(300))
        .build()
        .unwrap();

    let err = ServiceAccountKeyClient::new(&client)
        .create_service_account_key(CreateServiceAccountKeyRequest {
            parent: "organizations/o/serviceAccounts/s".to_owned(),
            service_account_key: Some(ServiceAccountKey::default()),
            ..Default::default()
        })
        .await
        .unwrap_err();

    assert_eq!(err.reason(), Some("CONFLICT"));
    assert!(!err.is_retryable());
    let api = err.api().expect("an API error");
    assert_eq!(api.http_status, Some(409));
    assert_eq!(
        api.metadata.get("serviceAccountKey").map(String::as_str),
        Some("organizations/o/serviceAccounts/s/keys/k")
    );
    assert!(
        err.to_string()
            .contains("organizations/o/serviceAccounts/s/keys/k was created by an earlier request"),
        "{err}"
    );

    let calls = calls.lock().unwrap();
    let [first, repeat] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    let [first_id] = first.query_values("requestId").try_into().unwrap();
    assert!(is_uuid(&first_id), "{first:?}");
    assert_eq!(repeat.query_values("requestId"), [first_id]);
    assert_eq!(
        repeat.path,
        "/v1beta1/organizations/o/serviceAccounts/s/keys"
    );
    assert_eq!(json_of(repeat), json!({}));
}

#[tokio::test]
async fn create_role_binding_keeps_a_given_request_id() {
    let (_server, client, calls) = recording("{}").await;

    RoleBindingClient::new(&client)
        .create_role_binding(CreateRoleBindingRequest {
            parent: "organizations/o".to_owned(),
            role_binding: Some(RoleBinding {
                member: "user:ada@example.com".to_owned(),
                role: "roles/viewer".to_owned(),
                project: "projects/p".to_owned(),
                ..Default::default()
            }),
            request_id: "given".to_owned(),
        })
        .await
        .unwrap();

    let calls = calls.lock().unwrap();
    let [created] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    assert_eq!(created.method, "POST");
    assert_eq!(created.path, "/v1beta1/organizations/o/roleBindings");
    assert_eq!(
        json_of(created),
        json!({"member": "user:ada@example.com", "role": "roles/viewer", "project": "projects/p"})
    );
    assert_eq!(
        created.query_pairs(),
        [("requestId".to_owned(), "given".to_owned())]
    );
}
