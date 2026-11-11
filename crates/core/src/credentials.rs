//! Credentials: where the access token each call is sent with comes from.

use std::fmt;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use chrono::{DateTime, Utc};
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::Error;

/// The identity provider's token endpoint, where service account
/// credentials and the CLI's session are exchanged for access tokens.
pub const TOKEN_URL: &str =
    "https://auth.origincloud.eu/realms/origincloud/protocol/openid-connect/token";

const ENV_ACCESS_TOKEN: &str = "ORIGINCLOUD_ACCESS_TOKEN";
const ENV_CLIENT_ID: &str = "ORIGINCLOUD_CLIENT_ID";
const ENV_CLIENT_SECRET: &str = "ORIGINCLOUD_CLIENT_SECRET";
const ENV_METADATA_DISABLED: &str = "ORIGINCLOUD_METADATA_DISABLED";
const ENV_METADATA_ENDPOINT: &str = "ORIGINCLOUD_METADATA_ENDPOINT";
const ENV_OIDC_DISABLED: &str = "ORIGINCLOUD_OIDC_DISABLED";
const ENV_OIDC_TOKEN_FILE: &str = "ORIGINCLOUD_OIDC_TOKEN_FILE";
const ENV_OIDC_AUDIENCE: &str = "ORIGINCLOUD_OIDC_AUDIENCE";
const ENV_STS_ENDPOINT: &str = "ORIGINCLOUD_STS_ENDPOINT";
const ENV_GITHUB_REQUEST_URL: &str = "ACTIONS_ID_TOKEN_REQUEST_URL";
const ENV_GITHUB_REQUEST_TOKEN: &str = "ACTIONS_ID_TOKEN_REQUEST_TOKEN";

/// The security token service, where a CI job's OIDC token is exchanged
/// for an access token.
pub const STS_ENDPOINT: &str = "https://sts.origincloudapis.eu";

/// The audience a GitHub Actions job's OIDC token is requested for.
pub const OIDC_AUDIENCE: &str = "https://sts.origincloudapis.eu";

/// How long before its expiry an exchanged access token is replaced.
const FEDERATED_EXPIRY_MARGIN: Duration = Duration::from_secs(60);

/// The waits before an exchange the STS answered with `503` (it can't
/// check the token right now) is sent again.
const STS_RETRY_DELAYS: [Duration; 2] = [Duration::from_millis(200), Duration::from_millis(400)];

const TOKEN_EXCHANGE_GRANT: &str = "urn:ietf:params:oauth:grant-type:token-exchange";
const JWT_TOKEN_TYPE: &str = "urn:ietf:params:oauth:token-type:jwt";

/// The instance metadata service, which software on an instance reaches
/// at this link-local address.
pub const METADATA_ENDPOINT: &str = "http://169.254.169.254";

/// How long a metadata-service session is asked for: the most it grants.
const METADATA_SESSION_TTL: Duration = Duration::from_secs(6 * 60 * 60);
const METADATA_SESSION_TTL_HEADER: &str = "x-origincloud-metadata-token-ttl-seconds";
const METADATA_SESSION_HEADER: &str = "x-origincloud-metadata-token";

/// How long reaching the metadata service may take, and opening a session
/// in all: off an instance, the chain's last step fails within them.
const METADATA_CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
const METADATA_SESSION_TIMEOUT: Duration = Duration::from_secs(2);

/// How long a credential request may take.
const METADATA_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// How long before its expiry the metadata service's credential, which
/// lives five minutes, is replaced.
const METADATA_EXPIRY_MARGIN: Duration = Duration::from_secs(60);

/// The waits before a request the metadata service answered with `503`
/// (it can't sign right now) is sent again.
const METADATA_RETRY_DELAYS: [Duration; 2] =
    [Duration::from_millis(100), Duration::from_millis(200)];

/// The OAuth client the CLI logs in as, which its refresh token belongs to.
const CLI_CLIENT_ID: &str = "origin-cli";

/// How long before its expiry a token is replaced, so that it doesn't
/// expire on the way.
const EXPIRY_MARGIN: Duration = Duration::from_secs(30);

/// The most of a token response that is read.
const MAX_TOKEN_RESPONSE_BYTES: usize = 1 << 20;

/// How long a token request may take.
const TOKEN_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// What [`Credentials::token`] returns.
pub type TokenFuture<'a> = Pin<Box<dyn Future<Output = Result<String, Error>> + Send + 'a>>;

/// Credentials supply the access token each call is sent with.
pub trait Credentials: Send + Sync + fmt::Debug {
    /// A valid access token.
    fn token(&self) -> TokenFuture<'_>;
}

