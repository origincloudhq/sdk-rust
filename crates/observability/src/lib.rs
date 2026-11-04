//! The Origin Cloud observability API: the metrics the platform collects for a project's compute instances, volumes and network interfaces.
//!
//! The clients are generated from the API definitions, `origincloud.observability`;
//! what they share, such as credentials, retries and errors, is in
//! [`origincloud_core`].

mod generated;

pub use generated::*;
