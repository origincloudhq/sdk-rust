//! Credentials: where the access token each call is sent with comes from.
//!
//! In a browser (`wasm32-unknown-unknown`) there are [`AccessToken`] and
//! the [`Credentials`] trait, for credentials of the page's own. The
//! providers that read the environment, files, a CI job's token or an
//! instance's metadata service, and `default_credentials`, which chains
//! them, are native only: a browser has none of these, and
//! [`ClientBuilder::build`](crate::ClientBuilder::build) without
//! credentials fails with [`Error::NoCredentials`] there. `ServiceAccount`
//! is there only with the feature `service-account-wasm`, for runtimes
//! that aren't a browser, and only from a credentials file's content
//! (`ServiceAccount::from_json`).

use std::fmt;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(any(not(target_arch = "wasm32"), feature = "service-account-wasm"))]
use std::sync::OnceLock;
#[cfg(any(not(target_arch = "wasm32"), feature = "service-account-wasm"))]
use std::time::{Duration, SystemTime};

#[cfg(not(target_arch = "wasm32"))]
use chrono::{DateTime, Utc};
#[cfg(any(not(target_arch = "wasm32"), feature = "service-account-wasm"))]
use serde::Deserialize;

use crate::Error;
use crate::runtime::MaybeSendFuture;
#[cfg(any(not(target_arch = "wasm32"), feature = "service-account-wasm"))]
use crate::runtime::{Mutex, now};
#[cfg(any(not(target_arch = "wasm32"), feature = "service-account-wasm"))]
use crate::transport::read_capped;

/// The identity provider's token endpoint, where the CLI's session is
/// renewed.
pub const TOKEN_URL: &str =
    "https://auth.origincloud.eu/realms/origincloud/protocol/openid-connect/token";

#[cfg(not(target_arch = "wasm32"))]
const ENV_ACCESS_TOKEN: &str = "ORIGINCLOUD_ACCESS_TOKEN";
#[cfg(not(target_arch = "wasm32"))]
const ENV_CREDENTIALS_FILE: &str = "ORIGINCLOUD_CREDENTIALS_FILE";
#[cfg(not(target_arch = "wasm32"))]
const ENV_METADATA_DISABLED: &str = "ORIGINCLOUD_METADATA_DISABLED";
#[cfg(not(target_arch = "wasm32"))]
const ENV_METADATA_ENDPOINT: &str = "ORIGINCLOUD_METADATA_ENDPOINT";
#[cfg(not(target_arch = "wasm32"))]
const ENV_OIDC_DISABLED: &str = "ORIGINCLOUD_OIDC_DISABLED";
#[cfg(not(target_arch = "wasm32"))]
const ENV_OIDC_TOKEN_FILE: &str = "ORIGINCLOUD_OIDC_TOKEN_FILE";
#[cfg(not(target_arch = "wasm32"))]
const ENV_OIDC_TOKEN: &str = "ORIGINCLOUD_OIDC_TOKEN";
#[cfg(not(target_arch = "wasm32"))]
const ENV_OIDC_AUDIENCE: &str = "ORIGINCLOUD_OIDC_AUDIENCE";
#[cfg(not(target_arch = "wasm32"))]
const ENV_STS_ENDPOINT: &str = "ORIGINCLOUD_STS_ENDPOINT";
/// What an HCP Terraform workspace with dynamic credentials sets in a run:
/// the workload identity token in `ENV_TFC_TOKEN`, or, for an audience
/// variable with a tag (`TFC_WORKLOAD_IDENTITY_AUDIENCE_<TAG>`), in
/// `ENV_TFC_TOKEN_<TAG>`. `ENV_TFC_TAG` names the tag to read.
#[cfg(not(target_arch = "wasm32"))]
const ENV_TFC_TOKEN: &str = "TFC_WORKLOAD_IDENTITY_TOKEN";
#[cfg(not(target_arch = "wasm32"))]
const ENV_TFC_TAG: &str = "ORIGINCLOUD_TFC_WORKLOAD_IDENTITY_TAG";
#[cfg(not(target_arch = "wasm32"))]
const ENV_GITHUB_REQUEST_URL: &str = "ACTIONS_ID_TOKEN_REQUEST_URL";
#[cfg(not(target_arch = "wasm32"))]
const ENV_GITHUB_REQUEST_TOKEN: &str = "ACTIONS_ID_TOKEN_REQUEST_TOKEN";

/// The security token service, where a CI job's OIDC token is exchanged
/// for an access token. A service account key's assertions are exchanged
/// where its credentials file says.
pub const STS_ENDPOINT: &str = "https://sts.origincloudapis.eu";

/// The audience a GitHub Actions job's OIDC token is requested for.
pub const OIDC_AUDIENCE: &str = "https://sts.origincloudapis.eu";

/// How long before its expiry an exchanged access token is replaced.
#[cfg(not(target_arch = "wasm32"))]
const FEDERATED_EXPIRY_MARGIN: Duration = Duration::from_secs(60);

/// The waits before an exchange the STS answered with `503` (it can't
/// check the token right now) is sent again.
#[cfg(not(target_arch = "wasm32"))]
const STS_RETRY_DELAYS: [Duration; 2] = [Duration::from_millis(200), Duration::from_millis(400)];

/// The `type` of a service account key's credentials file.
#[cfg(any(not(target_arch = "wasm32"), feature = "service-account-wasm"))]
const CREDENTIALS_FILE_TYPE: &str = "service_account_key";