/// The first credentials found, in this order:
///
/// 1. `ORIGINCLOUD_ACCESS_TOKEN`: an access token, sent as it is.
/// 2. `ORIGINCLOUD_CLIENT_ID` and `ORIGINCLOUD_CLIENT_SECRET`: a service
///    account's credential, exchanged for access tokens as they expire.
///    Only one of the two set is an error.
/// 3. A CI job's OIDC token, exchanged at the security token service
///    ([`Federated`]): from the file `ORIGINCLOUD_OIDC_TOKEN_FILE` names,
///    or else from GitHub Actions, in a job granted
///    `permissions: id-token: write`. Once found, its errors are returned,
///    not passed over. `ORIGINCLOUD_OIDC_DISABLED=true` leaves this step
///    out; `ORIGINCLOUD_STS_ENDPOINT` asks another address than
///    [`STS_ENDPOINT`], and `ORIGINCLOUD_OIDC_AUDIENCE` requests GitHub's
///    token for another audience than [`OIDC_AUDIENCE`].
/// 4. The session the `origin` CLI stored at its last login
///    (`origin id login`).
/// 5. The instance's metadata service, on an instance that runs as a
///    service account ([`InstanceMetadata`]). Nothing is requested until a
///    token is; off an instance, that fails with [`Error::NoCredentials`]
///    within about two seconds. `ORIGINCLOUD_METADATA_DISABLED=true` leaves
///    this step out; `ORIGINCLOUD_METADATA_ENDPOINT` asks another address
///    than [`METADATA_ENDPOINT`].
///
/// It fails with [`Error::NoCredentials`] when the metadata service is
/// left out and there are no others.
pub fn default_credentials() -> Result<Arc<dyn Credentials>, Error> {
    from_environment(&|name| std::env::var(name).ok())
}

/// [`default_credentials`] with the environment read through `env`.
pub(crate) fn from_environment(
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Arc<dyn Credentials>, Error> {
    let var = |name: &str| env(name).filter(|v| !v.is_empty());
    if let Some(token) = var(ENV_ACCESS_TOKEN) {
        return Ok(Arc::new(AccessToken::new(token)));
    }
    match (var(ENV_CLIENT_ID), var(ENV_CLIENT_SECRET)) {
        (Some(id), Some(secret)) => return Ok(Arc::new(ServiceAccount::new(id, secret))),
        (None, None) => {}
        _ => {
            return Err(Error::Config(format!(
                "{ENV_CLIENT_ID} and {ENV_CLIENT_SECRET} must be set together"
            )));
        }
    }
    if let Some(federated) = federated_from(&var) {
        return Ok(Arc::new(federated));
    }
    if let Some(path) = cli_session_path(env)
        && let Some(session) = CliSession::load_from(path)?
    {
        return Ok(Arc::new(session));
    }
    if var(ENV_METADATA_DISABLED).is_some_and(|v| v.eq_ignore_ascii_case("true")) {
        return Err(Error::NoCredentials);
    }
    let metadata = InstanceMetadata::new();
    Ok(Arc::new(match var(ENV_METADATA_ENDPOINT) {
        Some(endpoint) => metadata.with_endpoint(endpoint),
        None => metadata,
    }))
}

/// The federated step of the chain, when the environment holds a CI job's
/// OIDC token: a token file before GitHub Actions.
fn federated_from(var: &dyn Fn(&str) -> Option<String>) -> Option<Federated> {
    if var(ENV_OIDC_DISABLED).is_some_and(|v| v.eq_ignore_ascii_case("true")) {
        return None;
    }
    let mut federated = match (
        var(ENV_OIDC_TOKEN_FILE),
        var(ENV_GITHUB_REQUEST_URL),
        var(ENV_GITHUB_REQUEST_TOKEN),
    ) {
        (Some(path), _, _) => Federated::from_file(path),
        (None, Some(url), Some(token)) => Federated::github_actions(url, token),
        _ => return None,
    };
    if let Some(endpoint) = var(ENV_STS_ENDPOINT) {
        federated = federated.with_endpoint(endpoint);
    }
    if let Some(audience) = var(ENV_OIDC_AUDIENCE) {
        federated = federated.with_audience(audience);
    }
    Some(federated)
}

/// An access token, sent as it is. It is not renewed: a service account's
/// lives five minutes.
#[derive(Clone)]
pub struct AccessToken(String);

impl AccessToken {
    /// Credentials that always send `token`.
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }
}

impl fmt::Debug for AccessToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AccessToken(..)")
    }
}

impl Credentials for AccessToken {
    fn token(&self) -> TokenFuture<'_> {
        Box::pin(async move { Ok(self.0.clone()) })
    }
}

/// A service account's client ID and secret, exchanged for access tokens
/// with the client credentials grant as they expire.
pub struct ServiceAccount {
    client_id: String,
    client_secret: String,
    token_url: String,
    http: Option<reqwest::Client>,
    cache: Mutex<Cached>,
}

impl ServiceAccount {
    /// The service account `client_id` with the secret `client_secret`.
    pub fn new(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            token_url: TOKEN_URL.to_owned(),
            http: None,
            cache: Mutex::default(),
        }
    }

    /// Requests tokens at `url` instead of [`TOKEN_URL`].
    pub fn with_token_url(mut self, url: impl Into<String>) -> Self {
        self.token_url = url.into();
        self
    }

    /// Sends the token requests with `http`, instead of the client all
    /// credentials share. Each request is bounded by 30 seconds either way.
    pub fn with_http_client(mut self, http: reqwest::Client) -> Self {
        self.http = Some(http);
        self
    }
}

