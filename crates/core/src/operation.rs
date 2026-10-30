use std::fmt;
use std::marker::PhantomData;

use serde::de::DeserializeOwned;

use crate::error::ApiError;
use crate::longrunning::OperationResult;
use crate::{Error, Method, Request, Service, longrunning, path_value};

/// The longest the API waits for an operation in one call, in its JSON
/// form.
const WAIT_PER_CALL: &str = "60s";

/// A mutation the API accepted and carries out after answering. Its
/// result, a `T`, is the resource it acted on, or
/// [`Empty`](crate::protobuf::Empty) for a delete.
pub struct Operation<T> {
    service: Service,
    operation: longrunning::Operation,
    result_type: &'static str,
    result: PhantomData<fn() -> T>,
}

impl<T: DeserializeOwned> Operation<T> {
    /// The operation `operation`, which a method of `service` answered
    /// with; its result is the message `result_type`, such as
    /// `origincloud.pvs.v1beta1.Volume`. The generated clients call it.
    pub fn new(
        service: Service,
        operation: longrunning::Operation,
        result_type: &'static str,
    ) -> Self {
        Self {
            service,
            operation,
            result_type,
            result: PhantomData,
        }
    }

    /// The operation's resource name, by which [`Operation::wait`] and a
    /// service's `OperationClient` find it.
    pub fn name(&self) -> &str {
        &self.operation.name
    }

    /// Whether the operation had finished when it was last read.
    pub fn done(&self) -> bool {
        self.operation.done
    }

    /// The operation as it was last read, its metadata included.
    pub fn raw(&self) -> &longrunning::Operation {
        &self.operation
    }

    /// Waits until the operation finishes and returns its result, or the
    /// [`Error::Api`] it failed with. Once it returns a result, reads
    /// through the same [`Client`](crate::Client) show the operation's
    /// effect.
    ///
    /// It waits as long as the operation takes; to bound that, wrap it in a
    /// timeout such as `tokio::time::timeout`, which stops waiting, not the
    /// operation.
    pub async fn wait(&mut self) -> Result<T, Error> {
        while !self.operation.done {
            let path = path_value(
                "operation name",
                &self.operation.name,
                "projects/*/zones/*/operations/*",
            )?;
            let body = serde_json::json!({ "timeout": WAIT_PER_CALL });
            let call = Request::new(Method::POST, format!("/v1beta1/{path}:wait"))
                .with_body(body.to_string().into_bytes())
                .idempotent();
            self.operation = self.service.call(call).await?;
        }
        self.result()
    }

    /// The result of an operation that is done, or the [`Error::Api`] it
    /// failed with. It fails with [`Error::NotDone`] for an operation that
    /// isn't done yet.
    pub fn result(&self) -> Result<T, Error> {
        let name = &self.operation.name;
        if !self.operation.done {
            return Err(Error::NotDone(name.clone()));
        }
        match &self.operation.result {
            Some(OperationResult::Error(status)) => Err(ApiError::from_status(status).into()),
            Some(OperationResult::Response(response)) => {
                if response.type_name() != self.result_type {
                    return Err(Error::Decode(format!(
                        "the result of operation {name} is a {}, not a {}",
                        response.type_name(),
                        self.result_type
                    )));
                }
                response.decode().map_err(|e| {
                    Error::Decode(format!("reading the result of operation {name}: {e}"))
                })
            }
            None => Err(Error::Decode(format!(
                "operation {name} is done without a result"
            ))),
        }
    }
}

impl<T> fmt::Debug for Operation<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Operation")
            .field("name", &self.operation.name)
            .field("done", &self.operation.done)
            .field("result_type", &self.result_type)
            .finish_non_exhaustive()
    }
}
