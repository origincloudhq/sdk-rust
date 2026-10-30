//! The generated clients send each method as its HTTP binding says.

use std::sync::{Arc, Mutex};

use fake_server::{FakeServer, Received, Response};
use origincloud_core::protobuf::FieldMask;
use origincloud_core::{AccessToken, Client};
use origincloud_pvs::v1beta1::{
    CreateVolumeRequest, DeleteVolumeRequest, GetVolumeRequest, ImportVolumeRequest,
    ListStorageClassesRequest, ListVolumesRequest, StorageClassClient, UpdateVolumeRequest, Volume,
    VolumeClient,
};
use serde_json::{Value, json};

const ZONE: &str = "projects/p/zones/z";

fn client(server: &FakeServer) -> Client {
    Client::builder()
        .endpoint(server.url())
        .credentials(AccessToken::new("t"))
        .build()
        .unwrap()
}

/// A client whose calls are recorded and answered with `answer`.
async fn recording(answer: &'static str) -> (FakeServer, Client, Arc<Mutex<Vec<Received>>>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let log = Arc::clone(&calls);
    let server = FakeServer::start(move |r| {
        log.lock().unwrap().push(r);
        async move { Response::json(answer) }
    })
    .await;
    let client = client(&server);
    (server, client, calls)
}