impl fmt::Debug for ServiceAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServiceAccount")
            .field("client_id", &self.client_id)
            .field("token_url", &self.token_url)
            .finish_non_exhaustive()
    }
}

impl Credentials for ServiceAccount {
    fn token(&self) -> TokenFuture<'_> {
        Box::pin(async move {
            let mut cache = self.cache.lock().await;
            if let Some(token) = cache.valid() {
                return Ok(token);
            }
            let granted = request_token(
                self.http.as_ref(),
                &self.token_url,
                &[
                    ("grant_type", "client_credentials"),
                    ("client_id", &self.client_id),
                    ("client_secret", &self.client_secret),
                ],
            )
            .await?;
            cache.token = granted.access_token.clone();
            cache.expiry = Some(granted.expiry());
            Ok(granted.access_token)
        })
    }
}

/// The session the `origin` CLI stored at its last login, renewed from its
/// refresh token when its access token has expired. The renewed session
/// stays in memory: the CLI's file is never written.
pub struct CliSession {
    token_url: String,
    http: Option<reqwest::Client>,
    state: Mutex<SessionState>,
}

struct SessionState {
    cached: Cached,
    refresh_token: String,
    refresh_expiry: Option<SystemTime>,
}

/// The file the CLI stores its session in.
#[derive(Deserialize)]
struct StoredSession {
    #[serde(default)]
    access_token: String,
    #[serde(default)]
    expires: Option<DateTime<Utc>>,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    refresh_expires: Option<DateTime<Utc>>,
}

impl CliSession {
    /// The session the CLI stored, or `None` when it stored none: under
    /// `$XDG_DATA_HOME`, or `~/.local/share`, on every platform, at
    /// `origincloud/session.json`.
    pub fn load() -> Result<Option<Self>, Error> {
        match cli_session_path(&|name| std::env::var(name).ok()) {
            Some(path) => Self::load_from(path),
            None => Ok(None),
        }
    }

    /// The session stored at `path`, or `None` when there is no file.
    pub fn load_from(path: impl Into<PathBuf>) -> Result<Option<Self>, Error> {
        let path = path.into();
        let data = match std::fs::read(&path) {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(Error::Credentials(format!("reading the CLI session: {e}"))),
        };
        let stored: StoredSession = serde_json::from_slice(&data).map_err(|e| {
            Error::Credentials(format!("reading the CLI session {}: {e}", path.display()))
        })?;
        Ok(Some(Self {
            token_url: TOKEN_URL.to_owned(),
            http: None,
            state: Mutex::new(SessionState {
                cached: Cached {
                    token: stored.access_token,
                    expiry: stored.expires.map(SystemTime::from),
                },
                refresh_token: stored.refresh_token,
                refresh_expiry: stored.refresh_expires.map(SystemTime::from),
            }),
        }))
    }

    /// Renews the session at `url` instead of [`TOKEN_URL`].
    pub fn with_token_url(mut self, url: impl Into<String>) -> Self {
        self.token_url = url.into();
        self
    }

    /// Sends the renewals with `http`, instead of the client all
    /// credentials share. Each renewal is bounded by 30 seconds either way.
    pub fn with_http_client(mut self, http: reqwest::Client) -> Self {
        self.http = Some(http);
        self
    }
}

impl fmt::Debug for CliSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CliSession")
            .field("token_url", &self.token_url)
            .finish_non_exhaustive()
    }
}

impl Credentials for CliSession {
    fn token(&self) -> TokenFuture<'_> {
        Box::pin(async move {
            let mut state = self.state.lock().await;
            if let Some(token) = state.cached.valid() {
                return Ok(token);
            }
            if state
                .refresh_expiry
                .is_none_or(|expiry| SystemTime::now() >= expiry)
            {
                return Err(Error::Credentials(
                    "the CLI session has expired: log in again with `origin id login`".to_owned(),
                ));
            }
            let refresh_token = state.refresh_token.clone();
            let granted = request_token(
                self.http.as_ref(),
                &self.token_url,
                &[
                    ("grant_type", "refresh_token"),
                    ("client_id", CLI_CLIENT_ID),
                    ("refresh_token", &refresh_token),
                ],
            )
            .await
            .map_err(|e| match e {
                Error::Credentials(message) => {
                    Error::Credentials(format!("renewing the CLI session: {message}"))
                }
                other => other,
            })?;
            if !granted.refresh_token.is_empty() {
                state.refresh_token = granted.refresh_token.clone();
                state.refresh_expiry =
                    Some(SystemTime::now() + seconds(granted.refresh_expires_in));
            }
            state.cached = Cached {
                token: granted.access_token.clone(),
                expiry: Some(granted.expiry()),
            };
            Ok(granted.access_token)
        })
    }
}

/// The credential of the service account the instance runs as, from the
/// instance's metadata service: a session is opened for six hours, and each
/// access token, which lives five minutes, requested with it a minute
/// before the last one expires. The requests never go through a proxy,
/// which the metadata service refuses, and one it can't answer right now
/// (`503`) is sent again twice.
pub struct InstanceMetadata {
    endpoint: String,
    http: Option<reqwest::Client>,
    state: Mutex<MetadataState>,
}

