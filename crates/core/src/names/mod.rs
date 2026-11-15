//! Full resource names (AIP-122), the platform's identifiers:
//! `//{service}.origincloudapis.eu/{relative name}`, such as
//! `//pvs.origincloudapis.eu/projects/p/zones/eu-central1-az1/volumes/vol-1`.
//!
//! A resource's `name` is its relative name; its full name puts the host of
//! the service that owns it in front, with no scheme and no API version.
//! References in request bodies are full names. A request's path fields
//! (`name`, `parent`) take either: a client given a full name of its own
//! service sends its relative name, and refuses one of another service
//! before sending anything.
//!
//! ```
//! use origincloud_core::names::FullName;
//!
//! let volume: FullName =
//!     "//pvs.origincloudapis.eu/projects/p/zones/eu-central1-az1/volumes/vol-1".parse()?;
//! assert_eq!(volume.service(), "pvs");
//! assert_eq!(volume.host(), "pvs.origincloudapis.eu");
//! assert_eq!(volume.relative(), "projects/p/zones/eu-central1-az1/volumes/vol-1");
//! assert_eq!(
//!     volume.url("v1beta1")?,
//!     "https://pvs.origincloudapis.eu/v1beta1/projects/p/zones/eu-central1-az1/volumes/vol-1"
//! );
//! assert_eq!(
//!     FullName::from_relative("pvs", "projects/p/zones/eu-central1-az1/volumes/vol-1")?,
//!     volume
//! );
//! # Ok::<(), origincloud_core::names::FullNameError>(())
//! ```

use std::fmt;
use std::str::FromStr;

use serde::de::{self, Deserializer, Visitor};
use serde::{Deserialize, Serialize, Serializer};

include!("generated/services.rs");

/// The domain every service's host is under.
pub const DOMAIN: &str = "origincloudapis.eu";

/// A full resource name: `//`, the global host of the service that owns the
/// resource, `/` and its relative name.
///
/// Built only by [`FullName::parse`] (the full form) or
/// [`FullName::from_relative`] (a service and a relative name), both of
/// which refuse anything but the one canonical spelling, so two names are
/// equal exactly when they name the same resource. `Display`, `FromStr`
/// and serde use the full form, and only it.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FullName {
    /// `//{service}.{DOMAIN}/{relative}`, kept whole.
    full: String,
    /// The length of the service's short code.
    service_len: usize,
}

/// Why a string is not a full resource name, or not one that can be used
/// where it was given.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum FullNameError {
    /// An empty string.
    #[error("an empty string names no resource")]
    Empty,
    /// A URN, `urn:…`.
    #[error(
        "{0:?} is a URN; a resource is named by its full resource name, //{{service}}.{DOMAIN}/{{name}}"
    )]
    Urn(String),
    /// A URL, or anything else with a scheme.
    #[error("{0:?} is a URL; a full resource name has no scheme, //{{service}}.{DOMAIN}/{{name}}")]
    Scheme(String),
    /// A relative name where a full one is expected.
    #[error("{0:?} is a relative resource name; a full one starts with //{{service}}.{DOMAIN}/")]
    Relative(String),
    /// A bare id, with no collection.
    #[error(
        "{0:?} is an id; a resource is named by its full resource name, //{{service}}.{DOMAIN}/{{name}}"
    )]
    BareId(String),
    /// A host that is no service's.
    #[error("{name:?} names no resource: {host} is not the host of one of the platform's APIs")]
    UnknownService {
        /// What was given.
        name: String,
        /// The host it names.
        host: String,
    },
    /// A regional or zonal host, a front door rather than the service.
    #[error(
        "{name:?} names {host}; a full resource name has the service's global host, {service}.{DOMAIN}"
    )]
    NotGlobalHost {
        /// What was given.
        name: String,
        /// The host it names.
        host: String,
        /// The short code of the service whose front door it is.
        service: String,
    },
    /// An API version in the name.
    #[error("{name:?} carries the API version {version:?}; a full resource name has none")]
    Version {
        /// What was given.
        name: String,
        /// The version.
        version: String,
    },
    /// A service's host with nothing after it.
    #[error("{0:?} has no relative name after its service")]
    NoRelativeName(String),
    /// A full name where a relative one is expected.
    #[error("{0:?} is a full resource name; a relative one is expected here")]
    NotRelative(String),
    /// A name ending in `/`.
    #[error("{0:?} ends with a '/'")]
    TrailingSlash(String),
    /// A name with an empty segment.
    #[error("{0:?} has an empty segment")]
    EmptySegment(String),
    /// A name with a `.` or `..` segment.
    #[error("{name:?} has the segment {segment:?}, which a resource name can't")]
    DotSegment {
        /// What was given.
        name: String,
        /// The segment.
        segment: String,
    },
    /// A segment with a character a name can't hold.
    #[error(
        "{name:?} has the segment {segment:?}: a segment is letters, digits and '-', '.', '_' or '~'"
    )]
    InvalidSegment {
        /// What was given.
        name: String,
        /// The segment.
        segment: String,
    },
    /// [`FullName::url`] given something that is no API version.
    #[error("{0:?} is not an API version, such as v1 or v1beta1")]
    NotAVersion(String),
}

