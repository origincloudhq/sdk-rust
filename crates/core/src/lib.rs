//! What every service crate of the Origin Cloud SDK shares: credentials,
//! endpoints, retries, errors, long-running operations and
//! read-your-writes.
//!
//! A program creates one [`Client`] and hands it to the clients of the
//! services it uses:
//!
//! ```no_run
//! # fn main() -> Result<(), origincloud_core::Error> {
//! let client = origincloud_core::Client::new()?;
//! // let volumes = origincloud_pvs::v1beta1::VolumeClient::new(&client);
//! # Ok(())
//! # }
//! ```
//!
//! # Credentials
//!
//! Unless [`ClientBuilder::credentials`] names them, [`Client::new`] takes
//! the first of (natively; in a browser there are none of these, see
//! [`credentials`]):
//!
//! 1. `ORIGINCLOUD_ACCESS_TOKEN`: an access token, sent as it is.
//! 2. `ORIGINCLOUD_CREDENTIALS_FILE`: the path of a service account key's
//!    credentials file, whose signed assertions are exchanged for access
//!    tokens as they expire
#![cfg_attr(not(target_arch = "wasm32"), doc = "   ([`ServiceAccount`]).")]
#![cfg_attr(target_arch = "wasm32", doc = "   (`ServiceAccount`).")]
//! 3. A CI job's OIDC token, exchanged at the security token service for a
//!    `WORKLOAD` service account's access tokens
#![cfg_attr(not(target_arch = "wasm32"), doc = "   ([`Federated`]):")]
#![cfg_attr(target_arch = "wasm32", doc = "   (`Federated`):")]
//!    from the
//!    file `ORIGINCLOUD_OIDC_TOKEN_FILE` names, from `ORIGINCLOUD_OIDC_TOKEN`,
//!    which holds the token itself, from an HCP Terraform run's
//!    `TFC_WORKLOAD_IDENTITY_TOKEN` (`TFC_WORKLOAD_IDENTITY_TOKEN_<TAG>` when
//!    `ORIGINCLOUD_TFC_WORKLOAD_IDENTITY_TAG` names a tag), or in a GitHub
//!    Actions job granted
//!    `permissions: id-token: write`. `ORIGINCLOUD_OIDC_DISABLED=true`
//!    leaves this step out.
//! 4. The session the `origin` CLI stored at its last login
//!    (`origin id login`).
//! 5. On an instance that runs as a service account, its credential from
//!    the instance's metadata service. `ORIGINCLOUD_METADATA_DISABLED=true`
//!    leaves this step out.
//!
#![cfg_attr(
    not(target_arch = "wasm32"),
    doc = "[`ClientBuilder::impersonate_service_account`], or
`ORIGINCLOUD_IMPERSONATE_SERVICE_ACCOUNT`, names a service account to act
as instead: the token of the credentials found is exchanged at the
security token service for the account's, which every call is sent with
([`Impersonated`]).
"
)]
//!
//! # Endpoints
//!
//! Every service answers at `{service}.origincloudapis.eu`, which serves
//! every resource wherever it lives. [`ClientBuilder::region`], or
//! `ORIGINCLOUD_REGION`, picks a region's front door instead,
//! `{service}.{region}.origincloudapis.eu`, for the services that have
//! regional ones.
//!
//! # Retries
//!
//! A call the API answers as retryable is sent again, after the delay the
//! answer asks for or a growing one, as long as sending it again is safe: a
//! read, or a mutation with a request id. The client fills in a mutation's
//! request id when it is empty, so that a retry can't apply it twice.
//! [`ClientBuilder::max_attempts`] bounds the attempts of every call;
//! [`CallOptions`] those of the calls through one service client, such as
//! a mutation that must not be sent twice at all.
//!
//! # Read-your-writes
//!
//! The API answers reads from replicas that can be a moment behind a write
//! that just succeeded. The client keeps the consistency token each
//! mutation and each finished operation answers with, and sends it with
//! every later call, so that a read through the same client shows the
//! client's own writes. The token answered last replaces the one held: for
//! writes made one after another, it covers them all; of writes made
//! concurrently, it covers the one answered last (see
//! [`Client::set_consistency_token`]). The token is kept per client, and
//! shared by its clones; clients given one store share it
//! ([`ClientBuilder::consistency_token_store`]). A call the API refuses
//! because its token isn't one is sent again once without it, and the
//! token is dropped.
//!
//! # Names
//!
//! A resource's `name` is its relative name; its full resource name,
//! `//{service}.origincloudapis.eu/{name}`, is what references carry
//! ([`FullName`]). A request's path fields take either. A full name is
//! checked against the type the field refers to (the client's own service
//! for its own resources, resourcemanager for a project or an organization
//! as a `parent`) and sent as its relative name; one on any other host is
//! refused before anything is sent.
//!
//! # Errors
//!
//! A call the API refuses returns an [`Error::Api`]: branch on
//! [`Error::reason`], the platform's error code. [`ApiError::metadata`]
//! says more about some reasons, such as the name of the resource a
//! `CONFLICT` is about.
//!
//! # Runtime
//!
//! The clients are `async` and send their calls with
//! [reqwest](https://docs.rs/reqwest), on [Tokio](https://tokio.rs).
//!
//! They also build for the browser, `wasm32-unknown-unknown`, where
//! reqwest sends calls with the browser's `fetch`, retries wait on
//! `setTimeout`, and no Tokio runtime is needed: run the futures with the
//! UI framework's executor, such as `wasm-bindgen-futures`. There, the
//! futures the SDK returns aren't `Send` ([`MaybeSend`]), the credentials
//! are the page's own (see [`credentials`]), and the per-call
//! [`ClientBuilder::timeout`] is the only one, as a browser's `fetch` has
//! no connect timeout.

#![warn(missing_docs)]

mod client;
pub mod consistency;
pub mod credentials;
mod error;
pub mod longrunning;
pub mod names;
mod operation;
mod pages;
pub mod protobuf;
pub mod rpc;
mod runtime;
mod transport;
pub mod types;

pub use client::{CallOptions, Client, ClientBuilder, Service, VERSION};
pub use consistency::{ConsistencyTokenCell, ConsistencyTokenStore};
#[cfg(any(not(target_arch = "wasm32"), feature = "service-account-wasm"))]
pub use credentials::ServiceAccount;
pub use credentials::{AccessToken, Credentials};
#[cfg(not(target_arch = "wasm32"))]
pub use credentials::{CliSession, Federated, Impersonated, InstanceMetadata, default_credentials};
pub use error::{ApiError, Error, FieldViolation};
pub use names::{FullName, FullNameError};
pub use operation::Operation;
pub use pages::Pages;
pub use runtime::{MaybeSend, MaybeSendFuture};
pub use transport::{
    CONSISTENCY_TOKEN_HEADER, HttpBody, Method, Query, Request, json_body, new_request_id,
    path_value, query,
};
