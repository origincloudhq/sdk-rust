use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

use serde::Deserialize;

use crate::rpc::{Any, Status};

/// What a call, or waiting on an operation, failed with.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The API refused the call, or the operation failed: branch on
    /// [`ApiError::reason`].
    #[error(transparent)]
    Api(Box<ApiError>),
    /// The call got no answer: the connection failed or broke off.
    #[error("origincloud: {0}")]
    Transport(#[source] reqwest::Error),
    /// Neither the environment nor a CLI session holds credentials, and
    /// no instance metadata service answered.
    #[error(
        "origincloud: no credentials: set ORIGINCLOUD_CLIENT_ID and ORIGINCLOUD_CLIENT_SECRET, set ORIGINCLOUD_ACCESS_TOKEN, grant a GitHub Actions job `permissions: id-token: write` or set ORIGINCLOUD_OIDC_TOKEN_FILE to exchange its OIDC token, log in with `origin id login`, or run on an instance that runs as a service account"
    )]
    NoCredentials,
    /// The credentials couldn't supply an access token.
    #[error("origincloud: {0}")]
    Credentials(String),
    /// The client's configuration is invalid.
    #[error("origincloud: {0}")]
    Config(String),
    /// The request can't be sent as it is, such as a name of the wrong
    /// form; it was not sent.
    #[error("origincloud: {0}")]
    InvalidRequest(String),
    /// The answer couldn't be read.
    #[error("origincloud: {0}")]
    Decode(String),
    /// [`Operation::result`](crate::Operation::result) of an operation that
    /// isn't done.
    #[error("origincloud: operation {0} isn't done")]
    NotDone(String),
}

impl Error {
    /// The refusal, when the API refused the call or the operation failed.
    pub fn api(&self) -> Option<&ApiError> {
        match self {
            Self::Api(e) => Some(e),
            _ => None,
        }
    }

    /// The platform's error code, such as `NOT_FOUND`, when the API refused
    /// the call with one.
    pub fn reason(&self) -> Option<&str> {
        self.api()
            .map(|e| e.reason.as_str())
            .filter(|r| !r.is_empty())
    }

    /// Whether the API answered that the same call may succeed if it is
    /// sent again.
    pub fn is_retryable(&self) -> bool {
        self.api().is_some_and(ApiError::is_retryable)
    }
}

impl From<ApiError> for Error {
    fn from(e: ApiError) -> Self {
        Self::Api(Box::new(e))
    }
}

/// A call the API refused, or an operation that failed.
#[derive(Clone, Debug, Default, PartialEq)]
#[non_exhaustive]
pub struct ApiError {
    /// The response's HTTP status code; `None` for an operation's error.
    pub http_status: Option<u16>,
    /// The closest canonical code, such as `NOT_FOUND`, for generic
    /// tooling.
    pub status: String,
    /// The platform's error code, such as `NOT_FOUND` or `CONFLICT`: what
    /// to branch on.
    pub reason: String,
    /// More about the reason, by name, such as `serviceAccount`, the name
    /// of the service account an earlier request with the same request id
    /// created; empty when the API sends none.
    pub metadata: HashMap<String, String>,
    /// What went wrong, for a person.
    pub message: String,
    /// Identifies the request to Origin Cloud's support.
    pub request_id: String,
    /// The request fields that were refused.
    pub field_violations: Vec<FieldViolation>,
    /// How long the API asks to wait before trying again; `None` when it
    /// doesn't ask.
    pub retry_delay: Option<Duration>,
    retryable: bool,
}

/// One request field the API refused.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct FieldViolation {
    /// The field's JSON path, such as `volume.sizeGib`.
    pub field: String,
    /// Why it was refused.
    pub description: String,
}

impl ApiError {
    /// Whether the API answered that the same call may succeed if it is
    /// sent again.
    pub fn is_retryable(&self) -> bool {
        self.retryable
    }