impl FullName {
    /// Parses the full form, `//{host}/{relative}`, and nothing else.
    ///
    /// Refused: a URN, a URL (any scheme), a relative name, a bare id, a
    /// host that isn't the global host of one of [`RESOURCE_SERVICES`] (a
    /// regional or zonal one included), an API version as the first
    /// segment, a trailing `/`, and an empty, `.`, `..` or otherwise
    /// invalid segment.
    pub fn parse(name: &str) -> Result<FullName, FullNameError> {
        let Some(rest) = name.strip_prefix("//") else {
            return Err(not_full(name));
        };
        let Some((host, relative)) = rest.split_once('/') else {
            return Err(match service_of_host(name, rest) {
                Ok(_) => FullNameError::NoRelativeName(name.to_owned()),
                Err(e) => e,
            });
        };
        let service = service_of_host(name, host)?;
        if relative.is_empty() {
            return Err(FullNameError::NoRelativeName(name.to_owned()));
        }
        check_relative(name, relative)?;
        Ok(FullName {
            full: name.to_owned(),
            service_len: service.len(),
        })
    }

    /// The full name of the resource `relative` (its `name`) of `service`,
    /// a short code such as `pvs`: `//{service}.origincloudapis.eu/` +
    /// `relative`.
    ///
    /// Refused: a service that isn't one of [`RESOURCE_SERVICES`], a full
    /// name given as the relative one, an API version as the first segment,
    /// a trailing `/`, and an empty, `.`, `..` or otherwise invalid
    /// segment.
    pub fn from_relative(service: &str, relative: &str) -> Result<FullName, FullNameError> {
        if relative.starts_with("//") {
            return Err(FullNameError::NotRelative(relative.to_owned()));
        }
        let host = format!("{service}.{DOMAIN}");
        if !RESOURCE_SERVICES.contains(&service) {
            return Err(FullNameError::UnknownService {
                name: relative.to_owned(),
                host,
            });
        }
        if relative.is_empty() {
            return Err(FullNameError::NoRelativeName(relative.to_owned()));
        }
        check_relative(relative, relative)?;
        Ok(FullName {
            full: format!("//{host}/{relative}"),
            service_len: service.len(),
        })
    }

    /// The short code of the service that owns the resource (`pvs`).
    pub fn service(&self) -> &str {
        &self.full[2..2 + self.service_len]
    }

    /// The service's host (`pvs.origincloudapis.eu`).
    pub fn host(&self) -> &str {
        &self.full[2..self.host_end()]
    }

    /// The relative name, the resource's `name`: the full name without
    /// `//{host}/`.
    pub fn relative(&self) -> &str {
        &self.full[self.host_end() + 1..]
    }

    /// The full form, as `Display` writes it.
    pub fn as_str(&self) -> &str {
        &self.full
    }

    /// The URL that calls the resource,
    /// `https://{host}/{version}/{relative}`, for an API version such as
    /// `v1beta1`.
    pub fn url(&self, version: &str) -> Result<String, FullNameError> {
        if !is_version(version) {
            return Err(FullNameError::NotAVersion(version.to_owned()));
        }
        Ok(format!(
            "https://{}/{version}/{}",
            self.host(),
            self.relative()
        ))
    }

