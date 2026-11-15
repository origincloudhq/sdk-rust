//! The observability client calls the region's host, sends a time series
//! query as its HTTP binding says, and reads the points it answers with.

use std::sync::{Arc, Mutex};

use fake_server::{FakeServer, Received, Response};
use origincloud_core::protobuf::{Duration, Timestamp};
use origincloud_core::{AccessToken, Client};
use origincloud_observability::v1beta1::{
    MetricClient, QueryTimeSeriesRequest, query_time_series_request,
};

const INSTANCE: &str = "projects/p/zones/eu-central1-az1/instances/i-1";

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

fn at(seconds: i64) -> Option<Timestamp> {
    Some(Timestamp { seconds, nanos: 0 })
}

/// Two metrics of an instance, from 09:00 to 10:00 on 2026-11-04, as rates
/// over five minutes.
fn query() -> QueryTimeSeriesRequest {
    QueryTimeSeriesRequest {
        parent: "projects/p".to_owned(),
        resource: INSTANCE.to_owned(),
        metrics: vec![
            "oc_compute_instance_cpu_seconds_total".to_owned(),
            "oc_compute_instance_network_receive_bytes_total".to_owned(),
        ],
        start_time: at(1_793_782_800),
        end_time: at(1_793_786_400),
        period: Some(Duration {
            seconds: 300,
            nanos: 0,
        }),
        statistic: query_time_series_request::Statistic::Rate.into(),
    }
}

/// An empty region, which overrides `ORIGINCLOUD_REGION`, is no region.
#[test]
fn a_regional_client_calls_the_regions_host() {
    for (region, host) in [
        ("", "https://observability.origincloudapis.eu/"),
        (
            "eu-central1",
            "https://observability.eu-central1.origincloudapis.eu/",
        ),
    ] {
        let client = Client::builder()
            .region(region)
            .credentials(AccessToken::new("t"))
            .build()
            .unwrap();
        assert_eq!(
            MetricClient::new(&client)
                .service()
                .base_url()
                .map(|url| url.as_str()),
            Some(host),
            "{region:?}"
        );
    }
}

/// Every field is a query parameter: each metric one, the times as RFC 3339
/// and the period as a JSON duration.
#[tokio::test]
async fn query_time_series_sends_every_field_in_the_query() {
    let (_server, client, calls) = recording("{}").await;

    MetricClient::new(&client)
        .query_time_series(query())
        .await
        .unwrap();

    let calls = calls.lock().unwrap();
    let [queried] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    assert_eq!(queried.method, "GET");
    assert_eq!(queried.path, "/v1beta1/projects/p/timeSeries:query");
    assert!(queried.body.is_empty(), "{queried:?}");
    assert_eq!(queried.query_values("resource"), [INSTANCE]);
    assert_eq!(
        queried.query_values("metrics"),
        [
            "oc_compute_instance_cpu_seconds_total",
            "oc_compute_instance_network_receive_bytes_total"
        ]
    );
    assert_eq!(queried.query_values("startTime"), ["2026-11-04T09:00:00Z"]);
    assert_eq!(queried.query_values("endTime"), ["2026-11-04T10:00:00Z"]);
    assert_eq!(queried.query_values("period"), ["300s"]);
    assert_eq!(queried.query_values("statistic"), ["RATE"]);
    assert_eq!(queried.query_pairs().len(), 7, "{queried:?}");
}

#[tokio::test]
async fn the_time_series_decode_their_points() {
    let (_server, client, _calls) = recording(
        r#"{"timeSeries":[{"metric":"oc_compute_instance_cpu_seconds_total","resource":"projects/p/zones/eu-central1-az1/instances/i-1","unit":"s/s",
            "points":[{"endTime":"2026-11-04T09:05:00Z","value":0.25},{"endTime":"2026-11-04T09:10:00Z","value":0.5}]}]}"#,
    )
    .await;

    let got = MetricClient::new(&client)
        .query_time_series(query())
        .await
        .unwrap();
    let [series] = got.time_series.as_slice() else {
        panic!("{got:?}");
    };
    assert_eq!(series.metric, "oc_compute_instance_cpu_seconds_total");
    assert_eq!(series.resource, INSTANCE);
    assert_eq!(series.unit, "s/s");
    let [first, second] = series.points.as_slice() else {
        panic!("{series:?}");
    };
    assert_eq!((first.end_time, first.value), (at(1_793_783_100), 0.25));
    assert_eq!((second.end_time, second.value), (at(1_793_783_400), 0.5));
}

/// A query's `parent` is a project, resourcemanager's: its full name is sent
/// as its relative name, the same request as `projects/p` makes.
#[tokio::test]
async fn a_parent_takes_the_full_name_of_the_project_it_refers_to() {
    let (_server, client, calls) = recording("{}").await;
    let metrics = MetricClient::new(&client);
    for parent in [
        "projects/p",
        "//resourcemanager.origincloudapis.eu/projects/p",
    ] {
        metrics
            .query_time_series(QueryTimeSeriesRequest {
                parent: parent.to_owned(),
                ..query()
            })
            .await
            .unwrap();
    }
    let calls = calls.lock().unwrap();
    let [relative, full] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    assert_eq!(full.path, "/v1beta1/projects/p/timeSeries:query");
    assert_eq!(
        (&full.method, &full.path, &full.query, &full.body),
        (
            &relative.method,
            &relative.path,
            &relative.query,
            &relative.body
        )
    );
}

/// A full name on any other host is refused before sending, naming the host
/// the field takes: the client's own service's is no project's.
#[tokio::test]
async fn a_parent_on_another_host_is_refused_before_sending() {
    let (_server, client, calls) = recording("{}").await;
    let metrics = MetricClient::new(&client);
    for parent in [
        "//pvs.origincloudapis.eu/projects/p",
        "//observability.origincloudapis.eu/projects/p",
        "//resourcemanager.eu-central1.origincloudapis.eu/projects/p",
    ] {
        let err = metrics
            .query_time_series(QueryTimeSeriesRequest {
                parent: parent.to_owned(),
                ..query()
            })
            .await
            .unwrap_err();
        assert!(
            matches!(err, origincloud_core::Error::InvalidRequest(_)),
            "{parent}: {err:?}"
        );
        assert!(
            err.to_string().contains("parent")
                && err
                    .to_string()
                    .contains("resourcemanager.origincloudapis.eu"),
            "{parent}: {err}"
        );
    }
    let err = metrics
        .query_time_series(QueryTimeSeriesRequest {
            parent: "//pvs.origincloudapis.eu/projects/p".to_owned(),
            ..query()
        })
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains(
            "parent must be a name on resourcemanager.origincloudapis.eu, got \"//pvs.origincloudapis.eu/projects/p\""
        ),
        "{err}"
    );
    assert!(calls.lock().unwrap().is_empty());
}
