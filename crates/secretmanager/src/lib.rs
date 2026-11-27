//! The Origin Cloud secretmanager API: secrets: small confidential values, such as a database password, an API key or a TLS private key, kept encrypted and versioned by the platform and read by the people and workloads you allow.
//!
//! The clients are generated from the API definitions, `origincloud.secretmanager`;
//! what they share, such as credentials, retries and errors, is in
//! [`origincloud_core`].

mod generated;

pub use generated::*;