    /// The error a response with a status other than 200 carries. A
    /// response that carries none, such as a proxy's, is an error by its
    /// status alone, retryable when the status is.
    pub(crate) fn from_response(http_status: u16, body: &[u8]) -> Self {
        #[derive(Deserialize)]
        struct Body {
            error: Option<StatusBody>,
        }
        #[derive(Deserialize)]
        #[serde(default)]
        #[derive(Default)]
        struct StatusBody {
            status: String,
            message: String,
            details: Vec<serde_json::Value>,
        }

        let parsed = serde_json::from_slice::<Body>(body)
            .ok()
            .and_then(|b| b.error);
        let Some(parsed) = parsed else {
            let mut e = Self {
                http_status: Some(http_status),
                message: reqwest::StatusCode::from_u16(http_status)
                    .ok()
                    .and_then(|s| s.canonical_reason())
                    .unwrap_or_default()
                    .to_owned(),
                ..Self::default()
            };
            if matches!(http_status, 502..=504) {
                e.status = "UNAVAILABLE".to_owned();
                e.retryable = true;
            }
            return e;
        };
        let mut e = Self {
            http_status: Some(http_status),
            status: parsed.status,
            message: parsed.message,
            ..Self::default()
        };
        // A detail that isn't a message is skipped, like one of an unknown
        // type.
        let details: Vec<Any> = parsed
            .details
            .into_iter()
            .filter_map(|d| serde_json::from_value(d).ok())
            .collect();
        e.read_details(&details, true);
        e
    }

    /// The error a failed operation carries.
    pub(crate) fn from_status(status: &Status) -> Self {
        let mut e = Self {
            status: code_name(status.code).to_owned(),
            message: status.message.clone(),
            ..Self::default()
        };
        e.read_details(&status.details, false);
        e
    }

    fn read_details(&mut self, details: &[Any], retry_info: bool) {
        for detail in details {
            match detail.type_name() {
                "google.rpc.ErrorInfo" => {
                    self.reason = detail.string("reason").to_owned();
                    let metadata = detail
                        .fields()
                        .get("metadata")
                        .and_then(serde_json::Value::as_object);
                    self.metadata = metadata
                        .into_iter()
                        .flatten()
                        .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_owned())))
                        .collect();
                }
                "google.rpc.RequestInfo" => self.request_id = detail.string("requestId").to_owned(),
                "google.rpc.RetryInfo" if retry_info => {
                    self.retryable = true;
                    self.retry_delay = parse_duration(detail.string("retryDelay"));
                }
                "google.rpc.BadRequest" => {
                    let violations = detail
                        .fields()
                        .get("fieldViolations")
                        .and_then(serde_json::Value::as_array);
                    for v in violations.into_iter().flatten() {
                        let text = |name: &str| {
                            v.get(name)
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or_default()
                                .to_owned()
                        };
                        self.field_violations.push(FieldViolation {
                            field: text("field"),
                            description: text("description"),
                        });
                    }
                }
                _ => {}
            }
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("origincloud: ")?;
        if !self.reason.is_empty() {
            f.write_str(&self.reason)?;
        } else if !self.status.is_empty() {
            f.write_str(&self.status)?;
        } else {
            write!(f, "HTTP {}", self.http_status.unwrap_or_default())?;
        }
        if !self.message.is_empty() {
            write!(f, ": {}", self.message)?;
        }
        if !self.request_id.is_empty() {
            write!(f, " (request {})", self.request_id)?;
        }
        Ok(())
    }
}

impl std::error::Error for ApiError {}

/// A JSON duration, such as `1.5s`.
fn parse_duration(text: &str) -> Option<Duration> {
    let seconds: f64 = text.strip_suffix('s')?.parse().ok()?;
    Duration::try_from_secs_f64(seconds).ok()
}

