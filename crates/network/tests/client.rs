//! The network clients call the region's host, send each method as its HTTP
//! binding says, and follow a create that answers with an operation.

use std::sync::{Arc, Mutex};

use fake_server::{FakeServer, Received, Response};
use origincloud_core::{AccessToken, Client};
use origincloud_network::v1beta1::{
    CreateVpcRequest, GetVpcRequest, ListSubnetsRequest, SubnetClient, Vpc, VpcClient,
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
fn a_regional_client_calls_the_regions_host() {
    for (region, host) in [
        (None, "https://network.origincloudapis.eu/"),
        (
            Some("eu-central1"),
            "https://network.eu-central1.origincloudapis.eu/",
        ),
    ] {
        let mut builder = Client::builder().credentials(AccessToken::new("t"));
        if let Some(region) = region {
            builder = builder.region(region);
        }
        let client = builder.build().unwrap();
        for base in [
            VpcClient::new(&client).service().base_url(),
            SubnetClient::new(&client).service().base_url(),
        ] {
            assert_eq!(base.map(|url| url.as_str()), Some(host), "{region:?}");
        }
    }
}

#[tokio::test]
async fn get_vpc_reads_it_at_its_regional_name() {
    let (_server, client, calls) = recording(
        r#"{"name":"projects/p/regions/r/vpcs/v","projectDefault":true,"availableSubnetCount":250}"#,
    )
    .await;

    let vpc = VpcClient::new(&client)
        .get_vpc(GetVpcRequest {
            name: "projects/p/regions/r/vpcs/v".to_owned(),
        })
        .await
        .unwrap();
    assert!(vpc.project_default);
    assert_eq!(vpc.available_subnet_count, 250);

    let calls = calls.lock().unwrap();
    let [get] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    assert_eq!(get.method, "GET");
    assert_eq!(get.path, "/v1beta1/projects/p/regions/r/vpcs/v");
    assert!(get.query.is_empty() && get.body.is_empty(), "{get:?}");
}

/// A VPC is created asynchronously: the answer is the operation, named in
/// the request's region, not the VPC.
#[tokio::test]
async fn create_vpc_sends_the_vpc_and_answers_with_its_operation() {
    let (_server, client, calls) =
        recording(r#"{"name":"projects/p/regions/r/operations/op-1"}"#).await;

    let operation = VpcClient::new(&client)
        .create_vpc(CreateVpcRequest {
            parent: "projects/p/regions/r".to_owned(),
            vpc: Some(Vpc {
                tags: [("origin:name".to_owned(), "app".to_owned())].into(),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(operation.name(), "projects/p/regions/r/operations/op-1");
    assert!(!operation.done());

    let calls = calls.lock().unwrap();
    let [created] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    // body: "vpc": the VPC is the body, the request id goes in the query,
    // filled in as it was left empty.
    assert_eq!(created.method, "POST");
    assert_eq!(created.path, "/v1beta1/projects/p/regions/r/vpcs");
    assert_eq!(json_of(created), json!({"tags": {"origin:name": "app"}}));
    let [request_id] = created.query_values("requestId").try_into().unwrap();
    assert!(is_uuid(&request_id), "{created:?}");
    assert_eq!(created.query_pairs().len(), 1, "{created:?}");
}

/// Subnets are zonal; the VPC to list them for is a filter in the query.
#[tokio::test]
async fn list_subnets_sends_the_zone_in_the_path_and_the_vpc_in_the_query() {
    let (_server, client, calls) = recording(
        r#"{"subnets":[{"name":"projects/p/zones/z/subnets/s"}],"nextPageToken":"next"}"#,
    )
    .await;

    let page = SubnetClient::new(&client)
        .list_subnets(ListSubnetsRequest {
            parent: "projects/p/zones/z".to_owned(),
            vpc: "projects/p/regions/r/vpcs/v".to_owned(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(page.subnets.len(), 1);
    assert_eq!(page.next_page_token, "next");

    let calls = calls.lock().unwrap();
    let [listed] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    assert_eq!(listed.method, "GET");
    assert_eq!(listed.path, "/v1beta1/projects/p/zones/z/subnets");
    assert_eq!(
        listed.query_pairs(),
        [("vpc".to_owned(), "projects/p/regions/r/vpcs/v".to_owned())]
    );
}
