use std::time::Duration;

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderValue, USER_AGENT};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::error::ApiError;
use crate::{Error, Service};

/// The header carrying the consistency token, in responses and in
/// requests.
pub const CONSISTENCY_TOKEN_HEADER: &str = "Origin-Consistency-Token";

/// The most of a response body that is read.
const MAX_RESPONSE_BYTES: usize = 32 << 20;

const FIRST_RETRY_DELAY: Duration = Duration::from_millis(200);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(10);

/// The HTTP method of a [`Request`].
pub use reqwest::Method;

/// Query parameters, by their JSON paths.
pub type Query = Vec<(String, String)>;

/// One call of a service's method, as its HTTP binding sends it. The
/// generated clients build it.
#[derive(Clone, Debug)]
pub struct Request {
    method: Method,
    path: String,
    query: Query,
    body: Option<Vec<u8>>,
    idempotent: bool,
    /// A timeout answers the call rather than fails an attempt: it is
    /// returned at once, not retried.
    pub(crate) timeout_answers: bool,
}

impl Request {
    /// A call of `method` on `path`, the escaped path from the root.
    pub fn new(method: Method, path: impl Into<String>) -> Self {
        Self {
            method,
            path: path.into(),
            query: Query::new(),
            body: None,
            idempotent: false,
            timeout_answers: false,
        }
    }

    /// Sends `query` as the query parameters.
    pub fn with_query(mut self, query: Query) -> Self {
        self.query = query;
        self
    }

    /// Sends `body`, JSON, as the body.
    pub fn with_body(mut self, body: Vec<u8>) -> Self {
        self.body = Some(body);
        self
    }

    /// Marks the call as one whose sending twice has the effect of sending
    /// it once: a read, or a mutation with a request id. Only such a call is
    /// retried.
    pub fn idempotent(mut self) -> Self {
        self.idempotent = true;
        self
    }
}

impl Service {
    /// Sends `request`, retrying it while that is safe and the answer
    /// allows it, and reads the answer as `T`.
    pub async fn call<T: DeserializeOwned>(&self, request: Request) -> Result<T, Error> {
        let mut attempt = 1;
        loop {
            let err = match self.send(&request).await {
                Ok(answer) => {
                    return serde_json::from_slice(&answer).map_err(|e| {
                        Error::Decode(format!(
                            "reading the response to {} {}: {e}",
                            request.method, request.path
                        ))
                    });
                }
                Err(err) => err,
            };
            if request.timeout_answers && is_timeout(&err) {
                return Err(err);
            }
            if !request.idempotent || attempt >= self.client.inner.max_attempts {
                return Err(err);
            }
            let Some(delay) = retry_delay(&err, attempt) else {
                return Err(err);
            };
            tokio::time::sleep(delay).await;
            attempt += 1;
        }
    }

    /// Makes one attempt, and answers with the body of a 200.
    async fn send(&self, request: &Request) -> Result<Vec<u8>, Error> {
        let base = self.base.as_ref().map_err(|e| Error::Config(e.clone()))?;
        let mut url = base.clone();
        url.set_path(&format!(
            "{}{}",
            base.path().trim_end_matches('/'),
            request.path
        ));
        if request.query.is_empty() {
            url.set_query(None);
        } else {
            url.query_pairs_mut().clear().extend_pairs(&request.query);
        }
        let inner = &self.client.inner;
        let token = inner.credentials.token().await?;
        let mut builder = inner
            .http
            .request(request.method.clone(), url)
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .header(ACCEPT, "application/json")
            .header(USER_AGENT, &inner.user_agent);
        if let Some(timeout) = inner.timeout {
            builder = builder.timeout(timeout);
        }
        if let Some(body) = &request.body {
            builder = builder
                .header(CONTENT_TYPE, "application/json")
                .body(body.clone());
        }
        let held = self.client.consistency_token();
        if !held.is_empty() {
            builder = builder.header(CONSISTENCY_TOKEN_HEADER, held);
        }
        let mut response = builder.send().await.map_err(Error::Transport)?;
        // A failed mutation may still have applied a command: its token
        // comes back either way.
        if let Some(answered) = response
            .headers()
            .get(CONSISTENCY_TOKEN_HEADER)
            .and_then(|v: &HeaderValue| v.to_str().ok())
            .filter(|v| !v.is_empty())
        {
            self.client.set_consistency_token(answered);
        }
        let status = response.status();
        let mut answer = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(Error::Transport)? {
            answer.extend_from_slice(&chunk);
            if answer.len() >= MAX_RESPONSE_BYTES {
                answer.truncate(MAX_RESPONSE_BYTES);
                break;
            }
        }
        if status != reqwest::StatusCode::OK {
            return Err(ApiError::from_response(status.as_u16(), &answer).into());
        }
        Ok(answer)
    }
}

