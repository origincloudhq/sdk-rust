//! The compute clients call the region's host, send each method as its HTTP
//! binding says, and read what the service answers.

use std::sync::{Arc, Mutex};

use fake_server::{FakeServer, Received, Response};
use origincloud_compute::v1beta1::{
    CreateKeyPairRequest, GetInstanceRequest, InstanceClient, InstanceTypeClient, KeyPair,
    KeyPairClient, ListInstanceTypesRequest, ListKeyPairsRequest,
};
use origincloud_core::{AccessToken, Client};
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
        (None, "https://compute.origincloudapis.eu/"),
        (
            Some("eu-central1"),
            "https://compute.eu-central1.origincloudapis.eu/",
        ),
    ] {
        let mut builder = Client::builder().credentials(AccessToken::new("t"));
        if let Some(region) = region {
            builder = builder.region(region);
        }
        let client = builder.build().unwrap();
        for base in [
            InstanceClient::new(&client).service().base_url(),
            InstanceTypeClient::new(&client).service().base_url(),
            KeyPairClient::new(&client).service().base_url(),
        ] {
            assert_eq!(base.map(|url| url.as_str()), Some(host), "{region:?}");
        }
    }
}

#[tokio::test]
async fn get_instance_reads_it_at_its_name() {
    let (_server, client, calls) = recording(
        r#"{"name":"projects/p/zones/z/instances/i","instanceType":"instanceTypes/gp1.core.2"}"#,
    )
    .await;

    let instance = InstanceClient::new(&client)
        .get_instance(GetInstanceRequest {
            name: "projects/p/zones/z/instances/i".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(instance.name, "projects/p/zones/z/instances/i");
    assert_eq!(instance.instance_type, "instanceTypes/gp1.core.2");

    let calls = calls.lock().unwrap();
    let [get] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    assert_eq!(get.method, "GET");
    assert_eq!(get.path, "/v1beta1/projects/p/zones/z/instances/i");
    assert!(get.query.is_empty() && get.body.is_empty(), "{get:?}");
}

/// Key pairs are the project's, in every zone. A key pair the platform
/// generates is answered with its private key, once.
#[tokio::test]
async fn create_key_pair_sends_the_key_pair_with_a_request_id() {
    let (_server, client, calls) = recording(
        r#"{"name":"projects/p/keyPairs/k","publicKey":"ssh-ed25519 AAAA","privateKey":"shown-once"}"#,
    )
    .await;

    let key_pair = KeyPairClient::new(&client)
        .create_key_pair(CreateKeyPairRequest {
            parent: "projects/p".to_owned(),
            key_pair: Some(KeyPair {
                tags: [("origin:name".to_owned(), "deploy".to_owned())].into(),
                ..Default::default()
            }),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(key_pair.private_key, "shown-once");

    let calls = calls.lock().unwrap();
    let [created] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    // body: "key_pair": the key pair is the body, the request id goes in the
    // query, filled in as it was left empty.
    assert_eq!(created.method, "POST");
    assert_eq!(created.path, "/v1beta1/projects/p/keyPairs");
    assert_eq!(json_of(created), json!({"tags": {"origin:name": "deploy"}}));
    let [request_id] = created.query_values("requestId").try_into().unwrap();
    assert!(is_uuid(&request_id), "{created:?}");
    assert_eq!(created.query_pairs().len(), 1, "{created:?}");
}

#[tokio::test]
async fn list_key_pairs_sends_its_parent_in_the_path_and_the_paging_in_the_query() {
    let (_server, client, calls) =
        recording(r#"{"keyPairs":[{"name":"projects/p/keyPairs/k"}],"nextPageToken":"next"}"#)
            .await;

    let page = KeyPairClient::new(&client)
        .list_key_pairs(ListKeyPairsRequest {
            parent: "projects/p".to_owned(),
            page_size: 10,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(page.key_pairs.len(), 1);
    assert_eq!(page.next_page_token, "next");

    let calls = calls.lock().unwrap();
    let [listed] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    assert_eq!(listed.method, "GET");
    assert_eq!(listed.path, "/v1beta1/projects/p/keyPairs");
    assert_eq!(
        listed.query_pairs(),
        [("pageSize".to_owned(), "10".to_owned())]
    );
}

/// Instance types belong to no project: the list has no parent.
#[tokio::test]
async fn list_instance_types_reads_the_catalog() {
    let (_server, client, calls) = recording(
        r#"{"instanceTypes":[{"name":"instanceTypes/gp1.core.2","vcpu":2,"memoryMib":4096,"available":true}]}"#,
    )
    .await;

    let types = InstanceTypeClient::new(&client)
        .list_instance_types(ListInstanceTypesRequest {})
        .await
        .unwrap();
    let [gp1] = types.instance_types.as_slice() else {
        panic!("{types:?}");
    };
    assert_eq!((gp1.vcpu, gp1.memory_mib, gp1.available), (2, 4096, true));

    let calls = calls.lock().unwrap();
    let [listed] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    assert_eq!(listed.method, "GET");
    assert_eq!(listed.path, "/v1beta1/instanceTypes");
}