/// The algorithm a service account key's assertions are signed with.
#[cfg(any(not(target_arch = "wasm32"), feature = "service-account-wasm"))]
const ASSERTION_ALGORITHM: &str = "ES256";

/// How long an assertion is valid: the STS refuses one valid for longer
/// than an hour, and each is sent at once.
#[cfg(any(not(target_arch = "wasm32"), feature = "service-account-wasm"))]
const ASSERTION_LIFETIME: Duration = Duration::from_secs(5 * 60);

/// How long before its expiry a service account's access token is
/// replaced.
#[cfg(any(not(target_arch = "wasm32"), feature = "service-account-wasm"))]
const SERVICE_ACCOUNT_EXPIRY_MARGIN: Duration = Duration::from_secs(60);

#[cfg(any(not(target_arch = "wasm32"), feature = "service-account-wasm"))]
const JWT_BEARER_GRANT: &str = "urn:ietf:params:oauth:grant-type:jwt-bearer";
#[cfg(not(target_arch = "wasm32"))]
const TOKEN_EXCHANGE_GRANT: &str = "urn:ietf:params:oauth:grant-type:token-exchange";
#[cfg(not(target_arch = "wasm32"))]
const JWT_TOKEN_TYPE: &str = "urn:ietf:params:oauth:token-type:jwt";

/// The instance metadata service, which software on an instance reaches
/// at this link-local address.
pub const METADATA_ENDPOINT: &str = "http://169.254.169.254";

/// How long a metadata-service session is asked for: the most it grants.
#[cfg(not(target_arch = "wasm32"))]
const METADATA_SESSION_TTL: Duration = Duration::from_secs(6 * 60 * 60);
#[cfg(not(target_arch = "wasm32"))]
const METADATA_SESSION_TTL_HEADER: &str = "x-origincloud-metadata-token-ttl-seconds";
#[cfg(not(target_arch = "wasm32"))]
const METADATA_SESSION_HEADER: &str = "x-origincloud-metadata-token";

/// How long reaching the metadata service may take, and opening a session
/// in all: off an instance, the chain's last step fails within them.
#[cfg(not(target_arch = "wasm32"))]
const METADATA_CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
#[cfg(not(target_arch = "wasm32"))]
const METADATA_SESSION_TIMEOUT: Duration = Duration::from_secs(2);

/// How long a credential request may take.
#[cfg(not(target_arch = "wasm32"))]
const METADATA_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// How long before its expiry the metadata service's credential, which
/// lives five minutes, is replaced.
#[cfg(not(target_arch = "wasm32"))]
const METADATA_EXPIRY_MARGIN: Duration = Duration::from_secs(60);

/// The waits before a request the metadata service answered with `503`
/// (it can't sign right now) is sent again.
#[cfg(not(target_arch = "wasm32"))]
const METADATA_RETRY_DELAYS: [Duration; 2] =
    [Duration::from_millis(100), Duration::from_millis(200)];

/// The OAuth client the CLI logs in as, which its refresh token belongs to.
#[cfg(not(target_arch = "wasm32"))]
const CLI_CLIENT_ID: &str = "origin-cli";

/// How long before its expiry a token is replaced, so that it doesn't
/// expire on the way.
#[cfg(not(target_arch = "wasm32"))]
const EXPIRY_MARGIN: Duration = Duration::from_secs(30);

/// The most of a token response that is read.
#[cfg(any(not(target_arch = "wasm32"), feature = "service-account-wasm"))]
const MAX_TOKEN_RESPONSE_BYTES: usize = 1 << 20;

/// How long a token request may take.
#[cfg(any(not(target_arch = "wasm32"), feature = "service-account-wasm"))]
const TOKEN_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// What [`Credentials::token`] returns: `Send` natively; not in a
/// browser, where a fetch is a JavaScript promise.
pub type TokenFuture<'a> = MaybeSendFuture<'a, Result<String, Error>>;

/// Credentials supply the access token each call is sent with. `Send +
/// Sync` on every target, so a client can live in a UI framework's
/// reactive state (Leptos 0.8 keeps memo values `Send + Sync` even in a
/// browser); only the future [`token`](Self::token) returns is `!Send` there.
pub trait Credentials: Send + Sync + fmt::Debug {
    /// A valid access token.
    fn token(&self) -> TokenFuture<'_>;
}

/// The first credentials found, in this order:
///
/// 1. `ORIGINCLOUD_ACCESS_TOKEN`: an access token, sent as it is.
/// 2. `ORIGINCLOUD_CREDENTIALS_FILE`: the path of a service account key's
///    credentials file ([`ServiceAccount`]), whose assertions are exchanged
///    for access tokens as they expire. A file that can't be read is an
///    error, not passed over.
/// 3. A CI job's OIDC token, exchanged at the security token service
///    ([`Federated`]): from the file `ORIGINCLOUD_OIDC_TOKEN_FILE` names,
///    or else from `ORIGINCLOUD_OIDC_TOKEN`, which holds the token itself
///    (as GitLab CI's `id_tokens` set it), or else from an HCP Terraform
///    run's workload identity token, `TFC_WORKLOAD_IDENTITY_TOKEN`, or
///    `TFC_WORKLOAD_IDENTITY_TOKEN_<TAG>` when
///    `ORIGINCLOUD_TFC_WORKLOAD_IDENTITY_TAG` names the tag of a tagged
///    audience, or else from GitHub Actions, in a job granted `permissions: id-token: write`. Once found, its errors
///    are returned, not passed over. `ORIGINCLOUD_OIDC_DISABLED=true`
///    leaves this step out; `ORIGINCLOUD_STS_ENDPOINT` asks another address
///    than [`STS_ENDPOINT`], and `ORIGINCLOUD_OIDC_AUDIENCE` requests
///    GitHub's token for another audience than [`OIDC_AUDIENCE`].
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
#[cfg(not(target_arch = "wasm32"))]
pub fn default_credentials() -> Result<Arc<dyn Credentials>, Error> {
    from_environment(&|name| std::env::var(name).ok())
}

