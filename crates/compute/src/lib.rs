//! The Origin Cloud compute API: instances, images, key pairs, volume attachments and the instance catalog.
//!
//! The clients are generated from the API definitions, `origincloud.compute`;
//! what they share, such as credentials, retries and errors, is in
//! [`origincloud_core`].

mod generated;

pub use generated::*;
