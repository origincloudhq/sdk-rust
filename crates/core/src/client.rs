use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use url::Url;

use crate::Error;
use crate::credentials::{self, Credentials};

/// This SDK's version, sent in the `User-Agent` header.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

const ENV_REGION: &str = "ORIGINCLOUD_REGION";
const DEFAULT_MAX_ATTEMPTS: u32 = 5;
/// The longest the API waits for an operation in one call.
const DEFAULT_OPERATION_WAIT: Duration = Duration::from_secs(60);

/// What the clients of every service share: credentials, endpoints, an
/// HTTP client and the consistency token. Cloning it is cheap, and the
/// clones share all of it.
#[derive(Clone)]
pub struct Client {
    pub(crate) inner: Arc<Inner>,
}

pub(crate) struct Inner {
    pub(crate) http: reqwest::Client,
    pub(crate) credentials: Arc<dyn Credentials>,
    region: String,
    endpoint: Option<Url>,
    pub(crate) user_agent: String,
    pub(crate) max_attempts: u32,
    /// The timeout of each attempt, when the builder set one.
    pub(crate) timeout: Option<Duration>,
    /// How long one call waiting on an operation asks the API to wait.
    pub(crate) operation_wait: Duration,
    token: Mutex<String>,
}

impl Client {
    /// A client configured by the environment: the credentials
    /// [`default_credentials`](crate::default_credentials) finds, and the
    /// region `ORIGINCLOUD_REGION` names, if any.
    pub fn new() -> Result<Self, Error> {
        Self::builder().build()
    }

    /// A builder of a client configured otherwise.
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// The endpoint of the service whose host starts with `shortcode`, such
    /// as `pvs`. A regional service answers at its region's front door when
    /// the client has a region; a global one always at its regionless host.
    /// The generated clients call it.
    ///
    /// A shortcode that isn't a host label, `[a-z0-9-]+`, makes every call
    /// of the service fail with [`Error::Config`].
    pub fn service(&self, shortcode: &str, regional: bool) -> Service {
        let base = match &self.inner.endpoint {
            Some(endpoint) => Ok(endpoint.clone()),
            None if !is_label(shortcode) => Err(format!(
                "the service shortcode must be [a-z0-9-]+, got {shortcode:?}"
            )),
            None => {
                let region = &self.inner.region;
                let host = if regional && !region.is_empty() {
                    format!("{shortcode}.{region}.origincloudapis.eu")
                } else {
                    format!("{shortcode}.origincloudapis.eu")
                };
                Url::parse(&format!("https://{host}"))
                    .map_err(|e| format!("the service host {host:?} isn't valid: {e}"))
            }
        };
        Service {
            client: self.clone(),
            base,
        }
    }

    /// The consistency token the client holds: the one the last answer
    /// carrying one brought, or empty before any. Hand it to
    /// [`Client::set_consistency_token`] of a client in another process for
    /// its reads to show this client's writes.
    pub fn consistency_token(&self) -> String {
        self.inner.token.lock().expect("the token lock").clone()
    }

    /// Replaces the consistency token the client sends with its calls.
    ///
    /// The API merges a mutation's position into the token the mutation
    /// presented, so writes made one after another each answer with a token
    /// covering all of them. Writes made concurrently each present the same
    /// earlier token, and the client keeps whichever token is answered last:
    /// a read then sees that write, not necessarily the others, until it
    /// presents a token covering them, such as one from waiting on their
    /// operations.
    pub fn set_consistency_token(&self, token: impl Into<String>) {
        *self.inner.token.lock().expect("the token lock") = token.into();
    }
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("credentials", &self.inner.credentials)
            .field("region", &self.inner.region)
            .field("endpoint", &self.inner.endpoint.as_ref().map(Url::as_str))
            .field("max_attempts", &self.inner.max_attempts)
            .finish_non_exhaustive()
    }
}

/// Configures a [`Client`].
#[derive(Default)]
pub struct ClientBuilder {
    credentials: Option<Arc<dyn Credentials>>,
    region: Option<String>,
    endpoint: Option<String>,
    http: Option<reqwest::Client>,
    user_agent: Option<String>,
    max_attempts: Option<u32>,
    timeout: Option<Duration>,
    operation_wait: Option<Duration>,
}

impl ClientBuilder {
    /// Sets the credentials, instead of taking them from the environment or
    /// the CLI's session.
    pub fn credentials(mut self, credentials: impl Credentials + 'static) -> Self {
        self.credentials = Some(Arc::new(credentials));
        self
    }