/// Whether `err` is an attempt that timed out.
pub(crate) fn is_timeout(err: &Error) -> bool {
    matches!(err, Error::Transport(e) if e.is_timeout())
}

/// Whether `err` may go away if the call is sent again, and how long to
/// wait before attempt `attempt + 1`: what the answer asks for, or a
/// jittered delay that doubles with each attempt, whichever is longer.
fn retry_delay(err: &Error, attempt: u32) -> Option<Duration> {
    let asked = match err {
        Error::Transport(_) => Duration::ZERO,
        Error::Api(e) if e.is_retryable() => e.retry_delay.unwrap_or_default(),
        _ => return None,
    };
    let backoff = FIRST_RETRY_DELAY
        .checked_mul(1 << attempt.saturating_sub(1).min(16))
        .map_or(MAX_RETRY_DELAY, |b| b.min(MAX_RETRY_DELAY));
    let half = backoff / 2;
    let jitter = Duration::from_nanos(fastrand::u64(
        0..=u64::try_from(half.as_nanos()).unwrap_or(u64::MAX),
    ));
    Some(asked.max(half + jitter))
}

/// What a path segment may hold unescaped: the unreserved characters, and
/// the reserved ones a segment allows.
const PATH_SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~')
    .remove(b'$')
    .remove(b'&')
    .remove(b'+')
    .remove(b':')
    .remove(b'=')
    .remove(b'@');

/// `value` as the path segments of a request, checking it has the form
/// `pattern`, such as `projects/*/zones/*/volumes/*`, gives it. `field`
/// names it in the error. The generated clients call it.
pub fn path_value(field: &str, value: &str, pattern: &str) -> Result<String, Error> {
    let got: Vec<&str> = value.split('/').collect();
    let want: Vec<&str> = pattern.split('/').collect();
    let valid = got.len() == want.len()
        && got.iter().zip(&want).all(|(g, w)| match *w {
            "*" => !g.is_empty(),
            literal => *g == literal,
        });
    if !valid {
        return Err(Error::InvalidRequest(format!(
            "{field} must have the form {pattern}, got {value:?}"
        )));
    }
    Ok(got
        .iter()
        .map(|segment| utf8_percent_encode(segment, PATH_SEGMENT).to_string())
        .collect::<Vec<_>>()
        .join("/"))
}

/// The fields of `message` as query parameters, by their JSON paths,
/// leaving out the fields `skip` names and fields at their default value.
/// A nested field is sent by its dotted path, a repeated one as one
/// parameter per element. The generated clients send the fields their
/// method's path and body don't carry this way.
pub fn query<T: Serialize>(message: &T, skip: &[&str]) -> Result<Query, Error> {
    let encoded = serde_json::to_value(message)
        .map_err(|e| Error::InvalidRequest(format!("encoding the request: {e}")))?;
    let mut query = Query::new();
    if let Value::Object(fields) = encoded {
        flatten(&mut query, "", &fields, skip)?;
    }
    query.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(query)
}

fn flatten(
    query: &mut Query,
    prefix: &str,
    fields: &serde_json::Map<String, Value>,
    skip: &[&str],
) -> Result<(), Error> {
    for (name, value) in fields {
        let path = format!("{prefix}{name}");
        if skip.contains(&path.as_str()) {
            continue;
        }
        match value {
            Value::Object(nested) => flatten(query, &format!("{path}."), nested, skip)?,
            Value::Array(elements) => {
                for element in elements {
                    query.push((path.clone(), scalar(&path, element)?));
                }
            }
            other => query.push((path.clone(), scalar(&path, other)?)),
        }
    }
    Ok(())
}