#[derive(Default)]
struct MetadataState {
    session: Cached,
    cached: Cached,
}

/// The metadata service's credential.
#[derive(Deserialize)]
struct Issued {
    access_token: String,
    expires_at: DateTime<Utc>,
}

impl InstanceMetadata {
    /// The metadata service at [`METADATA_ENDPOINT`].
    pub fn new() -> Self {
        Self {
            endpoint: METADATA_ENDPOINT.to_owned(),
            http: None,
            state: Mutex::default(),
        }
    }

    /// Asks the metadata service at `url` instead of [`METADATA_ENDPOINT`].
    pub fn with_endpoint(mut self, url: impl Into<String>) -> Self {
        self.endpoint = url.into().trim_end_matches('/').to_owned();
        self
    }

    /// Sends the requests with `http`, as it is configured, instead of a
    /// client without a proxy that gives up connecting after a second.
    /// Opening a session is bounded by two seconds either way, and
    /// requesting a credential by five.
    pub fn with_http_client(mut self, http: reqwest::Client) -> Self {
        self.http = Some(http);
        self
    }

    fn http(&self) -> Result<&reqwest::Client, Error> {
        static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
        if let Some(http) = &self.http {
            return Ok(http);
        }
        CLIENT
            .get_or_init(|| {
                reqwest::Client::builder()
                    .no_proxy()
                    .connect_timeout(METADATA_CONNECT_TIMEOUT)
                    .build()
                    .map_err(|e| e.to_string())
            })
            .as_ref()
            .map_err(|e| Error::Credentials(format!("reaching the metadata service: {e}")))
    }

    /// A new session. Off an instance, there is no metadata service to
    /// answer, or another one: another cloud's, at the same address, which
    /// refuses the request as one it doesn't know. Either way, no
    /// credentials.
    async fn open_session(&self) -> Result<String, Error> {
        let request = self
            .http()?
            .put(format!("{}/latest/api/token", self.endpoint))
            .timeout(METADATA_SESSION_TIMEOUT)
            .header(
                METADATA_SESSION_TTL_HEADER,
                METADATA_SESSION_TTL.as_secs().to_string(),
            );
        let (status, body) = send_retrying(request, &METADATA_RETRY_DELAYS)
            .await
            .map_err(|_| Error::NoCredentials)?;
        match status.as_u16() {
            200 => Ok(String::from_utf8_lossy(&body).trim().to_owned()),
            403 => Err(refused_through_a_proxy()),
            503 => Err(Error::Credentials(
                "the metadata service refused a session (HTTP 503)".to_owned(),
            )),
            _ => Err(Error::NoCredentials),
        }
    }

    /// The credential, or `None` when the session is no longer valid.
    async fn request_credential(&self, session: &str) -> Result<Option<Issued>, Error> {
        let failed =
            |e: reqwest::Error| Error::Credentials(format!("reaching the metadata service: {e}"));
        let request = self
            .http()?
            .get(format!(
                "{}/latest/meta-data/service-account/token",
                self.endpoint
            ))
            .timeout(METADATA_REQUEST_TIMEOUT)
            .header(METADATA_SESSION_HEADER, session)
            .header(reqwest::header::ACCEPT, "application/json");
        let (status, body) = send_retrying(request, &METADATA_RETRY_DELAYS)
            .await
            .map_err(failed)?;
        match status.as_u16() {
            200 => {}
            401 => return Ok(None),
            404 => {
                return Err(Error::Credentials(
                    "the instance runs as no service account".to_owned(),
                ));
            }
            403 => return Err(refused_through_a_proxy()),
            status => {
                return Err(Error::Credentials(format!(
                    "the metadata service answered HTTP {status}"
                )));
            }
        }
        match serde_json::from_slice::<Issued>(&body) {
            Ok(issued) if !issued.access_token.is_empty() => Ok(Some(issued)),
            _ => Err(Error::Credentials(
                "the metadata service's answer is not a credential".to_owned(),
            )),
        }
    }
}

/// Sends `request`, and again after each of `delays` while it is answered
/// with `503`.
async fn send_retrying(
    request: reqwest::RequestBuilder,
    delays: &[Duration],
) -> Result<(reqwest::StatusCode, Vec<u8>), reqwest::Error> {
    let mut delays = delays.iter();
    loop {
        let attempt = request
            .try_clone()
            .expect("a credential request has no streamed body");
        let answer = read_bounded(attempt.send().await?).await?;
        match delays.next() {
            Some(delay) if answer.0 == reqwest::StatusCode::SERVICE_UNAVAILABLE => {
                tokio::time::sleep(*delay).await;
            }
            _ => return Ok(answer),
        }
    }
}

fn refused_through_a_proxy() -> Error {
    Error::Credentials(
        "the metadata service refused a request that came through a proxy".to_owned(),
    )
}