/// The name of a canonical code (`google.rpc.Code`).
fn code_name(code: i32) -> &'static str {
    match code {
        0 => "OK",
        1 => "CANCELLED",
        2 => "UNKNOWN",
        3 => "INVALID_ARGUMENT",
        4 => "DEADLINE_EXCEEDED",
        5 => "NOT_FOUND",
        6 => "ALREADY_EXISTS",
        7 => "PERMISSION_DENIED",
        8 => "RESOURCE_EXHAUSTED",
        9 => "FAILED_PRECONDITION",
        10 => "ABORTED",
        11 => "OUT_OF_RANGE",
        12 => "UNIMPLEMENTED",
        13 => "INTERNAL",
        14 => "UNAVAILABLE",
        15 => "DATA_LOSS",
        16 => "UNAUTHENTICATED",
        _ => "UNKNOWN",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BAD_REQUEST: &str = r#"{"error":{"code":400,"status":"INVALID_ARGUMENT","message":"invalid input on field volume.sizeGib: must be at least 1","details":[
        {"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"BAD_REQUEST","domain":"origincloudapis.eu"},
        {"@type":"type.googleapis.com/google.rpc.RequestInfo","requestId":"4bf92f3577b34da6"},
        {"@type":"type.googleapis.com/google.rpc.BadRequest","fieldViolations":[{"field":"volume.sizeGib","description":"must be at least 1"}]}]}}"#;

    #[test]
    fn a_status_is_read_with_its_details() {
        let e = ApiError::from_response(400, BAD_REQUEST.as_bytes());
        assert_eq!(e.http_status, Some(400));
        assert_eq!(e.status, "INVALID_ARGUMENT");
        assert_eq!(e.reason, "BAD_REQUEST");
        assert_eq!(e.request_id, "4bf92f3577b34da6");
        assert_eq!(
            e.message,
            "invalid input on field volume.sizeGib: must be at least 1"
        );
        assert_eq!(
            e.field_violations,
            vec![FieldViolation {
                field: "volume.sizeGib".to_owned(),
                description: "must be at least 1".to_owned()
            }]
        );
        assert!(e.metadata.is_empty());
        assert!(!e.is_retryable());
        assert_eq!(
            e.to_string(),
            "origincloud: BAD_REQUEST: invalid input on field volume.sizeGib: must be at least 1 (request 4bf92f3577b34da6)"
        );
    }

    #[test]
    fn error_info_metadata_is_read() {
        let body = r#"{"error":{"code":409,"status":"FAILED_PRECONDITION","message":"already created","details":[
            {"@type":"type.googleapis.com/google.rpc.ErrorInfo","reason":"CONFLICT","domain":"origincloudapis.eu","metadata":{"serviceAccount":"organizations/o/serviceAccounts/s","notText":1}}]}}"#;
        let e = ApiError::from_response(409, body.as_bytes());
        assert_eq!(e.reason, "CONFLICT");
        assert_eq!(
            e.metadata,
            HashMap::from([(
                "serviceAccount".to_owned(),
                "organizations/o/serviceAccounts/s".to_owned()
            )])
        );
    }

    #[test]
    fn retry_info_makes_it_retryable_with_its_delay() {
        let body = r#"{"error":{"code":503,"status":"UNAVAILABLE","message":"behind","details":[
            {"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"1.500s"}]}}"#;
        let e = ApiError::from_response(503, body.as_bytes());
        assert!(e.is_retryable());
        assert_eq!(e.retry_delay, Some(Duration::from_millis(1500)));
    }

    #[test]
    fn a_proxy_error_is_known_by_its_status() {
        let e = ApiError::from_response(502, b"<html>bad gateway</html>");
        assert!(e.is_retryable());
        assert_eq!(e.status, "UNAVAILABLE");
        assert_eq!(e.message, "Bad Gateway");
        let e = ApiError::from_response(500, b"");
        assert!(!e.is_retryable());
        assert_eq!(
            e.to_string(),
            "origincloud: HTTP 500: Internal Server Error"
        );
    }
}
