use std::fmt;
use std::sync::{Arc, Mutex};

use url::Url;

use crate::Error;
use crate::credentials::{self, Credentials};

/// This SDK's version, sent in the `User-Agent` header.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

const ENV_REGION: &str = "ORIGINCLOUD_REGION";
const DEFAULT_MAX_ATTEMPTS: u32 = 5;

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
    pub fn service(&self, shortcode: &str, regional: bool) -> Service {
        let base = match &self.inner.endpoint {
            Some(endpoint) => endpoint.clone(),
            None => {
                let region = &self.inner.region;
                let host = if regional && !region.is_empty() {
                    format!("{shortcode}.{region}.origincloudapis.eu")
                } else {
                    format!("{shortcode}.origincloudapis.eu")
                };
                Url::parse(&format!("https://{host}")).expect("a valid host")
            }
        };
        Service {
            client: self.clone(),
            base,
        }
    }

    /// The consistency token the client holds: the one the last mutation or
    /// finished operation answered with, or empty before any. Hand it to
    /// [`Client::set_consistency_token`] of a client in another process for
    /// its reads to show this client's writes.
    pub fn consistency_token(&self) -> String {
        self.inner.token.lock().expect("the token lock").clone()
    }

    /// Replaces the consistency token the client sends with its calls. The
    /// API merges a mutation's position into the token the mutation
    /// presented, so a client keeps every write it made since.
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
    /// regionless endpoint.
    pub fn region(mut self, region: impl Into<String>) -> Self {
        self.region = Some(region.into());
        self
    }

    /// Sends the calls of every service to one base URL, such as a test
    /// server's, instead of the service's own host.
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

    /// The client. It fails when no credentials are found or an option is
    /// invalid.
    pub fn build(self) -> Result<Client, Error> {
        self.build_with(&|name| std::env::var(name).ok())
    }

    /// [`ClientBuilder::build`] with the environment read through `env`.
    pub(crate) fn build_with(self, env: &dyn Fn(&str) -> Option<String>) -> Result<Client, Error> {
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
            .finish_non_exhaustive()
    }
}

/// The endpoint of one service, which its generated clients send calls to.
#[derive(Clone, Debug)]
pub struct Service {
    pub(crate) client: Client,
    pub(crate) base: Url,
}

impl Service {
    /// The client the service's calls go through.
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// The base URL calls are sent to, such as `https://pvs.origincloudapis.eu`.
    pub fn base_url(&self) -> &Url {
        &self.base
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
    fn invalid_options_are_refused() {
        let built = Client::builder()
            .credentials(AccessToken::new("t"))
            .max_attempts(0)
            .build_with(&no_env);
        assert!(matches!(built, Err(Error::Config(_))));
        let built = Client::builder()
            .credentials(AccessToken::new("t"))
            .endpoint("localhost")
            .build_with(&no_env);
        assert!(matches!(built, Err(Error::Config(_))));
    }

    #[test]
    fn no_credentials_at_all() {
        let env = |name: &str| {
            (name == "XDG_DATA_HOME").then(|| "/nonexistent/origincloud-sdk-test".to_owned())
        };
        assert!(matches!(
            Client::builder().build_with(&env),
            Err(Error::NoCredentials)
        ));
    }
}