    /// Sends calls to a region's front door,
    /// `{service}.{region}.origincloudapis.eu`, for the services that have
    /// one. It overrides `ORIGINCLOUD_REGION`; an empty region uses the
    /// regionless endpoint. A region, from either, is `[a-z0-9-]+`, such as
    /// `eu-central1`: [`ClientBuilder::build`] refuses any other.
    pub fn region(mut self, region: impl Into<String>) -> Self {
        self.region = Some(region.into());
        self
    }

    /// Sends the calls of every service to one base URL, such as a test
    /// server's, instead of the service's own host. [`ClientBuilder::build`]
    /// refuses one that isn't an absolute URL with a host.
    pub fn endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = Some(endpoint.into());
        self
    }

    /// Sets the HTTP client calls are sent with. Its timeout, if any,
    /// bounds each attempt.
    pub fn http_client(mut self, http: reqwest::Client) -> Self {
        self.http = Some(http);
        self
    }

    /// Adds a product, such as `my-tool/1.2`, to the `User-Agent` header.
    pub fn user_agent(mut self, product: impl Into<String>) -> Self {
        self.user_agent = Some(product.into());
        self
    }

    /// Sets how often a retryable call is sent at most, the first attempt
    /// included; 1 turns retries off. The default is 5.
    pub fn max_attempts(mut self, attempts: u32) -> Self {
        self.max_attempts = Some(attempts);
        self
    }

    /// Bounds each attempt of a call by `timeout`; an attempt that takes
    /// longer fails as a transport error, which a safe call retries. By
    /// default, only the HTTP client's own timeout, if any, bounds it.
    ///
    /// Waiting on an operation asks the API to wait less than this, so that
    /// a wait call doesn't time out on the way; see
    /// [`ClientBuilder::operation_wait`].
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Sets how long one call of [`Operation::wait`](crate::Operation::wait)
    /// asks the API to wait for the operation before answering with it as
    /// it is, in whole seconds, at least one; the default, and the most the
    /// API waits, is a minute. With a [`ClientBuilder::timeout`], a call
    /// asks for a second less than it at most. A wait call that times out
    /// all the same, such as under the HTTP client's own timeout, counts as
    /// the operation not being done yet, and the next one is sent.
    pub fn operation_wait(mut self, wait: Duration) -> Self {
        self.operation_wait = Some(wait);
        self
    }

    /// The client. It fails when no credentials are found or an option is
    /// invalid.
    pub fn build(self) -> Result<Client, Error> {
        self.build_with(&|name| std::env::var(name).ok())
    }

    /// [`ClientBuilder::build`] with the environment read through `env`.
    pub(crate) fn build_with(self, env: &dyn Fn(&str) -> Option<String>) -> Result<Client, Error> {
        if self.timeout == Some(Duration::ZERO) {
            return Err(Error::Config(
                "the timeout must be longer than zero".to_owned(),
            ));
        }
        let max_attempts = self.max_attempts.unwrap_or(DEFAULT_MAX_ATTEMPTS);
        if max_attempts < 1 {
            return Err(Error::Config(format!(
                "the most attempts must be at least 1, got {max_attempts}"
            )));
        }
        let mut user_agent = format!("origincloud-sdk-rust/{VERSION}");
        if let Some(product) = self.user_agent.filter(|p| !p.is_empty()) {
            user_agent = format!("{product} {user_agent}");
        }
        let region = match self.region {
            Some(region) => region,
            None => env(ENV_REGION).unwrap_or_default(),
        };
        if !region.is_empty() && !is_label(&region) {
            return Err(Error::Config(format!(
                "the region must be [a-z0-9-]+, such as eu-central1, got {region:?}"
            )));
        }
        let endpoint = match self.endpoint {
            Some(endpoint) => {
                let parsed = Url::parse(endpoint.trim_end_matches('/'))
                    .ok()
                    .filter(|u| u.has_host() && !u.cannot_be_a_base());
                Some(parsed.ok_or_else(|| {
                    Error::Config(format!(
                        "the endpoint must be an absolute URL, got {endpoint:?}"
                    ))
                })?)
            }
            None => None,
        };
        let http = match self.http {
            Some(http) => http,
            None => reqwest::Client::builder()
                .build()
                .map_err(|e| Error::Config(format!("creating the HTTP client: {e}")))?,
        };
        let credentials = match self.credentials {
            Some(credentials) => credentials,
            None => credentials::from_environment(env)?,
        };
        Ok(Client {
            inner: Arc::new(Inner {
                http,
                credentials,
                region,
                endpoint,
                user_agent,
                max_attempts,
                timeout: self.timeout,
                operation_wait: self.operation_wait.unwrap_or(DEFAULT_OPERATION_WAIT),
                token: Mutex::default(),
            }),
        })
    }
}