    /// The principal the name is, if it is one: a user, a service account,
    /// a service agent or an internal platform service, each an iam name.
    pub fn principal(&self) -> Option<Principal<'_>> {
        if self.service() != "iam" {
            return None;
        }
        let segments: Vec<&str> = self.relative().split('/').collect();
        match segments.as_slice() {
            ["users", user] => Some(Principal::User { user }),
            [
                "organizations",
                organization,
                "serviceAccounts",
                service_account,
            ] => Some(Principal::ServiceAccount {
                organization,
                service_account,
            }),
            ["serviceAgents", service_agent] => Some(Principal::ServiceAgent { service_agent }),
            ["services", service] => Some(Principal::Service { service }),
            _ => None,
        }
    }

    /// The role's id if the name is a role,
    /// `//iam.origincloudapis.eu/roles/{role}`: `iam.editor`. A role's id
    /// writes its parts with dots; `roles/iam/editor` is no role.
    pub fn role(&self) -> Option<&str> {
        if self.service() != "iam" {
            return None;
        }
        self.relative()
            .strip_prefix("roles/")
            .filter(|role| !role.contains('/'))
    }

    /// The full name of the role `role`, such as `iam.editor`:
    /// `//iam.origincloudapis.eu/roles/iam.editor`.
    pub fn of_role(role: &str) -> Result<FullName, FullNameError> {
        if role.contains('/') {
            return Err(FullNameError::InvalidSegment {
                name: role.to_owned(),
                segment: role.to_owned(),
            });
        }
        FullName::from_relative("iam", &format!("roles/{role}"))
    }

    /// Where the host ends in `full`.
    fn host_end(&self) -> usize {
        2 + self.service_len + 1 + DOMAIN.len()
    }
}

/// Who acts: a principal, as a member of a role binding or the actor of an
/// audit entry names it. Each is an iam full name.
///
/// An organisation acting on its own has no full name yet, and so no
/// variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Principal<'a> {
    /// A user: `//iam.origincloudapis.eu/users/{user}`.
    User {
        /// The user's id.
        user: &'a str,
    },
    /// An organization's service account:
    /// `//iam.origincloudapis.eu/organizations/{organization}/serviceAccounts/{service_account}`.
    ServiceAccount {
        /// The organization's id.
        organization: &'a str,
        /// The service account's id.
        service_account: &'a str,
    },
    /// A service agent, a platform service acting in a customer's project:
    /// `//iam.origincloudapis.eu/serviceAgents/{service_agent}`.
    ServiceAgent {
        /// The service agent's name.
        service_agent: &'a str,
    },
    /// An internal platform service:
    /// `//iam.origincloudapis.eu/services/{service}`.
    Service {
        /// The service's name.
        service: &'a str,
    },
}

impl Principal<'_> {
    /// The principal's full name.
    pub fn full_name(&self) -> Result<FullName, FullNameError> {
        let (relative, parts) = match *self {
            Principal::User { user } => (format!("users/{user}"), vec![user]),
            Principal::ServiceAccount {
                organization,
                service_account,
            } => (
                format!("organizations/{organization}/serviceAccounts/{service_account}"),
                vec![organization, service_account],
            ),
            Principal::ServiceAgent { service_agent } => (
                format!("serviceAgents/{service_agent}"),
                vec![service_agent],
            ),
            Principal::Service { service } => (format!("services/{service}"), vec![service]),
        };
        // A part with a '/' would make another name, of no principal.
        if let Some(part) = parts.iter().find(|p| p.contains('/')) {
            return Err(FullNameError::InvalidSegment {
                name: relative.clone(),
                segment: (*part).to_owned(),
            });
        }
        FullName::from_relative("iam", &relative)
    }
}

/// The short code of the service whose global host is `host`, or why there
/// is none.
fn service_of_host<'h>(name: &str, host: &'h str) -> Result<&'h str, FullNameError> {
    let code = host.strip_suffix(DOMAIN).and_then(|h| h.strip_suffix('.'));
    if let Some(code) = code.filter(|c| RESOURCE_SERVICES.contains(c)) {
        return Ok(code);
    }
    // `{service}.{region or zone}.origincloudapis.eu`: a front door, not
    // the service's name.
    let front_door = code
        .and_then(|h| h.split_once('.'))
        .filter(|(_, location)| !location.is_empty())
        .map(|(code, _)| code)
        .filter(|code| RESOURCE_SERVICES.contains(code));
    Err(match front_door {
        Some(service) => FullNameError::NotGlobalHost {
            name: name.to_owned(),
            host: host.to_owned(),
            service: service.to_owned(),
        },
        None => FullNameError::UnknownService {
            name: name.to_owned(),
            host: host.to_owned(),
        },
    })
}

