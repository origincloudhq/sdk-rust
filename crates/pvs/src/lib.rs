//! The Origin Cloud persistent volume service (PVS): block storage volumes,
//! their snapshots, and the storage classes they come in.
//!
//! The clients are generated from the API definitions; what they share,
//! such as credentials, retries and errors, is in
//! [`origincloud_core`].
//!
//! ```no_run
//! use origincloud_core::protobuf::FieldMask;
//! use origincloud_pvs::v1beta1::{
//!     CreateVolumeRequest, DeleteVolumeRequest, UpdateVolumeRequest, Volume, VolumeClient,
//! };
//!
//! # async fn example() -> Result<(), origincloud_core::Error> {
//! let client = origincloud_core::Client::new()?;
//! let volumes = VolumeClient::new(&client);
//!
//! let volume = volumes
//!     .create_volume(CreateVolumeRequest {
//!         parent: "projects/my-project-123/zones/eu-central1-az1".to_owned(),
//!         volume: Some(Volume { size_gib: 10, ..Default::default() }),
//!         ..Default::default()
//!     })
//!     .await?
//!     .wait()
//!     .await?;
//!
//! let volume = volumes
//!     .update_volume(UpdateVolumeRequest {
//!         volume: Some(Volume { name: volume.name, size_gib: 20, ..Default::default() }),
//!         update_mask: Some(FieldMask { paths: vec!["size_gib".to_owned()] }),
//!         ..Default::default()
//!     })
//!     .await?
//!     .wait()
//!     .await?;
//! println!("{} has {} GiB", volume.name, volume.size_gib);
//!
//! volumes
//!     .delete_volume(DeleteVolumeRequest { name: volume.name, ..Default::default() })
//!     .await?
//!     .wait()
//!     .await?;
//! # Ok(())
//! # }
//! ```

mod generated;

pub use generated::*;

/// The README's examples, compiled as tests.
#[doc = include_str!("../../../README.md")]
#[cfg(doctest)]
pub struct ReadmeDoctests;
