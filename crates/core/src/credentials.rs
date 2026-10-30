//! Credentials: where the access token each call is sent with comes from.

use std::fmt;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
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

/// The OAuth client the CLI logs in as, which its refresh token belongs to.
const CLI_CLIENT_ID: &str = "origin-cli";

/// How long before its expiry a token is replaced, so that it doesn't
/// expire on the way.
const EXPIRY_MARGIN: Duration = Duration::from_secs(30);

/// The most of a token response that is read.
const MAX_TOKEN_RESPONSE_BYTES: usize = 1 << 20;

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
/// 3. The session the `origin` CLI stored at its last login
///    (`origin id login`).
///
/// It fails with [`Error::NoCredentials`] when there are none.
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
    let path = cli_session_path(env).ok_or(Error::NoCredentials)?;
    match CliSession::load_from(path)? {
        Some(session) => Ok(Arc::new(session)),
        None => Err(Error::NoCredentials),
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

    /// Sends the token requests with `http`.
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

    /// Sends the renewals with `http`.
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
        let expiry = self.expiry?;
        (!self.token.is_empty() && SystemTime::now() + EXPIRY_MARGIN < expiry)
            .then(|| self.token.clone())
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

async fn request_token(
    http: Option<&reqwest::Client>,
    url: &str,
    form: &[(&str, &str)],
) -> Result<Granted, Error> {
    let failed = |e: &dyn fmt::Display| Error::Credentials(format!("requesting a token: {e}"));
    let default_client;
    let http = match http {
        Some(http) => http,
        None => {
            default_client = reqwest::Client::builder().build().map_err(|e| failed(&e))?;
            &default_client
        }
    };
    let mut response = http
        .post(url)
        .header(reqwest::header::ACCEPT, "application/json")
        .form(form)
        .send()
        .await
        .map_err(|e| failed(&e))?;
    let status = response.status();
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| failed(&e))? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_TOKEN_RESPONSE_BYTES {
            body.truncate(MAX_TOKEN_RESPONSE_BYTES);
            break;
        }
    }
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

    #[test]
    fn half_a_service_account_is_refused() {
        let err = from_environment(&env(&[(ENV_CLIENT_ID, "id")])).unwrap_err();
        assert!(err.to_string().contains("must be set together"), "{err}");
        let err = from_environment(&env(&[(ENV_CLIENT_SECRET, "secret")])).unwrap_err();
        assert!(err.to_string().contains("must be set together"), "{err}");
    }

    #[test]
    fn no_credentials_at_all() {
        let dir = data_home();
        let err = from_environment(&env(&[("XDG_DATA_HOME", dir.to_str().unwrap())])).unwrap_err();
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
