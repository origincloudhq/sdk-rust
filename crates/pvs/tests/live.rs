//! The SDK against the live API. These tests are ignored unless asked for
//! with `--ignored`, and then need `ORIGINCLOUD_LIVE_ZONE` to name a zone to
//! work in, `projects/{project}/zones/{zone}`, with credentials found the
//! usual way (see `origincloud_core::default_credentials`); without a zone
//! they fail. They create resources there and delete them again.
//!
//! ```sh
//! ORIGINCLOUD_LIVE_ZONE=projects/<project>/zones/<zone> \
//!   cargo test -p origincloud-pvs --test live -- --ignored --nocapture
//! ```

use std::collections::BTreeMap;
use std::time::Duration;

use origincloud_core::protobuf::FieldMask;
use origincloud_core::{Client, Error};
use origincloud_pvs::v1beta1::{
    CreateVolumeRequest, DeleteVolumeRequest, GetVolumeRequest, ListVolumesRequest,
    UpdateVolumeRequest, Volume, VolumeClient, volume,
};

fn live_zone() -> String {
    let zone = std::env::var("ORIGINCLOUD_LIVE_ZONE").unwrap_or_default();
    assert!(
        !zone.is_empty(),
        "the live tests need ORIGINCLOUD_LIVE_ZONE=projects/<project>/zones/<zone>"
    );
    zone
}

/// A volume is created, read straight away, grown and deleted, waiting on
/// each operation.
#[tokio::test]
#[ignore = "against the live API: run with --ignored and ORIGINCLOUD_LIVE_ZONE"]
async fn volume_lifecycle() {
    let zone = live_zone();
    let client = Client::builder()
        .user_agent("origincloud-sdk-rust-livetest")
        .build()
        .expect("credentials");
    let volumes = VolumeClient::new(&client);

    let created = tokio::time::timeout(Duration::from_secs(600), async {
        volumes
            .create_volume(CreateVolumeRequest {
                parent: zone.clone(),
                volume: Some(Volume {
                    size_gib: 1,
                    tags: BTreeMap::from([(
                        "origin:name".to_owned(),
                        "sdk-rust-livetest".to_owned(),
                    )]),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .await?
            .wait()
            .await
    })
    .await
    .expect("the create to finish in time")
    .expect("the create");
    eprintln!("created {}", created.name);

    let mut deleted = false;
    let outcome = tokio::time::timeout(
        Duration::from_secs(900),
        lifecycle(&volumes, &zone, &created, &mut deleted),
    )
    .await
    .unwrap_or_else(|_| Err("timed out".to_owned()));
    if !deleted {
        let cleanup = async {
            volumes
                .delete_volume(DeleteVolumeRequest {
                    name: created.name.clone(),
                    ..Default::default()
                })
                .await?
                .wait()
                .await
        };
        if let Err(e) = tokio::time::timeout(Duration::from_secs(300), cleanup).await {
            eprintln!("cleanup: deleting {}: {e}", created.name);
        }
    }
    outcome.unwrap();
}

async fn lifecycle(
    volumes: &VolumeClient,
    zone: &str,
    created: &Volume,
    deleted: &mut bool,
) -> Result<(), String> {
    let fail = |what: &str, e: Error| format!("{what}: {e}");
    if created.state() != volume::State::Available || created.size_gib != 1 {
        return Err(format!("created {created:?}"));
    }
    let get = || {
        volumes.get_volume(GetVolumeRequest {
            name: created.name.clone(),
        })
    };

    let got = get()
        .await
        .map_err(|e| fail("get right after the create", e))?;
    if got.state() != volume::State::Available {
        return Err(format!("got {got:?} right after the create"));
    }

    let resized = volumes
        .update_volume(UpdateVolumeRequest {
            volume: Some(Volume {
                name: created.name.clone(),
                size_gib: 2,
                ..Default::default()
            }),
            update_mask: Some(FieldMask {
                paths: vec!["size_gib".to_owned()],
            }),
            ..Default::default()
        })
        .await
        .map_err(|e| fail("resize", e))?
        .wait()
        .await
        .map_err(|e| fail("waiting for the resize", e))?;
    if resized.size_gib != 2 {
        return Err(format!("resized to {} GiB, want 2", resized.size_gib));
    }
    let got = get()
        .await
        .map_err(|e| fail("get right after the resize", e))?;
    if got.size_gib != 2 {
        return Err(format!("got {got:?} right after the resize"));
    }

    volumes
        .delete_volume(DeleteVolumeRequest {
            name: created.name.clone(),
            ..Default::default()
        })
        .await
        .map_err(|e| fail("delete", e))?
        .wait()
        .await
        .map_err(|e| fail("waiting for the delete", e))?;
    *deleted = true;
    let got = get()
        .await
        .map_err(|e| fail("get right after the delete", e))?;
    if got.state() != volume::State::Deleted {
        return Err(format!("got {got:?} right after the delete"));
    }

    let mut pages = volumes.all_volumes(ListVolumesRequest {
        parent: zone.to_owned(),
        ..Default::default()
    });
    while let Some(volume) = pages.next().await {
        let volume = volume.map_err(|e| fail("list", e))?;
        if volume.name == created.name {
            return Err(format!("the deleted volume {} is listed", volume.name));
        }
    }
    Ok(())
}