/// [`default_credentials`] with the environment read through `env`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn from_environment(
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Arc<dyn Credentials>, Error> {
    let var = |name: &str| env(name).filter(|v| !v.is_empty());
    if let Some(token) = var(ENV_ACCESS_TOKEN) {
        return Ok(Arc::new(AccessToken::new(token)));
    }
    if let Some(path) = var(ENV_CREDENTIALS_FILE) {
        return Ok(Arc::new(ServiceAccount::from_file(path)?));
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

/// In a browser there is no environment, no file system and no metadata
/// service, and a private key in a page is anybody's: the page supplies its own
/// [`Credentials`] (an [`AccessToken`], or one that refreshes its session).
#[cfg(target_arch = "wasm32")]
pub(crate) fn from_environment(
    _env: &dyn Fn(&str) -> Option<String>,
) -> Result<Arc<dyn Credentials>, Error> {
    Err(Error::NoCredentials)
}

/// The federated step of the chain, when the environment holds a CI job's
/// OIDC token: a token file, which can be rotated in place, before a token
/// given as it is, before an HCP Terraform run's workload identity token,
/// before GitHub Actions.
#[cfg(not(target_arch = "wasm32"))]
fn federated_from(var: &dyn Fn(&str) -> Option<String>) -> Option<Federated> {
    if var(ENV_OIDC_DISABLED).is_some_and(|v| v.eq_ignore_ascii_case("true")) {
        return None;
    }
    let mut federated = if let Some(path) = var(ENV_OIDC_TOKEN_FILE) {
        Federated::from_file(path)
    } else if let Some(token) = var(ENV_OIDC_TOKEN) {
        Federated::with_source(JobToken::Given {
            token,
            variable: Some(ENV_OIDC_TOKEN.to_owned()),
        })
    } else if let Some(variable) = tfc_token_variable(var) {
        Federated::with_source(JobToken::Given {
            token: var(&variable).unwrap_or_default(),
            variable: Some(variable),
        })
    } else if let (Some(url), Some(token)) =
        (var(ENV_GITHUB_REQUEST_URL), var(ENV_GITHUB_REQUEST_TOKEN))
    {
        Federated::github_actions(url, token)
    } else {
        return None;
    };
    if let Some(endpoint) = var(ENV_STS_ENDPOINT) {
        federated = federated.with_endpoint(endpoint);
    }
    if let Some(audience) = var(ENV_OIDC_AUDIENCE) {
        federated = federated.with_audience(audience);
    }
    Some(federated)
}

/// The variable an HCP Terraform run's workload identity token is read
/// from: the tagged one when `ORIGINCLOUD_TFC_WORKLOAD_IDENTITY_TAG` names a
/// tag, even if that variable is unset, so that the error names it; the
/// untagged one otherwise, when it is set. `None` when the run has none.
#[cfg(not(target_arch = "wasm32"))]
fn tfc_token_variable(var: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    match var(ENV_TFC_TAG).map(|tag| tag.trim().to_owned()) {
        Some(tag) if !tag.is_empty() => Some(format!("{ENV_TFC_TOKEN}_{tag}")),
        _ => var(ENV_TFC_TOKEN).map(|_| ENV_TFC_TOKEN.to_owned()),
    }
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

/// A service account key's credentials file, as IAM returns it once when
/// the key is created: the key's and the account's full names, the private
/// key, and the security token service its assertions are exchanged at.
#[cfg(any(not(target_arch = "wasm32"), feature = "service-account-wasm"))]
#[derive(Deserialize)]
struct CredentialsFile {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    service_account: String,
    #[serde(default)]
    algorithm: String,
    #[serde(default)]
    private_key: String,
    #[serde(default)]
    sts_endpoint: String,
}

/// An `EXTERNAL` service account, authenticated with one of its keys: an
/// assertion signed with the key from its credentials file (RFC 7523,
/// `ES256`) is exchanged at the security token service for the account's
/// access token, which is reused until a minute before it expires. Callers
/// that need a token while it is being renewed wait for that renewal and
/// share its token.
///
/// On `wasm32-unknown-unknown` it exists only with the feature
/// `service-account-wasm`, for runtimes that aren't a browser: a web
/// page, and a Web Worker too, is shipped to whoever loads it, private key
/// and all.
#[cfg(any(not(target_arch = "wasm32"), feature = "service-account-wasm"))]
pub struct ServiceAccount {
    key: String,
    service_account: String,
    signing_key: p256::ecdsa::SigningKey,
    sts_endpoint: String,
    http: Option<reqwest::Client>,
    cache: Mutex<Cached>,
}

#[cfg(any(not(target_arch = "wasm32"), feature = "service-account-wasm"))]
impl ServiceAccount {
    /// The service account whose key's credentials file is at `path`.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn from_file(path: impl AsRef<std::path::Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let data = std::fs::read(path).map_err(|e| {
            Error::Credentials(format!(
                "reading the credentials file {}: {e}",
                path.display()
            ))
        })?;
        Self::parse(&data).map_err(|reason| {
            Error::Credentials(format!(
                "reading the credentials file {}: {reason}",
                path.display()
            ))
        })
    }

    /// The service account whose key's credentials file is `json`, such as
    /// the `credentialsFile` a key's creation answers with.
    pub fn from_json(json: impl AsRef<[u8]>) -> Result<Self, Error> {
        Self::parse(json.as_ref())
            .map_err(|reason| Error::Credentials(format!("reading the credentials file: {reason}")))
    }

    fn parse(json: &[u8]) -> Result<Self, String> {
        use p256::pkcs8::DecodePrivateKey as _;

        // serde_json's errors name a position, never the file's content.
        let file: CredentialsFile = serde_json::from_slice(json).map_err(|e| e.to_string())?;
        if file.kind != CREDENTIALS_FILE_TYPE {
            return Err(format!(
                "its type is {:?}, not {CREDENTIALS_FILE_TYPE:?}",
                file.kind
            ));
        }
        if file.algorithm != ASSERTION_ALGORITHM {
            return Err(format!(
                "its algorithm is {:?}; this SDK signs with {ASSERTION_ALGORITHM}",
                file.algorithm
            ));
        }
        for (field, value) in [
            ("name", &file.name),
            ("service_account", &file.service_account),
            ("private_key", &file.private_key),
            ("sts_endpoint", &file.sts_endpoint),
        ] {
            if value.is_empty() {
                return Err(format!("it has no {field}"));
            }
        }
        let signing_key = p256::ecdsa::SigningKey::from_pkcs8_pem(&file.private_key)
            .map_err(|_| "its private_key is not a PKCS #8 P-256 key in PEM".to_owned())?;
        Ok(Self {
            key: file.name,
            service_account: file.service_account,
            signing_key,
            sts_endpoint: file.sts_endpoint,
            http: None,
            cache: Mutex::default(),
        })
    }

    /// The service account's full name, as the file names it.
    pub fn service_account(&self) -> &str {
        &self.service_account
    }

    /// The key's full name, as the file names it.
    pub fn key(&self) -> &str {
        &self.key
    }

    /// Sends the token requests with `http`, instead of the client all
    /// credentials share. Each request is bounded by 30 seconds either way.
    pub fn with_http_client(mut self, http: reqwest::Client) -> Self {
        self.http = Some(http);
        self
    }

    /// A fresh assertion: a JWT signed with the key, for the STS the file
    /// names, valid for five minutes, with an ID never used before.
    fn assertion(&self) -> String {
        use base64::Engine as _;
        use base64::engine::general_purpose::URL_SAFE_NO_PAD;
        use p256::ecdsa::signature::Signer as _;

        let issued = now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let header = serde_json::json!({
            "alg": ASSERTION_ALGORITHM,
            "typ": "JWT",
            "kid": self.key,
        });
        let claims = serde_json::json!({
            "iss": self.service_account,
            "sub": self.service_account,
            "aud": self.sts_endpoint,
            "iat": issued,
            "exp": issued + ASSERTION_LIFETIME.as_secs(),
            "jti": uuid::Uuid::new_v4().to_string(),
        });
        let signed = format!(
            "{}.{}",
            URL_SAFE_NO_PAD.encode(header.to_string()),
            URL_SAFE_NO_PAD.encode(claims.to_string())
        );
        let signature: p256::ecdsa::Signature = self.signing_key.sign(signed.as_bytes());
        format!("{signed}.{}", URL_SAFE_NO_PAD.encode(signature.to_bytes()))
    }
}

