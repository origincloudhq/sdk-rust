//! The support clients call the global host, send each method as its HTTP
//! binding says, and read a case's state.

use std::sync::{Arc, Mutex};

use fake_server::{FakeServer, Received, Response};
use origincloud_core::{AccessToken, Client};
use origincloud_support::v1beta1::{
    Case, CaseClient, CaseMessage, CaseMessageClient, CloseCaseRequest, CreateCaseMessageRequest,
    CreateCaseRequest, GetCaseRequest, case,
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
        CaseClient::new(&client).service().base_url(),
        CaseMessageClient::new(&client).service().base_url(),
    ] {
        assert_eq!(
            base.map(|url| url.as_str()),
            Some("https://support.origincloudapis.eu/")
        );
    }
}

/// A case opened without a request id gets one, so a retry after a lost
/// answer isn't a second case.
#[tokio::test]
async fn create_case_is_sent_with_a_request_id() {
    let (_server, client, calls) = recording("{}").await;

    CaseClient::new(&client)
        .create_case(CreateCaseRequest {
            parent: "organizations/o".to_owned(),
            case: Some(Case {
                subject: "Volume stuck".to_owned(),
                description: "It stays ATTACHING.".to_owned(),
                contact_email: "ada@example.com".to_owned(),
                project: "projects/p".to_owned(),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .unwrap();

    let calls = calls.lock().unwrap();
    let [created] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    // body: "case": the case is the body, the request id goes in the query.
    assert_eq!(created.method, "POST");
    assert_eq!(created.path, "/v1beta1/organizations/o/cases");
    assert_eq!(
        json_of(created),
        json!({
            "subject": "Volume stuck",
            "description": "It stays ATTACHING.",
            "contactEmail": "ada@example.com",
            "project": "projects/p"
        })
    );
    let [request_id] = created.query_values("requestId").try_into().unwrap();
    assert!(is_uuid(&request_id), "{created:?}");
    assert_eq!(created.query_pairs().len(), 1, "{created:?}");
}

#[tokio::test]
async fn create_case_message_keeps_a_given_request_id() {
    let (_server, client, calls) = recording("{}").await;

    CaseMessageClient::new(&client)
        .create_case_message(CreateCaseMessageRequest {
            parent: "organizations/o/cases/c".to_owned(),
            case_message: Some(CaseMessage {
                body: "Still stuck.".to_owned(),
                ..Default::default()
            }),
            request_id: "given".to_owned(),
        })
        .await
        .unwrap();

    let calls = calls.lock().unwrap();
    let [wrote] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    assert_eq!(wrote.method, "POST");
    assert_eq!(wrote.path, "/v1beta1/organizations/o/cases/c/messages");
    assert_eq!(json_of(wrote), json!({"body": "Still stuck."}));
    assert_eq!(
        wrote.query_pairs(),
        [("requestId".to_owned(), "given".to_owned())]
    );
}

#[tokio::test]
async fn close_case_sends_the_whole_request_as_its_body() {
    let (_server, client, calls) = recording("{}").await;

    CaseClient::new(&client)
        .close_case(CloseCaseRequest {
            name: "organizations/o/cases/c".to_owned(),
        })
        .await
        .unwrap();

    let calls = calls.lock().unwrap();
    let [closed] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    // body: "*": the whole request is the body, nothing goes in the query.
    assert_eq!(closed.method, "POST");
    assert_eq!(closed.path, "/v1beta1/organizations/o/cases/c:close");
    assert_eq!(json_of(closed), json!({"name": "organizations/o/cases/c"}));
    assert!(closed.query.is_empty(), "{closed:?}");
}

#[tokio::test]
async fn a_case_decodes_its_state() {
    let (_server, client, _calls) = recording(
        r#"{"name":"organizations/o/cases/c","subject":"Volume stuck","state":"IN_PROGRESS","createTime":"2026-11-03T09:00:00Z","updateTime":"2026-11-03T10:30:00Z"}"#,
    )
    .await;

    let got = CaseClient::new(&client)
        .get_case(GetCaseRequest {
            name: "organizations/o/cases/c".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(got.state(), case::State::InProgress);
    assert_eq!(got.subject, "Volume stuck");
    assert_eq!(got.project, "");
    let (created, updated) = (got.create_time.unwrap(), got.update_time.unwrap());
    assert_eq!(created.seconds, 1_793_696_400);
    assert_eq!(updated.seconds - created.seconds, 90 * 60);
}
