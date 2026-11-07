//! The Origin Cloud DNS API: hosted zones, their record sets, and private zones' VPC attachments.
//!
//! The clients are generated from the API definitions, `origincloud.dns`;
//! what they share, such as credentials, retries and errors, is in
//! [`origincloud_core`].

mod generated;

pub use generated::*;
