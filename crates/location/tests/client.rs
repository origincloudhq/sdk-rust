//! The location client calls the global host, and reads the catalog it answers.

use fake_server::{FakeServer, Response};
use origincloud_core::{AccessToken, Client};
use origincloud_location::v1beta1::{ListZonesRequest, LocationClient};

#[test]
fn a_regional_client_still_calls_the_regionless_host() {
    let client = Client::builder()
        .region("eu-central1")
        .credentials(AccessToken::new("t"))
        .build()
        .unwrap();
    let location = LocationClient::new(&client);
    assert_eq!(
        location.service().base_url().map(|url| url.as_str()),
        Some("https://location.origincloudapis.eu/")
    );
}

#[tokio::test]
async fn list_zones_reads_each_zone_and_its_services() {
    let server = FakeServer::start(|r| async move {
        assert_eq!((r.method.as_str(), r.path.as_str()), ("GET", "/v1beta1/zones"));
        Response::json(
            r#"{"zones":[{"name":"zones/eu-central1-az1","region":"regions/eu-central1","services":["pvs"],"displayName":"EU Central 1, zone 1"}]}"#,
        )
    })
    .await;
    let client = Client::builder()
        .endpoint(server.url())
        .credentials(AccessToken::new("t"))
        .build()
        .unwrap();

    let zones = LocationClient::new(&client)
        .list_zones(ListZonesRequest::default())
        .await
        .unwrap()
        .zones;
    assert_eq!(zones.len(), 1);
    assert_eq!(zones[0].region, "regions/eu-central1");
    assert_eq!(zones[0].display_name, "EU Central 1, zone 1");
    assert_eq!(zones[0].services, ["pvs"]);
}
