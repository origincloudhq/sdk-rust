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
//! the first of:
//!
//! 1. `ORIGINCLOUD_ACCESS_TOKEN`: an access token, sent as it is.
//! 2. `ORIGINCLOUD_CLIENT_ID` and `ORIGINCLOUD_CLIENT_SECRET`: a service
//!    account's credential, exchanged for access tokens as they expire.
//! 3. The session the `origin` CLI stored at its last login
//!    (`origin id login`).
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
//!
//! # Read-your-writes
//!
//! The API answers reads from replicas that can be a moment behind a write
//! that just succeeded. The client keeps the consistency token each
//! mutation and each finished operation answers with, and sends it with
//! every later call, so that a read through the same client always shows
//! the client's own writes.
//!
//! # Errors
//!
//! A call the API refuses returns an [`Error::Api`]: branch on
//! [`Error::reason`], the platform's error code.
//!
//! # Runtime
//!
//! The clients are `async` and send their calls with
//! [reqwest](https://docs.rs/reqwest), on [Tokio](https://tokio.rs).

#![warn(missing_docs)]

mod client;
pub mod credentials;
mod error;
pub mod longrunning;
mod operation;
mod pages;
pub mod protobuf;
pub mod rpc;
mod transport;

pub use client::{Client, ClientBuilder, Service, VERSION};
pub use credentials::{AccessToken, CliSession, Credentials, ServiceAccount, default_credentials};
pub use error::{ApiError, Error, FieldViolation};
pub use operation::Operation;
pub use pages::Pages;
pub use transport::{
    CONSISTENCY_TOKEN_HEADER, Method, Query, Request, json_body, new_request_id, path_value, query,
};
