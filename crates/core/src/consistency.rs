//! Where a [`Client`](crate::Client) keeps the consistency token it sends
//! with its calls and replaces with the one each answer carries.
//!
//! By default each client has its own, a [`ConsistencyTokenCell`], shared
//! by its clones. A program that wants several clients, or code outside the
//! SDK, to share one token gives each the same store
//! ([`ClientBuilder::consistency_token_store`](crate::ClientBuilder::consistency_token_store)),
//! such as one [`ConsistencyTokenCell`] in an [`Arc`](std::sync::Arc), or
//! an implementation of its own, such as one a browser tab's state holds.

use std::fmt;
use std::sync::Mutex;

/// Holds the consistency token a client presents. The client reads it
/// before each attempt of an authenticated call, replaces it with the token
/// an answer carries, and [`remove`](Self::remove)s one the API refused.
///
/// It must be `Send + Sync` on every target, as [`Client`](crate::Client)
/// is, so that a client can live in a browser app's reactive state. In a
/// browser a store therefore holds the token in something `Send + Sync`,
/// such as a `Mutex` or a reactive signal, and copies it to
/// `web_sys::Storage` from there if it is to outlive the page, rather than
/// wrapping the `Storage` itself, which isn't `Send`. Its methods are
/// synchronous, and are called with no lock of the client's held.
pub trait ConsistencyTokenStore: Send + Sync + fmt::Debug {
    /// The token held; empty when none is.
    fn get(&self) -> String;

    /// Replaces the token held with `token`.
    fn set(&self, token: String);

    /// Replaces the token held with `answered`, the token the answer to a
    /// call that presented `presented` (empty for none) carried. A store
    /// that holds a token other than `presented` by then was written by a
    /// concurrent call in between, and may keep one or the other as it
    /// sees fit; the default [`set`](Self::set)s `answered`, so the token
    /// answered last is kept.
    fn replace(&self, presented: &str, answered: String) {
        let _ = presented;
        self.set(answered);
    }

    /// Forgets `refused`, a token the API refused to read, if it is still
    /// the one held; a token that replaced it in the meantime stays. The
    /// default reads and then sets the token, so it can lose a token set
    /// in between; a store that can do it at once should.
    fn remove(&self, refused: &str) {
        if self.get() == refused {
            self.set(String::new());
        }
    }
}

/// A consistency token in memory: what a client holds by default. Give one
/// in an [`Arc`](std::sync::Arc) to several clients for them to share it.
#[derive(Default)]
pub struct ConsistencyTokenCell(Mutex<String>);

impl ConsistencyTokenCell {
    /// A cell holding no token.
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, String> {
        // A token is replaced whole, so one a panic left behind is whole.
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl ConsistencyTokenStore for ConsistencyTokenCell {
    fn get(&self) -> String {
        self.lock().clone()
    }

    fn set(&self, token: String) {
        *self.lock() = token;
    }

    fn remove(&self, refused: &str) {
        let mut held = self.lock();
        if *held == refused {
            held.clear();
        }
    }
}

/// The token itself is left out: it is a credential of sorts, naming what
/// the client wrote.
impl fmt::Debug for ConsistencyTokenCell {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConsistencyTokenCell")
            .field("holds_token", &!self.lock().is_empty())
            .finish()
    }
}

/// Why the API refuses a request whose `Origin-Consistency-Token` header
/// isn't a token it issued (`MALFORMED_TOKEN` in the API's
/// `oc-lib-http-api`), with status 400. The refusal comes before the
/// request does anything, so the request is sent again without it.
pub(crate) const MALFORMED_TOKEN: &str =
    "the Origin-Consistency-Token header must be a token a response of this API carried";

/// Whether `err` is the API refusing the consistency token presented, and
/// nothing else: a 400 with exactly [`MALFORMED_TOKEN`] as its message.
pub(crate) fn is_malformed_token(err: &crate::Error) -> bool {
    err.api()
        .is_some_and(|e| e.http_status == Some(400) && e.message == MALFORMED_TOKEN)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;
    use crate::error::ApiError;

    #[test]
    fn a_cell_forgets_a_refused_token_only_while_it_holds_it() {
        let cell = ConsistencyTokenCell::new();
        assert_eq!(cell.get(), "");
        cell.set("v1.a".to_owned());
        cell.remove("v1.b");
        assert_eq!(cell.get(), "v1.a");
        cell.remove("v1.a");
        assert_eq!(cell.get(), "");
        assert_eq!(
            format!("{cell:?}"),
            "ConsistencyTokenCell { holds_token: false }"
        );
    }

    #[test]
    fn the_default_remove_forgets_only_the_refused_token() {
        #[derive(Debug, Default)]
        struct Plain(Mutex<String>);
        impl ConsistencyTokenStore for Plain {
            fn get(&self) -> String {
                self.0.lock().unwrap().clone()
            }
            fn set(&self, token: String) {
                *self.0.lock().unwrap() = token;
            }
        }
        let store = Plain::default();
        store.set("v1.a".to_owned());
        store.remove("v1.b");
        assert_eq!(store.get(), "v1.a");
        store.remove("v1.a");
        assert_eq!(store.get(), "");
    }

    #[test]
    fn only_the_malformed_token_refusal_is_one() {
        let refusal = |status: u16, message: &str| {
            Error::from(ApiError::from_response(
                status,
                serde_json::json!({"error": {"code": status, "status": "INVALID_ARGUMENT", "message": message}})
                    .to_string()
                    .as_bytes(),
            ))
        };
        assert!(is_malformed_token(&refusal(400, MALFORMED_TOKEN)));
        assert!(!is_malformed_token(&refusal(
            400,
            "the request body must be at most 1 bytes"
        )));
        assert!(!is_malformed_token(&refusal(
            400,
            &format!("{MALFORMED_TOKEN}.")
        )));
        assert!(!is_malformed_token(&refusal(409, MALFORMED_TOKEN)));
        assert!(!is_malformed_token(&Error::NoCredentials));
    }
}
