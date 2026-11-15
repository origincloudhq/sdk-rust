//! What differs between a native target and a browser
//! (`wasm32-unknown-unknown`): whether futures must be `Send`, how to wait,
//! how to lock across a wait, and what time it is.
//!
//! In a browser, reqwest's futures hold JavaScript promises
//! (`Rc<RefCell<..>>`), which are not `Send`, and there is one thread, so
//! nothing needs them to be. Natively the bounds stay as they were, so a
//! client can be moved between Tokio's worker threads.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;
#[cfg(any(not(target_arch = "wasm32"), feature = "service-account-wasm"))]
use std::time::SystemTime;

use futures_core::Stream;

#[cfg(not(target_arch = "wasm32"))]
mod bounds {
    /// `Send` natively; nothing in a browser, where futures holding
    /// JavaScript values aren't `Send`. The bound the SDK puts on the
    /// futures and values it boxes.
    pub trait MaybeSend: Send {}
    impl<T: Send + ?Sized> MaybeSend for T {}
}

#[cfg(target_arch = "wasm32")]
mod bounds {
    /// `Send` natively; nothing in a browser, where futures holding
    /// JavaScript values aren't `Send`. The bound the SDK puts on the
    /// futures and values it boxes.
    pub trait MaybeSend {}
    impl<T: ?Sized> MaybeSend for T {}
}

pub use bounds::MaybeSend;

/// A boxed future: `Send` natively, not in a browser. (Named apart from
/// `futures::future::BoxFuture`, which is always `Send`.)
#[cfg(not(target_arch = "wasm32"))]
pub type MaybeSendFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
/// A boxed future: `Send` natively, not in a browser. (Named apart from
/// `futures::future::BoxFuture`, which is always `Send`.)
#[cfg(target_arch = "wasm32")]
pub type MaybeSendFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// A boxed stream: `Send` natively, not in a browser.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) type BoxStream<T> = Pin<Box<dyn Stream<Item = T> + Send>>;
/// A boxed stream: `Send` natively, not in a browser.
#[cfg(target_arch = "wasm32")]
pub(crate) type BoxStream<T> = Pin<Box<dyn Stream<Item = T>>>;

/// A lock held across an `.await`: Tokio's natively, as before; in a
/// browser, where Tokio isn't a dependency, `futures-util`'s.
#[cfg(all(target_arch = "wasm32", feature = "service-account-wasm"))]
pub(crate) use futures_util::lock::Mutex;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use tokio::sync::Mutex;

/// The longest a browser timer waits: `setTimeout` takes a signed 32-bit
/// number of milliseconds, and fires at once for a longer one.
const MAX_BROWSER_TIMER: Duration = Duration::from_millis(i32::MAX as u64);

/// Waits `delay`: on Tokio's timer natively; on the browser's
/// (`setTimeout`) in a browser, which has no Tokio timer driver, so
/// `tokio::time::sleep` would panic there. A browser waits at most
/// [`MAX_BROWSER_TIMER`] (about 24.8 days), as a longer delay, such as a
/// server's retry delay, would otherwise fire at once or panic.
pub(crate) async fn sleep(delay: Duration) {
    #[cfg(not(target_arch = "wasm32"))]
    tokio::time::sleep(delay).await;
    #[cfg(target_arch = "wasm32")]
    gloo_timers::future::sleep(browser_timer(delay)).await;
}

/// `delay`, at most as long as a browser timer can wait.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn browser_timer(delay: Duration) -> Duration {
    delay.min(MAX_BROWSER_TIMER)
}

/// The current time. In a browser `SystemTime::now` panics (the standard
/// library has no clock on `wasm32-unknown-unknown`), so it is read from
/// JavaScript's `Date.now()` there, in milliseconds since the Unix epoch.
#[cfg(any(not(target_arch = "wasm32"), feature = "service-account-wasm"))]
pub(crate) fn now() -> SystemTime {
    #[cfg(not(target_arch = "wasm32"))]
    {
        SystemTime::now()
    }
    #[cfg(target_arch = "wasm32")]
    {
        // A whole, non-negative number of milliseconds; `as` saturates.
        let millis = js_sys::Date::now() as u64;
        SystemTime::UNIX_EPOCH + Duration::from_millis(millis)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_browser_timer_waits_at_most_what_set_timeout_takes() {
        let max = Duration::from_millis(2_147_483_647);
        assert_eq!(
            browser_timer(Duration::from_millis(200)),
            Duration::from_millis(200)
        );
        assert_eq!(browser_timer(max), max);
        assert_eq!(browser_timer(max + Duration::from_millis(1)), max);
        assert_eq!(
            browser_timer(Duration::from_secs(10 * 365 * 24 * 60 * 60)),
            max
        );
        assert_eq!(browser_timer(Duration::MAX), max);
    }
}