impl Default for InstanceMetadata {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for InstanceMetadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InstanceMetadata")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl Credentials for InstanceMetadata {
    fn token(&self) -> TokenFuture<'_> {
        Box::pin(async move {
            let mut state = self.state.lock().await;
            if let Some(token) = state.cached.valid_for(METADATA_EXPIRY_MARGIN) {
                return Ok(token);
            }
            // A session refused as no longer valid is opened again, once.
            for _ in 0..2 {
                let session = match state.session.valid() {
                    Some(session) => session,
                    None => {
                        let session = self.open_session().await?;
                        state.session = Cached {
                            token: session.clone(),
                            expiry: Some(SystemTime::now() + METADATA_SESSION_TTL),
                        };
                        session
                    }
                };
                match self.request_credential(&session).await? {
                    Some(issued) => {
                        state.cached = Cached {
                            token: issued.access_token.clone(),
                            expiry: Some(issued.expires_at.into()),
                        };
                        return Ok(issued.access_token);
                    }
                    None => state.session = Cached::default(),
                }
            }
            Err(Error::Credentials(
                "the metadata service answered HTTP 401".to_owned(),
            ))
        })
    }
}

/// A CI job's OIDC token, exchanged at the security token service for the
/// access token of the `WORKLOAD` service account a federated credential
/// trusts the job as. The access token is reused until a minute before it
/// expires; then a fresh job token is fetched and exchanged again. An
/// exchange the STS can't check right now (`503`) is sent again twice.
pub struct Federated {
    source: JobToken,
    endpoint: String,
    audience: String,
    http: Option<reqwest::Client>,
    cache: Mutex<Cached>,
}

/// Where the job's token comes from.
enum JobToken {
    File(PathBuf),
    GitHubActions {
        request_url: String,
        request_token: String,
    },
}

/// GitHub's answer to a job token request.
#[derive(Deserialize)]
struct GitHubToken {
    value: String,
}

impl Federated {
    /// The token in the file at `path`, read again at every exchange, so
    /// that a rotated token is picked up: any CI provider's, or a
    /// Kubernetes projected service account token.
    pub fn from_file(path: impl Into<PathBuf>) -> Self {
        Self::with_source(JobToken::File(path.into()))
    }

    /// A GitHub Actions job's token, requested at `request_url` with
    /// `request_token`: the values of `ACTIONS_ID_TOKEN_REQUEST_URL` and
    /// `ACTIONS_ID_TOKEN_REQUEST_TOKEN`, which GitHub sets in a job
    /// granted `permissions: id-token: write`.
    pub fn github_actions(
        request_url: impl Into<String>,
        request_token: impl Into<String>,
    ) -> Self {
        Self::with_source(JobToken::GitHubActions {
            request_url: request_url.into(),
            request_token: request_token.into(),
        })
    }

    fn with_source(source: JobToken) -> Self {
        Self {
            source,
            endpoint: STS_ENDPOINT.to_owned(),
            audience: OIDC_AUDIENCE.to_owned(),
            http: None,
            cache: Mutex::default(),
        }
    }

    /// Exchanges the token at the STS at `url` instead of [`STS_ENDPOINT`].
    pub fn with_endpoint(mut self, url: impl Into<String>) -> Self {
        self.endpoint = url.into().trim_end_matches('/').to_owned();
        self
    }

    /// Requests GitHub's token for `audience` instead of [`OIDC_AUDIENCE`].
    /// A token file's audience is whatever its issuer put in it.
    pub fn with_audience(mut self, audience: impl Into<String>) -> Self {
        self.audience = audience.into();
        self
    }

    /// Sends the requests with `http`, instead of the client all
    /// credentials share. Each request is bounded by 30 seconds either way.
    pub fn with_http_client(mut self, http: reqwest::Client) -> Self {
        self.http = Some(http);
        self
    }

    fn http(&self) -> Result<&reqwest::Client, Error> {
        match &self.http {
            Some(http) => Ok(http),
            None => default_http_client()
                .map_err(|e| Error::Credentials(format!("exchanging the CI job's token: {e}"))),
        }
    }

    /// A fresh job token.
    async fn job_token(&self, http: &reqwest::Client) -> Result<String, Error> {
        match &self.source {
            JobToken::File(path) => {
                let read = std::fs::read_to_string(path).map_err(|e| {
                    Error::Credentials(format!(
                        "reading the OIDC token file {}: {e}",
                        path.display()
                    ))
                })?;
                let token = read.trim();
                if token.is_empty() {
                    return Err(Error::Credentials(format!(
                        "the OIDC token file {} is empty",
                        path.display()
                    )));
                }
                Ok(token.to_owned())
            }
            JobToken::GitHubActions {
                request_url,
                request_token,
            } => {
                let failed = |reason: &dyn fmt::Display| {
                    Error::Credentials(format!(
                        "requesting the GitHub Actions job's OIDC token: {reason}; \
                         the workflow needs `permissions: id-token: write`"
                    ))
                };
                let separator = if request_url.contains('?') { '&' } else { '?' };
                let audience: String =
                    url::form_urlencoded::byte_serialize(self.audience.as_bytes()).collect();
                let response = http
                    .get(format!("{request_url}{separator}audience={audience}"))
                    .timeout(TOKEN_REQUEST_TIMEOUT)
                    .header(
                        reqwest::header::AUTHORIZATION,
                        format!("bearer {request_token}"),
                    )
                    .header(reqwest::header::ACCEPT, "application/json")
                    .send()
                    .await
                    .map_err(|e| failed(&e))?;
                let (status, body) = read_bounded(response).await.map_err(|e| failed(&e))?;
                if status != reqwest::StatusCode::OK {
                    return Err(failed(&format_args!("HTTP {}", status.as_u16())));
                }
                match serde_json::from_slice::<GitHubToken>(&body) {
                    Ok(token) if !token.value.is_empty() => Ok(token.value),
                    _ => Err(Error::Credentials(
                        "requesting the GitHub Actions job's OIDC token: the answer holds no token"
                            .to_owned(),
                    )),
                }
            }
        }
    }