#[cfg(any(not(target_arch = "wasm32"), feature = "service-account-wasm"))]
impl fmt::Debug for ServiceAccount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServiceAccount")
            .field("service_account", &self.service_account)
            .field("key", &self.key)
            .field("sts_endpoint", &self.sts_endpoint)
            .finish_non_exhaustive()
    }
}

#[cfg(any(not(target_arch = "wasm32"), feature = "service-account-wasm"))]
impl Credentials for ServiceAccount {
    fn token(&self) -> TokenFuture<'_> {
        Box::pin(async move {
            let mut cache = self.cache.lock().await;
            if let Some(token) = cache.valid_for(SERVICE_ACCOUNT_EXPIRY_MARGIN) {
                return Ok(token);
            }
            let assertion = self.assertion();
            let granted = request_token(
                self.http.as_ref(),
                &format!("{}/v1/token", self.sts_endpoint.trim_end_matches('/')),
                &[("grant_type", JWT_BEARER_GRANT), ("assertion", &assertion)],
            )
            .await
            .map_err(|e| match e {
                Error::Credentials(message) => Error::Credentials(format!(
                    "exchanging an assertion of the service account key {}: {message}",
                    self.key
                )),
                other => other,
            })?;
            *cache = Cached {
                token: granted.access_token.clone(),
                expiry: Some(granted.expiry()),
            };
            Ok(granted.access_token)
        })
    }
}

/// The session the `origin` CLI stored at its last login, renewed from its
/// refresh token when its access token has expired. The renewed session
/// stays in memory: the CLI's file is never written.
#[cfg(not(target_arch = "wasm32"))]
pub struct CliSession {
    token_url: String,
    http: Option<reqwest::Client>,
    state: Mutex<SessionState>,
}

#[cfg(not(target_arch = "wasm32"))]
struct SessionState {
    cached: Cached,
    refresh_token: String,
    refresh_expiry: Option<SystemTime>,
}

/// The file the CLI stores its session in.
#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
impl fmt::Debug for CliSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CliSession")
            .field("token_url", &self.token_url)
            .finish_non_exhaustive()
    }
}

