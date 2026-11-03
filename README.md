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
2. `ORIGINCLOUD_CLIENT_ID` and `ORIGINCLOUD_CLIENT_SECRET`: a service account's
   credential, exchanged for access tokens as they expire. This is what
   automation should use.
3. The session the `origin` CLI stored at its last login (`origin id login`).

`Client::builder().credentials(...)` sets them explicitly instead.

## Behaviour

- **Endpoints.** Every service answers at `{service}.origincloudapis.eu`,
  which serves every resource wherever it lives. `ClientBuilder::region`, or
  `ORIGINCLOUD_REGION`, picks a region's front door instead,
  `{service}.{region}.origincloudapis.eu`, for the services that have one;
  a global service, such as location, keeps its regionless host.
- **Operations.** A create, update or delete returns an `Operation`; its
  `wait` returns the resource once the change is complete, or the error the
  change failed with.
- **Retries.** A call answered as retryable is sent again with a growing delay,
  when sending it again is safe: a read, or a mutation with a request id. The
  client fills in an empty request id, so a retried create never creates twice.
- **Read-your-writes.** The client keeps the consistency token each write
  answers with and sends it with later calls, so a read right after a write, or
  after waiting on an operation, sees it. The token answered last is the one
  kept: of concurrent writes, a read is sure to see the last answered.
- **Pages.** Each list method has an `all_...` counterpart that streams every
  page.
- **Errors.** A refused call returns `Error::Api`; branch on `Error::reason`,
  the platform's error code. `ApiError::metadata` says more about some reasons,
  such as the name of the resource a `CONFLICT` is about.
- **Runtime.** The clients are `async`, on [Tokio](https://tokio.rs), and send
  their calls with [reqwest](https://docs.rs/reqwest).

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
