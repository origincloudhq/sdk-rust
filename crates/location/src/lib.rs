//! The Origin Cloud location service: the platform's regions and zones, and
//! the services each zone offers. A global service: it answers at
//! `location.origincloudapis.eu` whatever region the client has.
//!
//! The clients are generated from the API definitions; what they share,
//! such as credentials, retries and errors, is in [`origincloud_core`].
//!
//! ```no_run
//! use origincloud_location::v1beta1::{ListZonesRequest, LocationClient};
//!
//! # async fn example() -> Result<(), origincloud_core::Error> {
//! let client = origincloud_core::Client::new()?;
//! let zones = LocationClient::new(&client)
//!     .list_zones(ListZonesRequest::default())
//!     .await?
//!     .zones;
//! for zone in zones.iter().filter(|zone| zone.services.iter().any(|s| s == "pvs")) {
//!     println!("{} offers volumes", zone.name);
//! }
//! # Ok(())
//! # }
//! ```

mod generated;

pub use generated::*;