fn scalar(path: &str, value: &Value) -> Result<String, Error> {
    match value {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(b.to_string()),
        _ => Err(Error::InvalidRequest(format!(
            "{path} can't be sent as a query parameter"
        ))),
    }
}

/// `message` as a JSON body. The generated clients call it.
pub fn json_body<T: Serialize>(message: &T) -> Result<Vec<u8>, Error> {
    serde_json::to_vec(message)
        .map_err(|e| Error::InvalidRequest(format!("encoding the request: {e}")))
}

/// A new random request id, a UUID, for a mutation whose caller left its
/// request id empty. The generated clients call it.
pub fn new_request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_value_is_checked_and_escaped() {
        assert_eq!(
            path_value(
                "name",
                "projects/p 1/zones/z/volumes/vol-1",
                "projects/*/zones/*/volumes/*"
            )
            .unwrap(),
            "projects/p%201/zones/z/volumes/vol-1"
        );
        assert_eq!(
            path_value(
                "name",
                "projects/a b%/zones/z/volumes/v",
                "projects/*/zones/*/volumes/*"
            )
            .unwrap(),
            "projects/a%20b%25/zones/z/volumes/v"
        );
        for bad in [
            "",
            "projects/p/zones/z",
            "projects/p/zones//volumes/v",
            "projects/p/regions/z/volumes/v",
            "projects/p/zones/z/volumes/v/x",
        ] {
            let err = path_value("name", bad, "projects/*/zones/*/volumes/*").unwrap_err();
            assert!(
                err.to_string()
                    .contains("name must have the form projects/*/zones/*/volumes/*"),
                "{bad:?}: {err}"
            );
        }
    }

    #[test]
    fn query_flattens_by_json_path() {
        let message = serde_json::json!({
            "parent": "projects/p/zones/z",
            "pageSize": 10,
            "filter": {"state": "AVAILABLE", "ids": ["a", "b"]},
            "all": true,
        });
        let got = query(&message, &["parent"]).unwrap();
        let encoded: Vec<String> = got.iter().map(|(k, v)| format!("{k}={v}")).collect();
        assert_eq!(
            encoded.join("&"),
            "all=true&filter.ids=a&filter.ids=b&filter.state=AVAILABLE&pageSize=10"
        );
        let err = query(&serde_json::json!({"items": [{"a": 1}]}), &[]).unwrap_err();
        assert!(
            err.to_string()
                .contains("items can't be sent as a query parameter")
        );
    }

    #[test]
    fn new_request_id_is_a_version_4_uuid() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..100 {
            let id = new_request_id();
            let parsed = uuid::Uuid::parse_str(&id).unwrap();
            assert_eq!(parsed.get_version_num(), 4);
            assert_eq!(id, id.to_lowercase());
            assert!(seen.insert(id));
        }
    }

    #[test]
    fn the_retry_delay_grows_and_honours_the_answer() {
        let unavailable = Error::from(ApiError::from_response(
            503,
            br#"{"error":{"status":"UNAVAILABLE","details":[{"@type":"type.googleapis.com/google.rpc.RetryInfo","retryDelay":"3s"}]}}"#,
        ));
        for attempt in 1..=10 {
            let delay = retry_delay(&unavailable, attempt).unwrap();
            assert!(
                delay >= Duration::from_secs(3),
                "attempt {attempt}: {delay:?}"
            );
            assert!(delay <= MAX_RETRY_DELAY, "attempt {attempt}: {delay:?}");
        }
        let proxy = Error::from(ApiError::from_response(502, b""));
        let first = retry_delay(&proxy, 1).unwrap();
        assert!(
            first >= Duration::from_millis(100) && first <= Duration::from_millis(200),
            "{first:?}"
        );
        let late = retry_delay(&proxy, 30).unwrap();
        assert!(
            late >= Duration::from_secs(5) && late <= MAX_RETRY_DELAY,
            "{late:?}"
        );
        let refused = Error::from(ApiError::from_response(400, b""));
        assert_eq!(retry_delay(&refused, 1), None);
        assert_eq!(retry_delay(&Error::NoCredentials, 1), None);
    }
}
