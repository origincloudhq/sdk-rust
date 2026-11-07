//! The dns clients call the global host, send each method as its HTTP
//! binding says, and read a zone's state and name servers.

use std::sync::{Arc, Mutex};

use fake_server::{FakeServer, Received, Response};
use origincloud_core::protobuf::Duration;
use origincloud_core::{AccessToken, Client};
use origincloud_dns::v1beta1::{
    BasicRecordData, BatchChangeRecordSetsRequest, CreateHostedZoneRequest, GetHostedZoneRequest,
    HostedZone, HostedZoneClient, RecordSet, RecordSetClient, VerifyHostedZoneRequest,
    VpcAttachmentClient, hosted_zone, record_set,
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
        HostedZoneClient::new(&client).service().base_url(),
        RecordSetClient::new(&client).service().base_url(),
        VpcAttachmentClient::new(&client).service().base_url(),
    ] {
        assert_eq!(
            base.map(|url| url.as_str()),
            Some("https://dns.origincloudapis.eu/")
        );
    }
}

/// A zone created without a request id gets one, so a retry after a lost
/// answer isn't a second zone.
#[tokio::test]
async fn create_hosted_zone_is_sent_with_a_request_id() {
    let (_server, client, calls) = recording("{}").await;

    HostedZoneClient::new(&client)
        .create_hosted_zone(CreateHostedZoneRequest {
            parent: "projects/p".to_owned(),
            hosted_zone: Some(HostedZone {
                dns_name: "example.com".to_owned(),
                visibility: hosted_zone::Visibility::Public.into(),
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
    // body: "hosted_zone": the zone is the body, the request id goes in the
    // query.
    assert_eq!(created.method, "POST");
    assert_eq!(created.path, "/v1beta1/projects/p/hostedZones");
    assert_eq!(
        json_of(created),
        json!({"dnsName": "example.com", "visibility": "PUBLIC"})
    );
    let [request_id] = created.query_values("requestId").try_into().unwrap();
    assert!(is_uuid(&request_id), "{created:?}");
    assert_eq!(created.query_pairs().len(), 1, "{created:?}");
}

#[tokio::test]
async fn batch_change_record_sets_sends_the_whole_request_as_its_body() {
    let (_server, client, calls) = recording("{}").await;

    RecordSetClient::new(&client)
        .batch_change_record_sets(BatchChangeRecordSetsRequest {
            parent: "projects/p/hostedZones/z".to_owned(),
            deletions: vec!["projects/p/hostedZones/z/recordSets/old".to_owned()],
            creations: vec![RecordSet {
                dns_name: "www.example.com".to_owned(),
                r#type: record_set::Type::A.into(),
                ttl: Some(Duration {
                    seconds: 60,
                    nanos: 0,
                }),
                data: Some(record_set::Data::Basic(BasicRecordData {
                    values: vec!["192.0.2.1".to_owned()],
                })),
                ..Default::default()
            }],
            request_id: "given".to_owned(),
            ..Default::default()
        })
        .await
        .unwrap();

    let calls = calls.lock().unwrap();
    let [batch] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    // body: "*": the whole request is the body, its request id included.
    assert_eq!(batch.method, "POST");
    assert_eq!(
        batch.path,
        "/v1beta1/projects/p/hostedZones/z/recordSets:batchChange"
    );
    assert_eq!(
        json_of(batch),
        json!({
            "parent": "projects/p/hostedZones/z",
            "deletions": ["projects/p/hostedZones/z/recordSets/old"],
            "creations": [{
                "dnsName": "www.example.com",
                "type": "A",
                "ttl": "60s",
                "basic": {"values": ["192.0.2.1"]}
            }],
            "requestId": "given"
        })
    );
    assert!(batch.query.is_empty(), "{batch:?}");
}

#[tokio::test]
async fn verify_hosted_zone_sends_the_whole_request_as_its_body() {
    let (_server, client, calls) = recording("{}").await;

    HostedZoneClient::new(&client)
        .verify_hosted_zone(VerifyHostedZoneRequest {
            name: "projects/p/hostedZones/z".to_owned(),
        })
        .await
        .unwrap();

    let calls = calls.lock().unwrap();
    let [verified] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    assert_eq!(verified.method, "POST");
    assert_eq!(verified.path, "/v1beta1/projects/p/hostedZones/z:verify");
    assert_eq!(
        json_of(verified),
        json!({"name": "projects/p/hostedZones/z"})
    );
    assert!(verified.query.is_empty(), "{verified:?}");
}

#[tokio::test]
async fn a_zone_decodes_its_state_and_name_servers() {
    let (_server, client, _calls) = recording(
        r#"{"name":"projects/p/hostedZones/z","dnsName":"example.com","visibility":"PUBLIC","state":"ACTIVE","nameServers":["ns1.origincloud.eu","ns2.origincloud.eu","ns3.origincloud.eu"],"tags":{"team":"web"}}"#,
    )
    .await;

    let got = HostedZoneClient::new(&client)
        .get_hosted_zone(GetHostedZoneRequest {
            name: "projects/p/hostedZones/z".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(got.state(), hosted_zone::State::Active);
    assert_eq!(got.visibility(), hosted_zone::Visibility::Public);
    assert_eq!(got.name_servers.len(), 3);
    assert_eq!(got.tags.get("team").map(String::as_str), Some("web"));
}
