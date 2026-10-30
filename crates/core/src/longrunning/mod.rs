//! Long-running operations: what a service's creates, updates and deletes
//! answer with, until their resource settles.
//!
//! A method that starts one returns an [`Operation`](crate::Operation) to
//! wait on. This module holds the operation as the API sends it, and the
//! `OperationService` every service serves at its own host, for resuming
//! an operation by its name:
//!
//! ```no_run
//! # async fn resume(volumes: &origincloud_core::Service) -> Result<(), origincloud_core::Error> {
//! use origincloud_core::longrunning::v1beta1::{GetOperationRequest, OperationClient};
//!
//! let operations = OperationClient::new(volumes);
//! let operation = operations
//!     .get_operation(GetOperationRequest {
//!         name: "projects/my-project-123/zones/eu-central1-az1/operations/op-1".to_owned(),
//!     })
//!     .await?;
//! println!("{} done: {}", operation.name, operation.done);
//! # Ok(())
//! # }
//! ```

mod generated;

pub use generated::*;

use serde::{Deserialize, Serialize};

use crate::rpc::{Any, Status};

/// A long-running operation (`google.longrunning.Operation`), as the API
/// sends it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(from = "Wire", into = "Wire")]
pub struct Operation {
    /// The operation's resource name,
    /// `projects/{project}/zones/{zone}/operations/{operation}`.
    pub name: String,
    /// What the operation is about: an
    /// [`OperationMetadata`](v1beta1::OperationMetadata).
    pub metadata: Option<Any>,
    /// Whether it is done.
    pub done: bool,
    /// What it ended with, once it is done.
    pub result: Option<OperationResult>,
}

/// What an operation ended with.
#[derive(Clone, Debug, PartialEq)]
pub enum OperationResult {
    /// It failed with this error.
    Error(Status),
    /// It succeeded with this result: the resource it acted on, or
    /// `google.protobuf.Empty` for a delete.
    Response(Any),
}

/// An operation's JSON form.
#[derive(Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct Wire {
    #[serde(skip_serializing_if = "String::is_empty")]
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    metadata: Option<Any>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    done: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Status>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response: Option<Any>,
}

impl From<Wire> for Operation {
    fn from(w: Wire) -> Self {
        let result = match (w.error, w.response) {
            (Some(error), _) => Some(OperationResult::Error(error)),
            (None, Some(response)) => Some(OperationResult::Response(response)),
            (None, None) => None,
        };
        Self {
            name: w.name,
            metadata: w.metadata,
            done: w.done,
            result,
        }
    }
}

impl From<Operation> for Wire {
    fn from(o: Operation) -> Self {
        let (error, response) = match o.result {
            Some(OperationResult::Error(e)) => (Some(e), None),
            Some(OperationResult::Response(r)) => (None, Some(r)),
            None => (None, None),
        };
        Self {
            name: o.name,
            metadata: o.metadata,
            done: o.done,
            error,
            response,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_operation_reads_its_result() {
        let done: Operation = serde_json::from_str(
            r#"{"name":"projects/p/zones/z/operations/op-1","done":true,"response":{"@type":"type.googleapis.com/x.Y","name":"n"},"unknown":1}"#,
        )
        .unwrap();
        assert!(done.done);
        let Some(OperationResult::Response(any)) = &done.result else {
            panic!("{done:?}");
        };
        assert_eq!(any.type_name(), "x.Y");
        let again: Operation =
            serde_json::from_str(&serde_json::to_string(&done).unwrap()).unwrap();
        assert_eq!(again, done);

        let running: Operation = serde_json::from_str(r#"{"name":"n"}"#).unwrap();
        assert!(!running.done && running.result.is_none());
    }
}
