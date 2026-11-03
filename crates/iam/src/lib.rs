//! The Origin Cloud IAM API: users, service accounts, roles, role bindings, permissions and the audit log.
//!
//! The clients are generated from the API definitions, `origincloud.iam`;
//! what they share, such as credentials, retries and errors, is in
//! [`origincloud_core`].

mod generated;

pub use generated::*;