    /// The job's token exchanged for an access token.
    async fn exchange(&self, http: &reqwest::Client, job_token: &str) -> Result<Granted, Error> {
        let request = http
            .post(format!("{}/v1/token", self.endpoint))
            .timeout(TOKEN_REQUEST_TIMEOUT)
            .header(reqwest::header::ACCEPT, "application/json")
            .form(&[
                ("grant_type", TOKEN_EXCHANGE_GRANT),
                ("subject_token", job_token),
                ("subject_token_type", JWT_TOKEN_TYPE),
            ]);
        let (status, body) = send_retrying(request, &STS_RETRY_DELAYS)
            .await
            .map_err(|e| Error::Credentials(format!("exchanging the CI job's token: {e}")))?;
        let parsed = serde_json::from_slice::<Granted>(&body);
        let granted = parsed.as_ref().ok();
        let error = granted.map_or("", |g| g.error.as_str());
        match status.as_u16() {
            200 => match parsed {
                Ok(granted) if !granted.access_token.is_empty() => Ok(granted),
                _ => Err(Error::Credentials(
                    "exchanging the CI job's token: the STS's answer is not a token response"
                        .to_owned(),
                )),
            },
            503 => Err(Error::Credentials(
                "exchanging the CI job's token: the STS can't check it right now \
                 (HTTP 503, 3 attempts); try again later"
                    .to_owned(),
            )),
            _ if error == "invalid_grant" => Err(Error::Credentials(
                "the STS refused the CI job's token (invalid_grant): check the federated \
                 credential's issuer, audience and conditions against the token's claims"
                    .to_owned(),
            )),
            status => {
                let reason = match granted {
                    Some(g) if !g.error_description.is_empty() => &g.error_description,
                    Some(g) if !g.error.is_empty() => &g.error,
                    _ => "not a token response",
                };
                Err(Error::Credentials(format!(
                    "exchanging the CI job's token: the STS answered HTTP {status}: {reason}"
                )))
            }
        }
    }
}

impl fmt::Debug for Federated {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("Federated");
        match &self.source {
            JobToken::File(path) => debug.field("token_file", path),
            JobToken::GitHubActions { .. } => debug.field("source", &"GitHub Actions"),
        };
        debug
            .field("endpoint", &self.endpoint)
            .field("audience", &self.audience)
            .finish_non_exhaustive()
    }
}

impl Credentials for Federated {
    fn token(&self) -> TokenFuture<'_> {
        Box::pin(async move {
            let mut cache = self.cache.lock().await;
            if let Some(token) = cache.valid_for(FEDERATED_EXPIRY_MARGIN) {
                return Ok(token);
            }
            let http = self.http()?;
            let job_token = self.job_token(http).await?;
            let granted = self.exchange(http, &job_token).await?;
            *cache = Cached {
                token: granted.access_token.clone(),
                expiry: Some(granted.expiry()),
            };
            Ok(granted.access_token)
        })
    }
}

/// Where the CLI stores its session: under `XDG_DATA_HOME`, or
/// `~/.local/share`, on every platform.
fn cli_session_path(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let base = match env("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        Some(base) => PathBuf::from(base),
        None => home_dir(env)?.join(".local").join("share"),
    };
    Some(base.join("origincloud").join("session.json"))
}

