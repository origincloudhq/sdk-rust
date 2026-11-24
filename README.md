# Origin Cloud SDK for Rust

The Rust client for Origin Cloud's REST APIs. Its service crates are generated
from the API definitions in
[origincloudhq/api](https://github.com/origincloudhq/api); `origincloud-core` is
what they share: credentials, endpoints, retries, errors, long-running
operations and read-your-writes.

```toml
[dependencies]
origincloud-core = "0.1"
origincloud-pvs = "0.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

| Crate | API |
|---|---|
| [`origincloud-core`](crates/core) | The client every service crate uses, and `longrunning::v1beta1` for resuming an operation by name |
| [`origincloud-pvs`](crates/pvs) | Persistent volumes: volumes, snapshots, storage classes (`origincloud_pvs::v1beta1`) |
| [`origincloud-location`](crates/location) | Regions and zones, and the services each zone offers (`origincloud_location::v1beta1`) |
| [`origincloud-compute`](crates/compute) | Instances, images, key pairs, volume attachments and the instance catalog (`origincloud_compute::v1beta1`) |
| [`origincloud-network`](crates/network) | VPCs, subnets, routing, gateways, floating IPs, network interfaces, security groups, ACLs and DNS policies (`origincloud_network::v1beta1`) |
| [`origincloud-resourcemanager`](crates/resourcemanager) | Organizations, projects and their quotas (`origincloud_resourcemanager::v1beta1`) |
| [`origincloud-iam`](crates/iam) | Users, service accounts, roles, role bindings, permissions and the audit log (`origincloud_iam::v1beta1`) |
| [`origincloud-billing`](crates/billing) | Billing accounts and the projects they pay for, payment methods, cost and usage, spend limits and the price list (`origincloud_billing::v1beta1`) |
| [`origincloud-observability`](crates/observability) | The metrics the platform collects for a project's compute instances, volumes and network interfaces (`origincloud_observability::v1beta1`) |
| [`origincloud-support`](crates/support) | Support cases, each a conversation between an organization and Origin Cloud support about a question or a problem (`origincloud_support::v1beta1`) |
| [`origincloud-dns`](crates/dns) | Hosted zones, their record sets, and private zones' VPC attachments (`origincloud_dns::v1beta1`) |
| [`origincloud-sts`](crates/sts) | The security token service: it turns a token from an identity provider outside Origin Cloud, such as a CI job's OIDC token, into the access token of a WORKLOAD service account that trusts it through a federated credential (`origincloud_sts::v1beta1`) |
| [`origincloud-kms`](crates/kms) | Grants: permission for one platform host to use one key for one operation and one encryption context, for at most an hour at a time (`origincloud_kms::v1beta1`) |

## Example

```rust,no_run
use origincloud_core::protobuf::FieldMask;
use origincloud_core::Client;
use origincloud_pvs::v1beta1::{
    CreateVolumeRequest, DeleteVolumeRequest, UpdateVolumeRequest, Volume, VolumeClient,
};

#[tokio::main]
async fn main() -> Result<(), origincloud_core::Error> {
    let client = Client::new()?;
    let volumes = VolumeClient::new(&client);

    let volume = volumes
        .create_volume(CreateVolumeRequest {
            parent: "projects/my-project-123/zones/eu-central1-az1".to_owned(),
            volume: Some(Volume { size_gib: 10, ..Default::default() }),
            ..Default::default()
        })
        .await?
        .wait()
        .await?;

    let volume = volumes
        .update_volume(UpdateVolumeRequest {
            volume: Some(Volume { name: volume.name, size_gib: 20, ..Default::default() }),
            update_mask: Some(FieldMask::new(["size_gib"])),
            ..Default::default()
        })
        .await?
        .wait()
        .await?;
    println!("{} has {} GiB", volume.name, volume.size_gib);

    volumes
        .delete_volume(DeleteVolumeRequest { name: volume.name, ..Default::default() })
        .await?
        .wait()
        .await?;
    Ok(())
}
```

## Credentials

`Client::new` takes the first of:

1. `ORIGINCLOUD_ACCESS_TOKEN`: an access token, sent as it is.
2. `ORIGINCLOUD_CREDENTIALS_FILE`: the path of a service account key's
   credentials file (see [A service account key](#a-service-account-key)).
   An assertion signed with the key is exchanged at the security token
   service for the account's access tokens, again a minute before each
   expires. This is what automation should use where it can't present an
   OIDC token (step 3) and doesn't run on an instance (step 5). A file that
   can't be read is an error, not passed over.
3. A CI job's OIDC token, exchanged at the security token service for the
   access token of the `WORKLOAD` service account whose federated credential
   trusts the job: no secret to store. In a GitHub Actions job granted
   `permissions: id-token: write` there is nothing to configure; elsewhere,
   `ORIGINCLOUD_OIDC_TOKEN_FILE` names a file holding the token (any CI
   provider's, or a Kubernetes projected token), read again at every
   exchange, or `ORIGINCLOUD_OIDC_TOKEN` holds the token itself, as GitLab
   CI sets it for an `id_tokens` entry of that name, or else an HCP Terraform
   run's workload identity token, `TFC_WORKLOAD_IDENTITY_TOKEN` (for a
   workspace whose `TFC_WORKLOAD_IDENTITY_AUDIENCE` is
   `https://sts.origincloudapis.eu`), or `TFC_WORKLOAD_IDENTITY_TOKEN_<TAG>`
   when `ORIGINCLOUD_TFC_WORKLOAD_IDENTITY_TAG` names the tag of a tagged
   audience; the file comes first, then the variable, then HCP Terraform, then
   GitHub Actions.
   The token is exchanged again a minute before the access token expires,
   with a fresh job token; a token given in an environment variable can't
   be renewed, so it is exchanged again as it is, until it expires itself.
   Once found, this step's errors are returned rather than passed over;
   `ORIGINCLOUD_OIDC_DISABLED=true` leaves it out, `ORIGINCLOUD_STS_ENDPOINT`
   points it at another STS (default `https://sts.origincloudapis.eu`) and
   `ORIGINCLOUD_OIDC_AUDIENCE` requests GitHub's token for another audience
   (the same default).
4. The session the `origin` CLI stored at its last login (`origin id login`).
5. On an instance that runs as a service account, its credential from the
   instance's metadata service: nothing to configure, and no secret on the
   instance. Off an instance, this step fails within about two seconds;
   `ORIGINCLOUD_METADATA_DISABLED=true` leaves it out, and
   `ORIGINCLOUD_METADATA_ENDPOINT` points it at another address.

`Client::builder().credentials(...)` sets them explicitly instead.

With `ORIGINCLOUD_IMPERSONATE_SERVICE_ACCOUNT` set, or
`Client::builder().impersonate_service_account(...)`, the client acts as a
service account instead (see
[Impersonating a service account](#impersonating-a-service-account)).

### A service account key

An `EXTERNAL` service account authenticates with a key that IAM holds. Creating
one returns its credentials file once, a JSON file with the private key in it;
IAM keeps only the public half. Save it where only the software that needs it
can read it, and point the SDK at it:

```sh
export ORIGINCLOUD_CREDENTIALS_FILE=/etc/myapp/origincloud-credentials.json
```

or name it in code:

```rust,ignore
use origincloud_core::{Client, ServiceAccount};

let client = Client::builder()
    .credentials(ServiceAccount::from_file("/etc/myapp/origincloud-credentials.json")?)
    .build()?;
```

`ServiceAccount::from_json` takes the file's content instead, such as the
`credentialsFile` that `ServiceAccountKeyClient::create_service_account_key`
answers with. Each token request signs a fresh assertion with the key (`ES256`,
`kid` the key's full name, `iss` and `sub` the account's, `aud` the file's
`sts_endpoint`, valid for five minutes, with a new `jti`) and exchanges it at
the file's `sts_endpoint` (`/v1/token`, the JWT bearer grant). The access token
is reused until a minute before it expires; callers that need one while it is
being renewed wait for that renewal and share its token. Deleting the key stops
its access tokens at their next call.

The client ID and secret of earlier versions, `ORIGINCLOUD_CLIENT_ID` and
`ORIGINCLOUD_CLIENT_SECRET`, are gone: they are no longer read, and the API no
longer issues them. Create a key for the account and use its credentials file.

### Impersonating a service account

A person can act as a service account without holding its key: the SDK takes the token the credentials above give, exchanges it at
the security token service for the account's token, and sends every call with
that token. The calls then have the account's roles alone, such as a
deployment account's narrow ones, and the audit log names the account as the
principal and the caller as the actor. Name the account by its full name:

```sh
export ORIGINCLOUD_IMPERSONATE_SERVICE_ACCOUNT=//iam.origincloudapis.eu/organizations/my-org/serviceAccounts/deployer
```

or in code, on top of the person's credentials:

```rust,ignore
use origincloud_core::Client;

let client = Client::builder()
    .impersonate_service_account(
        "//iam.origincloudapis.eu/organizations/my-org/serviceAccounts/deployer",
    )
    .build()?;
```

The builder's name overrides the variable, and an empty one impersonates no
one. `origincloud_core::Impersonated` wraps any `Credentials` the same way.

- **Who may.** The caller needs the role
  `roles/iam.service-account-token-creator` on the account, its project or its
  organization. The STS impersonates `EXTERNAL` and `WORKLOAD` accounts that
  are active, and takes a person's token, such as the session of
  `origin id login`; it refuses another service account's.
- **The exchange.** `POST {sts}/v1/token`, an RFC 8693 token exchange:
  `grant_type` `urn:ietf:params:oauth:grant-type:token-exchange`, the caller's
  token as `subject_token` (`subject_token_type`
  `urn:ietf:params:oauth:token-type:access_token`), and the account's full
  name as `service_account`. The STS is `https://sts.origincloudapis.eu`, or
  the one `ORIGINCLOUD_STS_ENDPOINT` names; the caller's token is sent over
  https only (or plain http to the loopback interface, for a local test
  server).
- **Renewal.** The account's token lives five minutes at most. It is reused
  until a minute before it expires; then the caller's current token is
  exchanged again. Callers that need one while it is being renewed wait for
  that renewal and share its token.
- **Refusal.** A refused exchange fails the call with an error naming the
  account, and nothing is sent to the API: the caller's own token is never
  used in the account's place. The STS answers every refusal alike
  (`invalid_grant`), whether the binding is missing or the account doesn't
  exist or is deleted.

## Behaviour

- **Endpoints.** Every service answers at `{service}.origincloudapis.eu`,
  which serves every resource wherever it lives. `ClientBuilder::region`, or
  `ORIGINCLOUD_REGION`, picks a region's front door instead,
  `{service}.{region}.origincloudapis.eu`, for the services that have one;
  a global service, such as location, keeps its regionless host.
- **Names.** A resource's `name` is its relative name; its full resource
  name, `//{service}.origincloudapis.eu/{name}`, is what references carry.
  `origincloud_core::FullName` parses one (refusing a URN, a relative name,
  a regional or zonal host, a version), builds one with `from_relative`,
  and gives its `service`, `host`, `relative` name and `url`; principals
  and roles are iam names (`principal`, `role`). A request's `name` or
  `parent` takes either form. A full name is checked against the type the
  field refers to: the client's own service for its own resources, the
  referenced type's service otherwise (a billing account's `parent` takes
  `//resourcemanager.origincloudapis.eu/organizations/o`). It is sent as
  its relative name, in the path and the body alike, and a full name on
  any other host is refused before anything is sent.
- **Operations.** A create, update or delete returns an `Operation`; its
  `wait` returns the resource once the change is complete, or the error the
  change failed with.
- **Retries.** A call answered as retryable is sent again with a growing delay,
  when sending it again is safe: a read, or a mutation with a request id. The
  client fills in an empty request id, so a retried create never creates twice.
  `ClientBuilder::max_attempts` bounds every call; a service client's
  `with_options(CallOptions::new().max_attempts(1))` turns retries off for the
  calls made through it, for a mutation that must not be sent twice.
- **Read-your-writes.** The client keeps the consistency token each write
  answers with and sends it with later calls, so a read right after a write, or
  after waiting on an operation, sees it. The token answered last is the one
  kept: of concurrent writes, a read is sure to see the last answered. Clients
  given the same `ClientBuilder::consistency_token_store` share one token. A
  call the API refuses because its token isn't one is sent again once without
  it, and the token is dropped.
- **Pages.** Each list method has an `all_...` counterpart that streams every
  page.
- **Errors.** A refused call returns `Error::Api`; branch on `Error::reason`,
  the platform's error code. `ApiError::metadata` says more about some reasons,
  such as the name of the resource a `CONFLICT` is about.
- **Runtime.** The clients are `async`, on [Tokio](https://tokio.rs), and send
  their calls with [reqwest](https://docs.rs/reqwest); in a browser, see below.

## In a browser

The crates build for `wasm32-unknown-unknown`, for a web app written in Rust
(Leptos, Yew, Dioxus, ...). There is no Tokio there: calls go out through the
browser's `fetch`, retries wait on `setTimeout`, and the futures run on the
app's executor, such as `wasm_bindgen_futures::spawn_local`. They aren't
`Send` there (the bound is `origincloud_core::MaybeSend`, the boxed future
`MaybeSendFuture`), but `Client` and
the `Credentials` it holds still are `Send + Sync`, so a client can live in
the framework's reactive state.

A browser has no environment, files, CI token or metadata service, so only
these credentials exist there, and `Client::new` without them fails with
`Error::NoCredentials`:

- `AccessToken`: a token the page already holds, such as the user's from its
  OIDC login.
- Your own `Credentials`, for a token the page renews itself.
- `ServiceAccount` only with the feature `service-account-wasm`, for a
  wasm runtime that isn't a browser (Node.js, Deno, an edge worker), from the
  credentials file's content (`ServiceAccount::from_json`). It is left out by
  default because a browser app, its Web Workers included, is downloaded by
  whoever loads it, and a private key in it with it.

```rust,ignore
let client = Client::builder()
    .credentials(AccessToken::new(token))
    .build()?;
```

`ClientBuilder::timeout` bounds each attempt there too; a `reqwest::Client`
set with `ClientBuilder::http_client` has no connect timeout or proxy
settings in a browser, which leaves them to the browser. A response body is
read whole and then held to the same 32 MiB limit. A retry waits at most
about 24.8 days there, the longest a browser timer can, however long the
server asks.

A call to a method that authenticates no caller, such as billing's legal
documents, carries no `User-Agent` and no consistency token, only `Accept`, so
the browser sends it without a CORS preflight.

## Versions

A service crate's modules are its API's versions: `origincloud_pvs::v1beta1` is
the preview version of the PVS API and may change in incompatible ways, like
the API itself. The crates follow semantic versioning; before 1.0, a minor
version may change the Rust API incompatibly.

## Contributing

Issues and pull requests are welcome. The service crates are generated from
the API definitions, so a change to what a service offers starts as an issue;
see [CONTRIBUTING.md](CONTRIBUTING.md) for how the SDK is built and checked.

## License

Apache License 2.0; see [LICENSE](LICENSE).
