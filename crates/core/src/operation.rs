use std::fmt;
use std::marker::PhantomData;
use std::time::Duration;

use serde::de::DeserializeOwned;

use crate::error::ApiError;
use crate::longrunning::OperationResult;
use crate::transport::is_timeout;
use crate::{Error, Method, Request, Service, longrunning, path_value};

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
    /// Each call asks the API to wait up to
    /// [`ClientBuilder::operation_wait`](crate::ClientBuilder::operation_wait),
    /// a minute by default, and less than the client's
    /// [`timeout`](crate::ClientBuilder::timeout). A call that times out
    /// all the same, such as under a shorter timeout of the HTTP client's
    /// own, counts as the operation not being done yet: the next call is
    /// sent, without using up the call's retries.
    ///
    /// It waits as long as the operation takes; to bound that, wrap it in a
    /// timeout such as `tokio::time::timeout`, which stops waiting, not the
    /// operation.
    pub async fn wait(&mut self) -> Result<T, Error> {
        let body = serde_json::json!({ "timeout": format!("{}s", self.wait_per_call().as_secs()) });
        while !self.operation.done {
            let path = path_value(
                "operation name",
                &self.operation.name,
                "projects/*/zones/*/operations/*",
            )?;
            let mut call = Request::new(Method::POST, format!("/v1beta1/{path}:wait"))
                .with_body(body.to_string().into_bytes())
                .idempotent();
            call.timeout_answers = true;
            match self.service.call(call).await {
                Ok(operation) => self.operation = operation,
                Err(err) if is_timeout(&err) => {}
                Err(err) => return Err(err),
            }
        }
        self.result()
    }

    /// How long one wait call asks the API to wait: the client's
    /// `operation_wait`, kept a second, or half, below its timeout, in
    /// whole seconds, at least one.
    fn wait_per_call(&self) -> Duration {
        let inner = &self.service.client.inner;
        let mut wait = inner.operation_wait;
        if let Some(timeout) = inner.timeout {
            let margin = Duration::from_secs(1).min(timeout / 2);
            wait = wait.min(timeout - margin);
        }
        Duration::from_secs(wait.as_secs().max(1))
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