impl fmt::Debug for ClientBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientBuilder")
            .field("credentials", &self.credentials)
            .field("region", &self.region)
            .field("endpoint", &self.endpoint)
            .field("max_attempts", &self.max_attempts)
            .field("timeout", &self.timeout)
            .field("operation_wait", &self.operation_wait)
            .finish_non_exhaustive()
    }
}

/// Whether `s` can be one label of a host name, as a service shortcode or
/// a region is: `[a-z0-9-]+`.
fn is_label(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The endpoint of one service, which its generated clients send calls to.
#[derive(Clone, Debug)]
pub struct Service {
    pub(crate) client: Client,
    /// The base URL, or why there is none.
    pub(crate) base: Result<Url, String>,
}

impl Service {
    /// The client the service's calls go through.
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// The base URL calls are sent to, such as `https://pvs.origincloudapis.eu`;
    /// `None` when the service's shortcode can't name a host.
    pub fn base_url(&self) -> Option<&Url> {
        self.base.as_ref().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AccessToken;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn base(client: &Client, regional: bool) -> String {
        client
            .service("pvs", regional)
            .base_url()
            .unwrap()
            .as_str()
            .trim_end_matches('/')
            .to_owned()
    }

    #[test]
    fn endpoints() {
        for (region, regional, want) in [
            ("", true, "https://pvs.origincloudapis.eu"),
            (
                "eu-central1",
                true,
                "https://pvs.eu-central1.origincloudapis.eu",
            ),
            ("eu-central1", false, "https://pvs.origincloudapis.eu"),
        ] {
            let client = Client::builder()
                .credentials(AccessToken::new("t"))
                .region(region)
                .build_with(&no_env)
                .unwrap();
            assert_eq!(
                base(&client, regional),
                want,
                "region {region:?}, regional {regional}"
            );
        }

        let env = |name: &str| (name == ENV_REGION).then(|| "eu-west1".to_owned());
        let client = Client::builder()
            .credentials(AccessToken::new("t"))
            .build_with(&env)
            .unwrap();
        assert_eq!(
            base(&client, true),
            "https://pvs.eu-west1.origincloudapis.eu"
        );
        let client = Client::builder()
            .credentials(AccessToken::new("t"))
            .region("")
            .build_with(&env)
            .unwrap();
        assert_eq!(
            base(&client, true),
            "https://pvs.origincloudapis.eu",
            "an empty region overrides the environment"
        );
    }

    #[test]
    fn a_region_that_isnt_a_host_label_is_refused() {
        for region in [
            "eu/central1",
            "x@evil.com/",
            "eu central1",
            "eu-central1:443",
            "EU-CENTRAL1",
        ] {
            let built = Client::builder()
                .credentials(AccessToken::new("t"))
                .region(region)
                .build_with(&no_env);
            let Err(Error::Config(message)) = built else {
                panic!("region {region:?}: {built:?}");
            };
            assert!(message.contains(&format!("{region:?}")), "{message}");

            let env = |name: &str| (name == ENV_REGION).then(|| region.to_owned());
            let built = Client::builder()
                .credentials(AccessToken::new("t"))
                .build_with(&env);
            assert!(
                matches!(built, Err(Error::Config(_))),
                "ORIGINCLOUD_REGION={region:?}"
            );
        }
    }

    #[test]
    fn a_shortcode_that_isnt_a_host_label_fails_its_calls_instead_of_panicking() {
        let client = Client::builder()
            .credentials(AccessToken::new("t"))
            .build_with(&no_env)
            .unwrap();
        assert!(client.service("x@evil.com/", true).base_url().is_none());
        assert!(client.service("pvs", true).base_url().is_some());
    }

    #[test]
    fn invalid_options_are_refused() {
        let built = Client::builder()
            .credentials(AccessToken::new("t"))
            .max_attempts(0)
            .build_with(&no_env);
        assert!(matches!(built, Err(Error::Config(_))));
        for endpoint in [
            "localhost",
            "",
            "http://",
            "not a url",
            "http://exa mple.com",
            "mailto:x@y",
        ] {
            let built = Client::builder()
                .credentials(AccessToken::new("t"))
                .endpoint(endpoint)
                .build_with(&no_env);
            assert!(
                matches!(built, Err(Error::Config(_))),
                "endpoint {endpoint:?}"
            );
        }
    }

    #[test]
    fn no_credentials_at_all() {
        let env = |name: &str| match name {
            "XDG_DATA_HOME" => Some("/nonexistent/origincloud-sdk-test".to_owned()),
            "ORIGINCLOUD_METADATA_DISABLED" => Some("true".to_owned()),
            _ => None,
        };
        assert!(matches!(
            Client::builder().build_with(&env),
            Err(Error::NoCredentials)
        ));
    }
}