#[cfg(not(target_arch = "wasm32"))]
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
#[cfg(not(target_arch = "wasm32"))]
pub struct InstanceMetadata {
    endpoint: String,
    http: Option<reqwest::Client>,
    state: Mutex<MetadataState>,
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Default)]
struct MetadataState {
    session: Cached,
    cached: Cached,
}

/// The metadata service's credential.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Deserialize)]
struct Issued {
    access_token: String,
    expires_at: DateTime<Utc>,
}

#[cfg(not(target_arch = "wasm32"))]
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
#[cfg(not(target_arch = "wasm32"))]
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

#[cfg(not(target_arch = "wasm32"))]
fn refused_through_a_proxy() -> Error {
    Error::Credentials(
        "the metadata service refused a request that came through a proxy".to_owned(),
    )
}

#[cfg(not(target_arch = "wasm32"))]
impl Default for InstanceMetadata {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl fmt::Debug for InstanceMetadata {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InstanceMetadata")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

#[cfg(not(target_arch = "wasm32"))]
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
/// expires; then a fresh job token is fetched and exchanged again, or a
/// token given as it is ([`Federated::from_token`]) exchanged again. An
/// exchange the STS can't check right now (`503`) is sent again twice.
#[cfg(not(target_arch = "wasm32"))]
pub struct Federated {
    source: JobToken,
    endpoint: String,
    audience: String,
    http: Option<reqwest::Client>,
    cache: Mutex<Cached>,
}

/// Where the job's token comes from.
#[cfg(not(target_arch = "wasm32"))]
enum JobToken {
    File(PathBuf),
    /// A token given as it is, from the environment variable `variable`
    /// names, if any.
    Given {
        token: String,
        variable: Option<String>,
    },
    GitHubActions {
        request_url: String,
        request_token: String,
    },
}

/// The claims of a job token that are read before it is exchanged.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Deserialize)]
struct JobTokenClaims {
    exp: Option<f64>,
}

/// GitHub's answer to a job token request.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Deserialize)]
struct GitHubToken {
    value: String,
}

#[cfg(not(target_arch = "wasm32"))]
impl Federated {
    /// The token in the file at `path`, read again at every exchange, so
    /// that a rotated token is picked up: any CI provider's, or a
    /// Kubernetes projected service account token.
    pub fn from_file(path: impl Into<PathBuf>) -> Self {
        Self::with_source(JobToken::File(path.into()))
    }