fn is_uuid(s: &str) -> bool {
    s.len() == 36 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

fn json_of(r: &Received) -> Value {
    serde_json::from_slice(&r.body).unwrap()
}

#[tokio::test]
async fn each_binding_is_sent_as_its_http_rule_says() {
    let (_server, client, calls) =
        recording(r#"{"name":"projects/p/zones/z/operations/op-1"}"#).await;
    let volumes = VolumeClient::new(&client);
    let name = format!("{ZONE}/volumes/vol-1");

    volumes
        .list_volumes(ListVolumesRequest {
            parent: ZONE.to_owned(),
            page_size: 5,
            page_token: "next".to_owned(),
        })
        .await
        .unwrap();
    volumes
        .update_volume(UpdateVolumeRequest {
            volume: Some(Volume {
                name: name.clone(),
                size_gib: 2,
                ..Default::default()
            }),
            update_mask: Some(FieldMask {
                paths: vec!["size_gib".to_owned(), "tags".to_owned()],
            }),
            ..Default::default()
        })
        .await
        .unwrap();
    volumes
        .import_volume(ImportVolumeRequest {
            parent: ZONE.to_owned(),
            volume: Some(Volume {
                size_gib: 3,
                ..Default::default()
            }),
            source_url: "https://example.com/disk.img".to_owned(),
            request_id: "given".to_owned(),
        })
        .await
        .unwrap();
    volumes
        .delete_volume(DeleteVolumeRequest {
            name: name.clone(),
            ..Default::default()
        })
        .await
        .unwrap();

    let calls = calls.lock().unwrap();
    let [list, update, imported, deleted] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    assert_eq!(list.method, "GET");
    assert_eq!(list.path, format!("/v1beta1/{ZONE}/volumes"));
    assert!(list.body.is_empty());
    assert_eq!(
        list.query_pairs(),
        [
            ("pageSize".to_owned(), "5".to_owned()),
            ("pageToken".to_owned(), "next".to_owned())
        ]
    );

    // body: "volume": the volume is the body, the rest goes in the query.
    assert_eq!(update.method, "PATCH");
    assert_eq!(update.path, format!("/v1beta1/{name}"));
    assert_eq!(json_of(update), json!({"name": name, "sizeGib": 2}));
    assert_eq!(update.query_values("updateMask"), ["sizeGib,tags"]);
    assert!(is_uuid(&update.query_values("requestId")[0]), "{update:?}");

    // body: "*": everything is the body, nothing the query.
    assert_eq!(imported.method, "POST");
    assert_eq!(imported.path, format!("/v1beta1/{ZONE}/volumes:import"));
    assert!(imported.query.is_empty());
    assert_eq!(
        json_of(imported),
        json!({"parent": ZONE, "volume": {"sizeGib": 3}, "sourceUrl": "https://example.com/disk.img", "requestId": "given"})
    );

    assert_eq!(deleted.method, "DELETE");
    assert_eq!(deleted.path, format!("/v1beta1/{name}"));
    assert!(deleted.body.is_empty());
    assert!(
        is_uuid(&deleted.query_values("requestId")[0]),
        "{deleted:?}"
    );
}

#[tokio::test]
async fn an_empty_request_id_is_filled_in_and_a_given_one_kept() {
    let (_server, client, calls) =
        recording(r#"{"name":"projects/p/zones/z/operations/op-1"}"#).await;
    let volumes = VolumeClient::new(&client);
    let request = CreateVolumeRequest {
        parent: ZONE.to_owned(),
        volume: Some(Volume {
            size_gib: 1,
            ..Default::default()
        }),
        ..Default::default()
    };

    volumes.create_volume(request.clone()).await.unwrap();
    volumes.create_volume(request).await.unwrap();
    let calls = calls.lock().unwrap();
    let first = &calls[0].query_values("requestId")[0];
    let second = &calls[1].query_values("requestId")[0];
    assert!(is_uuid(first) && is_uuid(second), "{first} {second}");
    assert_ne!(first, second, "each call gets its own request id");
    assert_eq!(json_of(&calls[0]), json!({"sizeGib": 1}));
}

#[tokio::test]
async fn a_create_without_a_volume_sends_an_empty_one() {
    let (_server, client, calls) =
        recording(r#"{"name":"projects/p/zones/z/operations/op-1"}"#).await;
    VolumeClient::new(&client)
        .create_volume(CreateVolumeRequest {
            parent: ZONE.to_owned(),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(calls.lock().unwrap()[0].text(), "{}");
}

#[tokio::test]
async fn a_name_is_escaped_once() {
    let (_server, client, calls) = recording("{}").await;
    VolumeClient::new(&client)
        .get_volume(GetVolumeRequest {
            name: "projects/a b%/zones/z/volumes/v".to_owned(),
        })
        .await
        .unwrap();
    assert_eq!(
        calls.lock().unwrap()[0].path,
        "/v1beta1/projects/a%20b%25/zones/z/volumes/v"
    );
}

#[tokio::test]
async fn a_name_of_the_wrong_form_is_refused_before_sending() {
    let (_server, client, calls) = recording("{}").await;
    let err = VolumeClient::new(&client)
        .get_volume(GetVolumeRequest {
            name: "vol-1".to_owned(),
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("name must have the form projects/*/zones/*/volumes/*"),
        "{err}"
    );
    assert!(calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_response_is_read_with_proto3_json() {
    let (_server, client, _) = recording(
        r#"{"name":"projects/p/zones/z/volumes/vol-1","sizeGib":10,"state":"AVAILABLE","resizeState":"RESIZE_COMPLETED","tags":{"origin:name":"data"},"aFieldFromANewerApi":1,"creator":"user:u1"}"#,
    )
    .await;
    let volume = VolumeClient::new(&client)
        .get_volume(GetVolumeRequest {
            name: format!("{ZONE}/volumes/vol-1"),
        })
        .await
        .unwrap();
    assert_eq!(volume.size_gib, 10);
    assert_eq!(
        volume.state(),
        origincloud_pvs::v1beta1::volume::State::Available
    );
    assert_eq!(
        volume.resize_state(),
        origincloud_pvs::v1beta1::volume::ResizeState::ResizeCompleted
    );
    assert_eq!(volume.tags["origin:name"], "data");
    assert_eq!(volume.creator, "user:u1");
}

#[tokio::test]
async fn all_volumes_follows_the_pages() {
    let server = FakeServer::start(|r| async move {
        let token = r.query_values("pageToken").pop().unwrap_or_default();
        Response::json(match token.as_str() {
            "" => format!(
                r#"{{"volumes":[{{"name":"{ZONE}/volumes/vol-1"}},{{"name":"{ZONE}/volumes/vol-2"}}],"nextPageToken":"p2"}}"#
            ),
            "p2" => format!(r#"{{"volumes":[{{"name":"{ZONE}/volumes/vol-3"}}]}}"#),
            other => panic!("page token {other}"),
        })
    })
    .await;
    let client = client(&server);
    let mut pages = VolumeClient::new(&client).all_volumes(ListVolumesRequest {
        parent: ZONE.to_owned(),
        ..Default::default()
    });
    let mut names = Vec::new();
    while let Some(volume) = pages.next().await {
        names.push(volume.unwrap().name.rsplit('/').next().unwrap().to_owned());
    }
    assert_eq!(names, ["vol-1", "vol-2", "vol-3"]);
}

#[tokio::test]
async fn a_service_without_a_parent_is_sent_without_one() {
    let (_server, client, calls) =
        recording(r#"{"storageClasses":[{"name":"storageClasses/gp1"}]}"#).await;
    let classes = StorageClassClient::new(&client)
        .list_storage_classes(ListStorageClassesRequest::default())
        .await
        .unwrap();
    assert_eq!(classes.storage_classes[0].name, "storageClasses/gp1");
    let calls = calls.lock().unwrap();
    assert_eq!(calls[0].method, "GET");
    assert_eq!(calls[0].path, "/v1beta1/storageClasses");
}

#[test]
fn the_clients_use_the_service_s_regional_host() {
    let client = Client::builder()
        .credentials(AccessToken::new("t"))
        .region("eu-central1")
        .build()
        .unwrap();
    let volumes = VolumeClient::new(&client);
    assert_eq!(
        volumes.service().base_url().as_str(),
        "https://pvs.eu-central1.origincloudapis.eu/"
    );
}
