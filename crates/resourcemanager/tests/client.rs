//! The resource manager clients call the global host, even from a regional
//! client, send each method as its HTTP binding says, and read what the
//! service answers.

use std::sync::{Arc, Mutex};

use fake_server::{FakeServer, Received, Response};
use origincloud_core::{AccessToken, Client};
use origincloud_resourcemanager::v1beta1::{
    CreateProjectRequest, GetProjectRequest, ListProjectsRequest, OrganizationClient, Project,
    ProjectClient,
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
        OrganizationClient::new(&client).service().base_url(),
        ProjectClient::new(&client).service().base_url(),
    ] {
        assert_eq!(
            base.map(|url| url.as_str()),
            Some("https://resourcemanager.origincloudapis.eu/")
        );
    }
}

#[tokio::test]
async fn get_project_reads_it_at_its_name() {
    let (_server, client, calls) =
        recording(r#"{"name":"projects/p","organization":"organizations/o","displayName":"Web"}"#)
            .await;

    let project = ProjectClient::new(&client)
        .get_project(GetProjectRequest {
            name: "projects/p".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(project.organization, "organizations/o");
    assert_eq!(project.display_name, "Web");

    let calls = calls.lock().unwrap();
    let [get] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    assert_eq!(get.method, "GET");
    assert_eq!(get.path, "/v1beta1/projects/p");
    assert!(get.query.is_empty() && get.body.is_empty(), "{get:?}");
}

/// A project is created at the top level, its organization in the body and
/// its id chosen by the client, in the query with the request id.
#[tokio::test]
async fn create_project_sends_the_project_and_its_id() {
    let (_server, client, calls) = recording(
        r#"{"name":"projects/web","organization":"organizations/o","displayName":"Web"}"#,
    )
    .await;

    let project = ProjectClient::new(&client)
        .create_project(CreateProjectRequest {
            project: Some(Project {
                organization: "organizations/o".to_owned(),
                display_name: "Web".to_owned(),
                ..Default::default()
            }),
            project_id: "web".to_owned(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(project.name, "projects/web");

    let calls = calls.lock().unwrap();
    let [created] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    assert_eq!(created.method, "POST");
    assert_eq!(created.path, "/v1beta1/projects");
    assert_eq!(
        json_of(created),
        json!({"organization": "organizations/o", "displayName": "Web"})
    );
    assert_eq!(created.query_values("projectId"), ["web"]);
    let [request_id] = created.query_values("requestId").try_into().unwrap();
    assert!(is_uuid(&request_id), "{created:?}");
}

#[tokio::test]
async fn list_projects_sends_the_organization_in_the_query() {
    let (_server, client, calls) =
        recording(r#"{"projects":[{"name":"projects/p"}],"nextPageToken":"next"}"#).await;

    let page = ProjectClient::new(&client)
        .list_projects(ListProjectsRequest {
            organization: "organizations/o".to_owned(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(page.projects.len(), 1);
    assert_eq!(page.next_page_token, "next");

    let calls = calls.lock().unwrap();
    let [listed] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    assert_eq!(listed.method, "GET");
    assert_eq!(listed.path, "/v1beta1/projects");
    assert_eq!(
        listed.query_pairs(),
        [("organization".to_owned(), "organizations/o".to_owned())]
    );
}