    /// The token `token`, exchanged as it is: any CI provider's, such as the
    /// one GitLab CI sets in the variable its `id_tokens` names. It can't
    /// be renewed: each renewal of the access token exchanges it again,
    /// until it expires itself, and from then on fails without asking the
    /// STS.
    pub fn from_token(token: impl Into<String>) -> Self {
        Self::with_source(JobToken::Given {
            token: token.into(),
            variable: None,
        })
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
    /// A token file's or a given token's audience is whatever its issuer
    /// put in it.
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
            JobToken::Given { token, variable } => {
                let token = token.trim();
                if token.is_empty() {
                    return Err(Error::Credentials(format!(
                        "the CI job's OIDC token{} is empty{}",
                        in_variable(variable.as_deref()),
                        if variable.is_some() {
                            " or not set"
                        } else {
                            ""
                        }
                    )));
                }
                if let Some(expiry) = job_token_expiry(token)
                    && expiry <= SystemTime::now()
                {
                    let expiry = DateTime::<Utc>::from(expiry)
                        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
                    return Err(Error::Credentials(format!(
                        "the CI job's OIDC token{} expired at {expiry}; a token given as it is \
                         can't be renewed: ask the CI provider for a longer-lived one, or use \
                         {ENV_OIDC_TOKEN_FILE}",
                        in_variable(variable.as_deref())
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

#[cfg(not(target_arch = "wasm32"))]
impl fmt::Debug for Federated {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = f.debug_struct("Federated");
        match &self.source {
            JobToken::File(path) => debug.field("token_file", path),
            JobToken::Given { .. } => debug.field("source", &"token"),
            JobToken::GitHubActions { .. } => debug.field("source", &"GitHub Actions"),
        };
        debug
            .field("endpoint", &self.endpoint)
            .field("audience", &self.audience)
            .finish_non_exhaustive()
    }
}

#[cfg(not(target_arch = "wasm32"))]
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

/// ` in VARIABLE`, naming where a given job token came from, if anywhere.
#[cfg(not(target_arch = "wasm32"))]
fn in_variable(variable: Option<&str>) -> String {
    variable.map_or_else(String::new, |name| format!(" in {name}"))
}

/// When a job token (a JWT) expires, as its unverified `exp` claim says:
/// `None` when it has none, or isn't a JWT whose claims can be read, which
/// is left to the STS to judge.
#[cfg(not(target_arch = "wasm32"))]
fn job_token_expiry(token: &str) -> Option<SystemTime> {
    use base64::Engine as _;
    use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};

    const BASE64URL: GeneralPurpose = GeneralPurpose::new(
        &base64::alphabet::URL_SAFE,
        GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
    );
    let payload = token.split('.').nth(1)?;
    let claims: JobTokenClaims = serde_json::from_slice(&BASE64URL.decode(payload).ok()?).ok()?;
    let exp = claims.exp?;
    if !exp.is_finite() || exp < 0.0 {
        return None;
    }
    SystemTime::UNIX_EPOCH.checked_add(Duration::try_from_secs_f64(exp).ok()?)
}

/// Where the CLI stores its session: under `XDG_DATA_HOME`, or
/// `~/.local/share`, on every platform.
#[cfg(not(target_arch = "wasm32"))]
fn cli_session_path(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let base = match env("XDG_DATA_HOME").filter(|v| !v.is_empty()) {
        Some(base) => PathBuf::from(base),
        None => home_dir(env)?.join(".local").join("share"),
    };
    Some(base.join("origincloud").join("session.json"))
}

#[cfg(not(target_arch = "wasm32"))]
fn home_dir(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    env(name).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// One access token, replaced once it is about to expire.
#[cfg(any(not(target_arch = "wasm32"), feature = "service-account-wasm"))]
#[derive(Default)]
struct Cached {
    token: String,
    expiry: Option<SystemTime>,
}

#[cfg(any(not(target_arch = "wasm32"), feature = "service-account-wasm"))]
impl Cached {
    #[cfg(not(target_arch = "wasm32"))]
    fn valid(&self) -> Option<String> {
        self.valid_for(EXPIRY_MARGIN)
    }

    /// The token, unless it expires within `margin`.
    fn valid_for(&self, margin: Duration) -> Option<String> {
        let expiry = self.expiry?;
        (!self.token.is_empty() && now() + margin < expiry).then(|| self.token.clone())
    }
}

/// A token endpoint's answer.
#[cfg(any(not(target_arch = "wasm32"), feature = "service-account-wasm"))]
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

#[cfg(any(not(target_arch = "wasm32"), feature = "service-account-wasm"))]
impl Granted {
    fn expiry(&self) -> SystemTime {
        now() + seconds(self.expires_in)
    }
}

#[cfg(any(not(target_arch = "wasm32"), feature = "service-account-wasm"))]
fn seconds(n: i64) -> Duration {
    Duration::from_secs(u64::try_from(n).unwrap_or_default())
}

/// The HTTP client token requests are sent with unless the credentials
/// name one: one for the process, created by the first request.
#[cfg(any(not(target_arch = "wasm32"), feature = "service-account-wasm"))]
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

#[cfg(any(not(target_arch = "wasm32"), feature = "service-account-wasm"))]
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
#[cfg(any(not(target_arch = "wasm32"), feature = "service-account-wasm"))]
async fn read_bounded(
    response: reqwest::Response,
) -> Result<(reqwest::StatusCode, Vec<u8>), reqwest::Error> {
    let status = response.status();
    let (body, _truncated) = read_capped(response, MAX_TOKEN_RESPONSE_BYTES).await?;
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

    /// A P-256 key for the tests alone, in PKCS #8 PEM.
    const TEST_KEY: &str = "-----BEGIN PRIVATE KEY-----
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgyHvHtkzp0fjy/hHK
/yt/j0yihbwhkfpFzHsrSjh6OqOhRANCAARxfaIx32RdN3JO0GPH8ght/CFjaoE3
ao6XiaIxhBdJ6jPPWX/9086Bti4LIfP+qzNTxPn85k7f+yGc6aQoBkr1
-----END PRIVATE KEY-----
";

    fn credentials_file(fields: serde_json::Value) -> serde_json::Value {
        let mut file = serde_json::json!({
            "type": "service_account_key",
            "name": "//iam.origincloudapis.eu/organizations/o/serviceAccounts/s/keys/k",
            "service_account": "//iam.origincloudapis.eu/organizations/o/serviceAccounts/s",
            "algorithm": "ES256",
            "private_key": TEST_KEY,
            "sts_endpoint": "https://sts.origincloudapis.eu",
        });
        for (k, v) in fields.as_object().unwrap() {
            file[k] = v.clone();
        }
        file
    }

    /// A credentials file in a directory of its own, and its path.
    fn write_credentials_file(file: &serde_json::Value) -> String {
        let path = data_home().join("credentials.json");
        std::fs::write(&path, file.to_string()).unwrap();
        path.to_str().unwrap().to_owned()
    }

    #[tokio::test]
    async fn an_access_token_in_the_environment_comes_first() {
        let file = write_credentials_file(&credentials_file(serde_json::json!({})));
        let credentials = from_environment(&env(&[
            (ENV_ACCESS_TOKEN, "from-env"),
            (ENV_CREDENTIALS_FILE, &file),
        ]))
        .unwrap();
        assert_eq!(credentials.token().await.unwrap(), "from-env");
    }

    #[test]
    fn a_credentials_file_comes_before_the_cli_session() {
        let dir = data_home();
        let hour = chrono::Duration::hours(1);
        write_session(&dir, now() + hour, now() + hour);
        let file = write_credentials_file(&credentials_file(serde_json::json!({})));
        let credentials = from_environment(&env(&[
            ("XDG_DATA_HOME", dir.to_str().unwrap()),
            (ENV_CREDENTIALS_FILE, &file),
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
    fn a_credentials_file_comes_before_a_ci_job_s_token() {
        let file = write_credentials_file(&credentials_file(serde_json::json!({})));
        let credentials = from_environment(&env(&with(
            &GITHUB,
            &[(ENV_OIDC_TOKEN, "job-token"), (ENV_CREDENTIALS_FILE, &file)],
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
    fn a_token_in_the_environment_is_exchanged() {
        let credentials = from_environment(&env(&[(ENV_OIDC_TOKEN, "gitlab-job-token")])).unwrap();
        let debug = format!("{credentials:?}");
        assert!(
            debug.starts_with(r#"Federated { source: "token""#),
            "{debug}"
        );
        assert!(!debug.contains("gitlab-job-token"), "{debug}");
    }

    #[test]
    fn a_token_file_comes_before_a_token_in_the_environment() {
        let credentials = from_environment(&env(&[
            (ENV_OIDC_TOKEN, "gitlab-job-token"),
            (ENV_OIDC_TOKEN_FILE, "/var/run/secrets/token"),
        ]))
        .unwrap();
        let debug = format!("{credentials:?}");
        assert!(
            debug.starts_with(r#"Federated { token_file: "/var/run/secrets/token""#),
            "{debug}"
        );
    }

    #[test]
    fn a_token_in_the_environment_comes_before_github_actions() {
        let credentials = from_environment(&env(&with(
            &GITHUB,
            &[(ENV_OIDC_TOKEN, "gitlab-job-token")],
        )))
        .unwrap();
        let debug = format!("{credentials:?}");
        assert!(
            debug.starts_with(r#"Federated { source: "token""#),
            "{debug}"
        );
    }

    #[test]
    fn a_token_in_the_environment_is_left_out_with_the_federated_step() {
        let dir = data_home();
        let credentials = from_environment(&env(&[
            (ENV_OIDC_TOKEN, "gitlab-job-token"),
            (ENV_OIDC_DISABLED, "true"),
            ("XDG_DATA_HOME", dir.to_str().unwrap()),
        ]))
        .unwrap();
        assert!(
            format!("{credentials:?}").starts_with("InstanceMetadata"),
            "{credentials:?}"
        );
    }

    /// An unsigned JWT with the claims `claims`.
    fn jwt(claims: &str) -> String {
        use base64::Engine as _;
        let encode = |part: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(part);
        format!(
            "{}.{}.",
            encode(r#"{"alg":"none","typ":"JWT"}"#),
            encode(claims)
        )
    }

    #[tokio::test]
    async fn an_expired_token_in_the_environment_names_the_variable() {
        let token = jwt(r#"{"sub":"project_path:group/project","exp":1577836800}"#);
        let credentials = from_environment(&env(&[
            (ENV_OIDC_TOKEN, &token),
            (ENV_STS_ENDPOINT, "http://127.0.0.1:9"),
        ]))
        .unwrap();
        let message = credentials.token().await.unwrap_err().to_string();
        assert!(
            message.contains(
                "the CI job's OIDC token in ORIGINCLOUD_OIDC_TOKEN expired at 2020-01-01T00:00:00Z"
            ),
            "{message}"
        );
        assert!(message.contains("ORIGINCLOUD_OIDC_TOKEN_FILE"), "{message}");
        assert!(!message.contains(&token), "{message}");
    }

    /// The error of exchanging the token the environment `vars` holds: an
    /// expired one, so that nothing is sent, which names the variable it
    /// was read from.
    async fn exchange_error(vars: &[(&str, &str)]) -> String {
        let credentials = from_environment(&env(&with(
            vars,
            &[(ENV_STS_ENDPOINT, "http://127.0.0.1:9")],
        )))
        .unwrap();
        credentials.token().await.unwrap_err().to_string()
    }

    fn expired() -> String {
        jwt(r#"{"sub":"organization:org:project:p:workspace:w:run_phase:plan","exp":1577836800}"#)
    }

    #[test]
    fn an_hcp_terraform_token_is_exchanged() {
        let credentials = from_environment(&env(&[(ENV_TFC_TOKEN, "tfc-token")])).unwrap();
        let debug = format!("{credentials:?}");
        assert!(
            debug.starts_with(r#"Federated { source: "token""#),
            "{debug}"
        );
        assert!(!debug.contains("tfc-token"), "{debug}");
    }

    #[tokio::test]
    async fn an_expired_hcp_terraform_token_names_its_variable() {
        let token = expired();
        let message = exchange_error(&[(ENV_TFC_TOKEN, &token)]).await;
        assert!(
            message.contains(
                "the CI job's OIDC token in TFC_WORKLOAD_IDENTITY_TOKEN expired at 2020-01-01T00:00:00Z"
            ),
            "{message}"
        );
        assert!(!message.contains(&token), "{message}");
    }

    #[tokio::test]
    async fn a_tagged_hcp_terraform_token_is_read_by_its_tag_alone() {
        let token = expired();
        let message = exchange_error(&[
            (ENV_TFC_TAG, " ORIGIN "),
            (ENV_TFC_TOKEN, "untagged-token"),
            ("TFC_WORKLOAD_IDENTITY_TOKEN_ORIGIN", &token),
            ("TFC_WORKLOAD_IDENTITY_TOKEN_OTHER", "other-token"),
        ])
        .await;
        assert!(
            message.contains("OIDC token in TFC_WORKLOAD_IDENTITY_TOKEN_ORIGIN expired"),
            "{message}"
        );
    }

    #[tokio::test]
    async fn a_tag_without_its_token_fails_naming_the_variable() {
        for vars in [
            vec![(ENV_TFC_TAG, "ORIGIN"), (ENV_TFC_TOKEN, "untagged-token")],
            vec![
                (ENV_TFC_TAG, "ORIGIN"),
                (ENV_TFC_TOKEN, "untagged-token"),
                ("TFC_WORKLOAD_IDENTITY_TOKEN_ORIGIN", ""),
            ],
        ] {
            let message = exchange_error(&vars).await;
            assert!(
                message.contains("in TFC_WORKLOAD_IDENTITY_TOKEN_ORIGIN is empty or not set"),
                "{message}"
            );
            assert!(!message.contains("untagged-token"), "{message}");
        }
    }

    #[test]
    fn a_tagged_hcp_terraform_token_without_its_tag_is_left_out() {
        let dir = data_home();
        let credentials = from_environment(&env(&[
            ("TFC_WORKLOAD_IDENTITY_TOKEN_ORIGIN", "tagged-token"),
            ("XDG_DATA_HOME", dir.to_str().unwrap()),
        ]))
        .unwrap();
        assert!(
            format!("{credentials:?}").starts_with("InstanceMetadata"),
            "{credentials:?}"
        );
    }

    #[test]
    fn an_hcp_terraform_token_is_left_out_with_the_federated_step() {
        let dir = data_home();
        let credentials = from_environment(&env(&[
            (ENV_TFC_TOKEN, "tfc-token"),
            (ENV_TFC_TAG, "ORIGIN"),
            (ENV_OIDC_DISABLED, "true"),
            ("XDG_DATA_HOME", dir.to_str().unwrap()),
        ]))
        .unwrap();
        assert!(
            format!("{credentials:?}").starts_with("InstanceMetadata"),
            "{credentials:?}"
        );
    }

    #[test]
    fn an_hcp_terraform_token_comes_before_github_actions() {
        let credentials =
            from_environment(&env(&with(&GITHUB, &[(ENV_TFC_TOKEN, "tfc-token")]))).unwrap();
        let debug = format!("{credentials:?}");
        assert!(
            debug.starts_with(r#"Federated { source: "token""#),
            "{debug}"
        );
    }

    #[test]
    fn an_hcp_terraform_token_comes_after_a_token_file() {
        let credentials = from_environment(&env(&[
            (ENV_TFC_TOKEN, "tfc-token"),
            (ENV_OIDC_TOKEN_FILE, "/var/run/secrets/token"),
        ]))
        .unwrap();
        let debug = format!("{credentials:?}");
        assert!(
            debug.starts_with(r#"Federated { token_file: "/var/run/secrets/token""#),
            "{debug}"
        );
    }

    #[tokio::test]
    async fn an_hcp_terraform_token_comes_after_a_token_in_the_environment() {
        let token = expired();
        let message =
            exchange_error(&[(ENV_OIDC_TOKEN, &token), (ENV_TFC_TOKEN, "tfc-token")]).await;
        assert!(
            message.contains("in ORIGINCLOUD_OIDC_TOKEN expired"),
            "{message}"
        );
    }

    #[test]
    fn a_job_token_s_expiry_is_read_from_its_claims() {
        let at = |secs| Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs));
        assert_eq!(
            job_token_expiry(&jwt(r#"{"exp":1577836800}"#)),
            at(1577836800)
        );
        assert_eq!(
            job_token_expiry(&jwt(r#"{"exp":1577836800.5}"#)),
            at(1577836800).map(|t| t + Duration::from_millis(500))
        );
        // Padded base64url, as some issuers write it.
        let padded = {
            use base64::Engine as _;
            let claims = base64::engine::general_purpose::URL_SAFE.encode(r#"{"exp":12}"#);
            assert!(claims.ends_with("=="), "{claims}");
            format!("e30.{claims}.")
        };
        assert_eq!(job_token_expiry(&padded), at(12));
        for unreadable in [
            jwt(r#"{"sub":"no expiry"}"#),
            jwt(r#"{"exp":"tomorrow"}"#),
            jwt(r#"{"exp":-1}"#),
            jwt("not json"),
            "not-a-jwt".to_owned(),
            "a.%%%.c".to_owned(),
        ] {
            assert_eq!(job_token_expiry(&unreadable), None, "{unreadable}");
        }
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
    fn a_credentials_file_that_cant_be_read_is_an_error_not_passed_over() {
        let missing = data_home().join("missing.json");
        let err = from_environment(&env(&with(
            &GITHUB,
            &[(ENV_CREDENTIALS_FILE, missing.to_str().unwrap())],
        )))
        .unwrap_err();
        assert!(
            err.to_string().contains(&format!(
                "reading the credentials file {}",
                missing.display()
            )),
            "{err}"
        );
    }

    #[test]
    fn a_credentials_file_is_checked_when_it_is_read() {
        let refused = |fields: serde_json::Value| {
            ServiceAccount::from_json(credentials_file(fields).to_string())
                .unwrap_err()
                .to_string()
        };
        let err = refused(serde_json::json!({"type": "authorized_user"}));
        assert!(err.contains(r#"its type is "authorized_user""#), "{err}");
        let err = refused(serde_json::json!({"algorithm": "RS256"}));
        assert!(err.contains(r#"its algorithm is "RS256""#), "{err}");
        let err = refused(serde_json::json!({"sts_endpoint": ""}));
        assert!(err.contains("it has no sts_endpoint"), "{err}");
        let err = refused(
            serde_json::json!({"private_key": "-----BEGIN PRIVATE KEY-----\nAAAA\n-----END PRIVATE KEY-----\n"}),
        );
        assert!(err.contains("not a PKCS #8 P-256 key"), "{err}");
        let err = ServiceAccount::from_json("not json")
            .unwrap_err()
            .to_string();
        assert!(
            err.starts_with("origincloud: reading the credentials file: "),
            "{err}"
        );
    }

    #[test]
    fn a_service_account_s_debug_names_the_key_but_shows_no_private_key() {
        let account =
            ServiceAccount::from_json(credentials_file(serde_json::json!({})).to_string()).unwrap();
        let debug = format!("{account:?}");
        assert!(
            debug.contains(
                r#"key: "//iam.origincloudapis.eu/organizations/o/serviceAccounts/s/keys/k""#
            ),
            "{debug}"
        );
        assert!(
            !debug.contains("PRIVATE KEY") && !debug.contains("MIGHAgEA"),
            "{debug}"
        );
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
