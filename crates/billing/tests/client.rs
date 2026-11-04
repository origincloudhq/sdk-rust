//! The billing clients call the global host, send each method as its HTTP
//! binding says, and read the price list as the service answers it.

use std::sync::{Arc, Mutex};

use fake_server::{FakeServer, Received, Response};
use origincloud_billing::v1beta1::{
    BillingAccount, BillingAccountClient, CostClient, CreateBillingAccountRequest,
    GetPriceListRequest, Granularity, PriceListClient, QueryProjectCostRequest, SpendLimit,
    SpendLimitClient, UpdateSpendLimitRequest,
};
use origincloud_core::protobuf::{FieldMask, Timestamp};
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

fn at(seconds: i64, nanos: i32) -> Option<Timestamp> {
    Some(Timestamp { seconds, nanos })
}

#[test]
fn a_regional_client_still_calls_the_regionless_host() {
    let client = Client::builder()
        .region("eu-central1")
        .credentials(AccessToken::new("t"))
        .build()
        .unwrap();
    for base in [
        BillingAccountClient::new(&client).service().base_url(),
        CostClient::new(&client).service().base_url(),
        PriceListClient::new(&client).service().base_url(),
        SpendLimitClient::new(&client).service().base_url(),
    ] {
        assert_eq!(
            base.map(|url| url.as_str()),
            Some("https://billing.origincloudapis.eu/")
        );
    }
}

/// The price list answers with its fields' proto names, not their camel
/// case JSON names, and leaves out a price's end while it has none.
#[tokio::test]
async fn the_price_list_decodes_as_the_service_answers_it() {
    let (_server, client, calls) = recording(
        r#"{"currency":"EUR","amounts_exclude_vat":true,"last_changed":"2026-09-22T19:42:11.021738Z",
            "families":[{"family":"compute_instance","prices":[{"product_key":"compute.gp1.core.2",
            "dimensions":{"instance_type":"gp1.core.2"},"unit_type":"seconds","unit_count":3600,"amount":"0.0551",
            "valid_from":"2026-08-11T08:37:24.666437Z"}]}]}"#,
    )
    .await;

    let list = PriceListClient::new(&client)
        .get_price_list(GetPriceListRequest {})
        .await
        .unwrap();
    assert_eq!(list.currency, "EUR");
    assert!(list.amounts_exclude_vat);
    assert_eq!(list.last_changed, at(1_790_106_131, 21_738_000));
    let [family] = list.families.as_slice() else {
        panic!("{list:?}");
    };
    assert_eq!(family.family, "compute_instance");
    let [price] = family.prices.as_slice() else {
        panic!("{family:?}");
    };
    assert_eq!(price.product_key, "compute.gp1.core.2");
    assert_eq!(
        price.dimensions.get("instance_type").map(String::as_str),
        Some("gp1.core.2")
    );
    assert_eq!(price.unit_type, "seconds");
    assert_eq!(price.unit_count, 3600);
    assert_eq!(price.amount, "0.0551");
    assert_eq!(price.valid_from, at(1_786_437_444, 666_437_000));
    assert_eq!(price.valid_until, None);

    let calls = calls.lock().unwrap();
    let [get] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    assert_eq!(get.method, "GET");
    assert_eq!(get.path, "/v1/pricing");
    assert!(get.query.is_empty() && get.body.is_empty(), "{get:?}");
}

/// Every field of a cost query is a query parameter, the times as RFC 3339
/// and the granularity by its name.
#[tokio::test]
async fn query_project_cost_sends_its_period_in_the_query() {
    let (_server, client, calls) = recording("{}").await;

    CostClient::new(&client)
        .query_project_cost(QueryProjectCostRequest {
            project: "projects/p".to_owned(),
            start_time: at(1_793_491_200, 0),
            end_time: at(1_793_750_400, 0),
            granularity: Granularity::Daily.into(),
        })
        .await
        .unwrap();

    let calls = calls.lock().unwrap();
    let [cost] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    assert_eq!(cost.method, "GET");
    assert_eq!(cost.path, "/v1beta1/projects/p:queryCost");
    assert!(cost.body.is_empty(), "{cost:?}");
    assert_eq!(cost.query_values("startTime"), ["2026-11-01T00:00:00Z"]);
    assert_eq!(cost.query_values("endTime"), ["2026-11-04T00:00:00Z"]);
    assert_eq!(cost.query_values("granularity"), ["DAILY"]);
    assert_eq!(cost.query_pairs().len(), 3, "{cost:?}");
}

#[tokio::test]
async fn update_spend_limit_sends_the_spend_limit_and_its_mask() {
    let (_server, client, calls) = recording("{}").await;

    SpendLimitClient::new(&client)
        .update_spend_limit(UpdateSpendLimitRequest {
            spend_limit: Some(SpendLimit {
                name: "organizations/o/billingAccounts/b/spendLimit".to_owned(),
                limit: Some("250.00".to_owned()),
                alert_thresholds_percent: vec![50, 90],
                ..Default::default()
            }),
            update_mask: Some(FieldMask::new(["limit", "alert_thresholds_percent"])),
        })
        .await
        .unwrap();

    let calls = calls.lock().unwrap();
    let [updated] = calls.as_slice() else {
        panic!("{calls:?}");
    };
    // body: "spend_limit": the spend limit is the body, the mask goes in the
    // query, in camel case.
    assert_eq!(updated.method, "PATCH");
    assert_eq!(
        updated.path,
        "/v1beta1/organizations/o/billingAccounts/b/spendLimit"
    );
    assert_eq!(
        json_of(updated),
        json!({
            "name": "organizations/o/billingAccounts/b/spendLimit",
            "limit": "250.00",
            "alertThresholdsPercent": [50, 90]
        })
    );
    assert_eq!(
        updated.query_pairs(),
        [(
            "updateMask".to_owned(),
            "limit,alertThresholdsPercent".to_owned()
        )]
    );
}

/// A create without a request id gets one, so a retry after a lost answer
/// isn't a second account.
#[tokio::test]
async fn create_billing_account_is_sent_with_a_request_id() {
    let (_server, client, calls) = recording("{}").await;

    BillingAccountClient::new(&client)
        .create_billing_account(CreateBillingAccountRequest {
            parent: "organizations/o".to_owned(),
            billing_account: Some(BillingAccount {
                display_name: "Research".to_owned(),
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
    assert_eq!(created.method, "POST");
    assert_eq!(created.path, "/v1beta1/organizations/o/billingAccounts");
    assert_eq!(json_of(created), json!({"displayName": "Research"}));
    let [request_id] = created.query_values("requestId").try_into().unwrap();
    assert!(is_uuid(&request_id), "{created:?}");
    assert_eq!(created.query_pairs().len(), 1, "{created:?}");
}