fn home_dir(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    env(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// One access token, replaced once it is about to expire.
#[derive(Default)]
struct Cached {
    token: String,
    expiry: Option<SystemTime>,
}

impl Cached {
    fn valid(&self) -> Option<String> {
        self.valid_for(EXPIRY_MARGIN)
    }

    /// The token, unless it expires within `margin`.
    fn valid_for(&self, margin: Duration) -> Option<String> {
        let expiry = self.expiry?;
        (!self.token.is_empty() && SystemTime::now() + margin < expiry).then(|| self.token.clone())
    }
}

/// A token endpoint's answer.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Granted {
    access_token: String,
    expires_in: i64,
    refresh_token: String,
    refresh_expires_in: i64,
    error: String,
    error_description: String,
}

impl Granted {
    fn expiry(&self) -> SystemTime {
        SystemTime::now() + seconds(self.expires_in)
    }
}

fn seconds(n: i64) -> Duration {
    Duration::from_secs(u64::try_from(n).unwrap_or_default())
}

/// The HTTP client token requests are sent with unless the credentials
/// name one: one for the process, created by the first request.
fn default_http_client() -> Result<&'static reqwest::Client, &'static str> {
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .build()
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(String::as_str)
}

async fn request_token(
    http: Option<&reqwest::Client>,
    url: &str,
    form: &[(&str, &str)],
) -> Result<Granted, Error> {
    let failed = |e: &dyn fmt::Display| Error::Credentials(format!("requesting a token: {e}"));
    let http = match http {
        Some(http) => http,
        None => default_http_client().map_err(|e| failed(&e))?,
    };
    let response = http
        .post(url)
        .timeout(TOKEN_REQUEST_TIMEOUT)
        .header(reqwest::header::ACCEPT, "application/json")
        .form(form)
        .send()
        .await
        .map_err(|e| failed(&e))?;
    let (status, body) = read_bounded(response).await.map_err(|e| failed(&e))?;
    let granted: Granted = serde_json::from_slice(&body).map_err(|_| {
        Error::Credentials(format!(
            "requesting a token: HTTP {}, not a token response",
            status.as_u16()
        ))
    })?;
    if status != reqwest::StatusCode::OK || granted.access_token.is_empty() {
        let reason = if granted.error_description.is_empty() {
            &granted.error
        } else {
            &granted.error_description
        };
        return Err(Error::Credentials(format!(
            "the token request was refused (HTTP {}): {reason}",
            status.as_u16()
        )));
    }
    Ok(granted)
}

/// A response's status and its body, up to the most of a token response
/// that is read.
async fn read_bounded(
    mut response: reqwest::Response,
) -> Result<(reqwest::StatusCode, Vec<u8>), reqwest::Error> {
    let status = response.status();
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_TOKEN_RESPONSE_BYTES {
            body.truncate(MAX_TOKEN_RESPONSE_BYTES);
            break;
        }
    }
    Ok((status, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |name| vars.get(name).cloned()
    }

    fn now() -> DateTime<Utc> {
        SystemTime::now().into()
    }

    fn data_home() -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("origincloud-sdk-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("origincloud")).unwrap();
        dir
    }

    fn write_session(
        dir: &std::path::Path,
        expires: DateTime<Utc>,
        refresh_expires: DateTime<Utc>,
    ) -> PathBuf {
        let path = dir.join("origincloud").join("session.json");
        let data = serde_json::json!({
            "version": 1,
            "access_token": "stored-access",
            "expires": expires.to_rfc3339(),
            "refresh_token": "stored-refresh",
            "refresh_expires": refresh_expires.to_rfc3339(),
            "token_type": "Bearer",
        });
        std::fs::write(&path, data.to_string()).unwrap();
        path
    }

    #[tokio::test]
    async fn an_access_token_in_the_environment_comes_first() {
        let credentials = from_environment(&env(&[
            (ENV_ACCESS_TOKEN, "from-env"),
            (ENV_CLIENT_ID, "id"),
            (ENV_CLIENT_SECRET, "secret"),
        ]))
        .unwrap();
        assert_eq!(credentials.token().await.unwrap(), "from-env");
    }

    #[test]
    fn a_service_account_in_the_environment_comes_before_the_cli_session() {
        let dir = data_home();
        let hour = chrono::Duration::hours(1);
        write_session(&dir, now() + hour, now() + hour);
        let credentials = from_environment(&env(&[
            ("XDG_DATA_HOME", dir.to_str().unwrap()),
            (ENV_CLIENT_ID, "id"),
            (ENV_CLIENT_SECRET, "secret"),
        ]))
        .unwrap();
        assert!(
            format!("{credentials:?}").starts_with("ServiceAccount"),
            "{credentials:?}"
        );
    }

    const GITHUB: [(&str, &str); 2] = [
        (
            ENV_GITHUB_REQUEST_URL,
            "https://github.invalid/idtoken?api-version=2.0",
        ),
        (ENV_GITHUB_REQUEST_TOKEN, "github-request-token"),
    ];

    fn with<'a>(
        base: &[(&'a str, &'a str)],
        more: &[(&'a str, &'a str)],
    ) -> Vec<(&'a str, &'a str)> {
        base.iter().chain(more).copied().collect()
    }

    #[test]
    fn a_service_account_comes_before_a_ci_job_s_token() {
        let credentials = from_environment(&env(&with(
            &GITHUB,
            &[(ENV_CLIENT_ID, "id"), (ENV_CLIENT_SECRET, "secret")],
        )))
        .unwrap();
        assert!(
            format!("{credentials:?}").starts_with("ServiceAccount"),
            "{credentials:?}"
        );
    }

    #[test]
    fn a_ci_job_s_token_comes_before_the_cli_session() {
        let dir = data_home();
        let hour = chrono::Duration::hours(1);
        write_session(&dir, now() + hour, now() + hour);
        let credentials = from_environment(&env(&with(
            &GITHUB,
            &[("XDG_DATA_HOME", dir.to_str().unwrap())],
        )))
        .unwrap();
        let debug = format!("{credentials:?}");
        assert!(debug.starts_with("Federated"), "{debug}");
        assert!(debug.contains("GitHub Actions"), "{debug}");
        assert!(debug.contains(&format!("{STS_ENDPOINT:?}")), "{debug}");
        assert!(!debug.contains("github-request-token"), "{debug}");
        assert!(!debug.contains("github.invalid"), "{debug}");
    }

    #[test]
    fn a_token_file_comes_before_github_actions() {
        let credentials = from_environment(&env(&with(
            &GITHUB,
            &[(ENV_OIDC_TOKEN_FILE, "/var/run/secrets/token")],
        )))
        .unwrap();
        let debug = format!("{credentials:?}");
        assert!(
            debug.starts_with(r#"Federated { token_file: "/var/run/secrets/token""#),
            "{debug}"
        );
    }

    #[test]
    fn github_actions_needs_both_its_variables() {
        let dir = data_home();
        for half in GITHUB {
            let credentials =
                from_environment(&env(&[half, ("XDG_DATA_HOME", dir.to_str().unwrap())])).unwrap();
            assert!(
                format!("{credentials:?}").starts_with("InstanceMetadata"),
                "{credentials:?}"
            );
        }
    }

    #[test]
    fn the_federated_step_can_be_left_out() {
        let dir = data_home();
        let credentials = from_environment(&env(&with(
            &GITHUB,
            &[
                (ENV_OIDC_TOKEN_FILE, "/var/run/secrets/token"),
                (ENV_OIDC_DISABLED, "True"),
                ("XDG_DATA_HOME", dir.to_str().unwrap()),
            ],
        )))
        .unwrap();
        assert!(
            format!("{credentials:?}").starts_with("InstanceMetadata"),
            "{credentials:?}"
        );
    }

    #[test]
    fn the_sts_and_the_audience_can_be_named() {
        let credentials = from_environment(&env(&with(
            &GITHUB,
            &[
                (ENV_STS_ENDPOINT, "http://127.0.0.1:8080/"),
                (ENV_OIDC_AUDIENCE, "https://sts.example"),
            ],
        )))
        .unwrap();
        let debug = format!("{credentials:?}");
        assert!(
            debug.contains(r#"endpoint: "http://127.0.0.1:8080", audience: "https://sts.example""#),
            "{debug}"
        );
    }

    #[test]
    fn half_a_service_account_is_refused() {
        let err = from_environment(&env(&[(ENV_CLIENT_ID, "id")])).unwrap_err();
        assert!(err.to_string().contains("must be set together"), "{err}");
        let err = from_environment(&env(&[(ENV_CLIENT_SECRET, "secret")])).unwrap_err();
        assert!(err.to_string().contains("must be set together"), "{err}");
    }

    #[test]
    fn without_others_the_metadata_service_comes_last() {
        let dir = data_home();
        let credentials =
            from_environment(&env(&[("XDG_DATA_HOME", dir.to_str().unwrap())])).unwrap();
        assert!(
            format!("{credentials:?}").starts_with("InstanceMetadata"),
            "{credentials:?}"
        );
    }

    #[test]
    fn the_metadata_service_s_address_can_be_named() {
        let dir = data_home();
        let credentials = from_environment(&env(&[
            ("XDG_DATA_HOME", dir.to_str().unwrap()),
            (ENV_METADATA_ENDPOINT, "http://127.0.0.1:8080/"),
        ]))
        .unwrap();
        assert!(
            format!("{credentials:?}").contains(r#"endpoint: "http://127.0.0.1:8080""#),
            "{credentials:?}"
        );
    }

    #[test]
    fn no_credentials_at_all_without_the_metadata_service() {
        let dir = data_home();
        let err = from_environment(&env(&[
            ("XDG_DATA_HOME", dir.to_str().unwrap()),
            (ENV_METADATA_DISABLED, "TRUE"),
        ]))
        .unwrap_err();
        assert!(matches!(err, Error::NoCredentials), "{err}");
    }

    #[test]
    fn the_session_is_found_under_the_home_directory_without_xdg_data_home() {
        let path = cli_session_path(&env(&[
            ("HOME", "/home/someone"),
            ("USERPROFILE", "/home/someone"),
        ]))
        .unwrap();
        assert_eq!(
            path,
            PathBuf::from("/home/someone/.local/share/origincloud/session.json")
        );
    }

    #[tokio::test]
    async fn the_cli_session_is_used_as_stored() {
        let dir = data_home();
        write_session(
            &dir,
            now() + chrono::Duration::hours(1),
            now() + chrono::Duration::hours(2),
        );
        let credentials =
            from_environment(&env(&[("XDG_DATA_HOME", dir.to_str().unwrap())])).unwrap();
        assert_eq!(credentials.token().await.unwrap(), "stored-access");
    }

    #[tokio::test]
    async fn a_cli_session_past_its_refresh_expiry_asks_to_log_in() {
        let dir = data_home();
        let path = write_session(
            &dir,
            now() - chrono::Duration::hours(1),
            now() - chrono::Duration::minutes(1),
        );
        let session = CliSession::load_from(path).unwrap().unwrap();
        let err = session.token().await.unwrap_err();
        assert!(err.to_string().contains("origin id login"), "{err}");
    }
}
