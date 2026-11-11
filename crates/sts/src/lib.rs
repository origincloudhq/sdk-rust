//! The Origin Cloud STS API: the security token service: it turns a token from an identity provider outside Origin Cloud, such as a CI job's OIDC token, into the access token of a WORKLOAD service account that trusts it through a federated credential.
//!
//! The clients are generated from the API definitions, `origincloud.sts`;
//! what they share, such as credentials, retries and errors, is in
//! [`origincloud_core`].

mod generated;

pub use generated::*;
