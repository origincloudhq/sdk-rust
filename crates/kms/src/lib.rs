//! The Origin Cloud KMS API: grants: permission for one platform host to use one key for one operation and one encryption context, for at most an hour at a time.
//!
//! The clients are generated from the API definitions, `origincloud.kms`;
//! what they share, such as credentials, retries and errors, is in
//! [`origincloud_core`].

mod generated;

pub use generated::*;