/// Why `name`, which doesn't start with `//`, is not a full name.
fn not_full(name: &str) -> FullNameError {
    let owned = name.to_owned();
    if name.is_empty() {
        FullNameError::Empty
    } else if name
        .get(..4)
        .is_some_and(|p| p.eq_ignore_ascii_case("urn:"))
    {
        FullNameError::Urn(owned)
    } else if name.contains("://") {
        FullNameError::Scheme(owned)
    } else if name.contains('/') {
        FullNameError::Relative(owned)
    } else {
        FullNameError::BareId(owned)
    }
}

/// Checks a non-empty relative name; `name` is what the error reports.
fn check_relative(name: &str, relative: &str) -> Result<(), FullNameError> {
    if relative.ends_with('/') {
        return Err(FullNameError::TrailingSlash(name.to_owned()));
    }
    let mut segments = relative.split('/').peekable();
    if let Some(first) = segments.peek()
        && is_version(first)
    {
        return Err(FullNameError::Version {
            name: name.to_owned(),
            version: (*first).to_owned(),
        });
    }
    for segment in segments {
        check_segment(name, segment)?;
    }
    Ok(())
}

/// A segment is non-empty, neither `.` nor `..`, and made of RFC 3986's
/// unreserved characters, so a name is a URL path as it is.
fn check_segment(name: &str, segment: &str) -> Result<(), FullNameError> {
    if segment.is_empty() {
        return Err(FullNameError::EmptySegment(name.to_owned()));
    }
    if segment == "." || segment == ".." {
        return Err(FullNameError::DotSegment {
            name: name.to_owned(),
            segment: segment.to_owned(),
        });
    }
    let unreserved = |c: char| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~');
    if !segment.chars().all(unreserved) {
        return Err(FullNameError::InvalidSegment {
            name: name.to_owned(),
            segment: segment.to_owned(),
        });
    }
    Ok(())
}

/// Whether `segment` reads as an API version (AIP-185): `v{major}`,
/// optionally followed by `alpha` or `beta` and a number.
fn is_version(segment: &str) -> bool {
    let Some(rest) = segment.strip_prefix('v') else {
        return false;
    };
    let major_len = rest.chars().take_while(char::is_ascii_digit).count();
    if major_len == 0 || rest.starts_with('0') {
        return false;
    }
    let stability = &rest[major_len..];
    let number = stability
        .strip_prefix("alpha")
        .or_else(|| stability.strip_prefix("beta"));
    match number {
        None => stability.is_empty(),
        Some(number) => number.chars().all(|c| c.is_ascii_digit()),
    }
}

/// The relative name to send in a request's path for `value`, the path
/// field `field` of a call to the service `own` (a short code): `value` as
/// it is when it isn't a full name, its relative name when it is one of
/// `own`'s, and an error when it is another service's, a URN, a URL or no
/// valid full name.
pub(crate) fn path_name<'v>(own: &str, field: &str, value: &'v str) -> Result<&'v str, String> {
    if !value.starts_with("//") {
        return match not_full(value) {
            e @ (FullNameError::Urn(_) | FullNameError::Scheme(_)) => Err(format!("{field}: {e}")),
            _ => Ok(value),
        };
    }
    let name = FullName::parse(value).map_err(|e| format!("{field}: {e}"))?;
    if name.service() != own {
        return Err(format!(
            "{field} must be a name on {own}.{DOMAIN}, the host this client calls, got {value:?}, a name on {}",
            name.host()
        ));
    }
    Ok(&value[value.len() - name.relative().len()..])
}

impl fmt::Display for FullName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.full)
    }
}

impl fmt::Debug for FullName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("FullName").field(&self.full).finish()
    }
}

impl FromStr for FullName {
    type Err = FullNameError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        FullName::parse(s)
    }
}

impl AsRef<str> for FullName {
    fn as_ref(&self) -> &str {
        &self.full
    }
}

impl From<FullName> for String {
    fn from(name: FullName) -> String {
        name.full
    }
}

/// The full form, as a string.
impl Serialize for FullName {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.full)
    }
}

/// A string in the full form; anything [`FullName::parse`] refuses is an
/// error.
impl<'de> Deserialize<'de> for FullName {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct FullNameVisitor;

        impl Visitor<'_> for FullNameVisitor {
            type Value = FullName;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a full resource name, //{service}.origincloudapis.eu/{name}")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<FullName, E> {
                FullName::parse(value).map_err(E::custom)
            }
        }

        deserializer.deserialize_str(FullNameVisitor)
    }
}

#[cfg(test)]
mod tests;
