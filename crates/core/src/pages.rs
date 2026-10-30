use std::collections::VecDeque;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures_core::Stream;
use futures_util::StreamExt as _;

use crate::Error;

/// Everything a list method lists, page by page: a [`Stream`] that fetches
/// the next page as it is reached. A page that fails ends it with its
/// error.
///
/// Without a stream library at hand, [`Pages::next`] reads it:
///
/// ```no_run
/// # async fn list(mut pages: origincloud_core::Pages<String>) -> Result<(), origincloud_core::Error> {
/// while let Some(item) = pages.next().await {
///     println!("{}", item?);
/// }
/// # Ok(())
/// # }
/// ```
pub struct Pages<T> {
    stream: Pin<Box<dyn Stream<Item = Result<T, Error>> + Send>>,
}

struct State<R, F, T> {
    /// The request for the next page; `None` after the last one.
    request: Option<R>,
    buffered: VecDeque<T>,
    fetch: F,
    set_page_token: fn(&mut R, String),
}

impl<T: Send + 'static> Pages<T> {
    /// The items `fetch` lists from `request`'s page on: it answers with a
    /// page's items and the next page's token, which `set_page_token` puts
    /// in the request for it, until the token is empty. The generated
    /// clients call it.
    pub fn new<R, F, Fut>(request: R, fetch: F, set_page_token: fn(&mut R, String)) -> Self
    where
        R: Clone + Send + 'static,
        F: Fn(R) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(Vec<T>, String), Error>> + Send + 'static,
    {
        let state = State {
            request: Some(request),
            buffered: VecDeque::new(),
            fetch,
            set_page_token,
        };
        let stream = futures_util::stream::unfold(state, |mut state| async move {
            loop {
                if let Some(item) = state.buffered.pop_front() {
                    return Some((Ok(item), state));
                }
                let mut request = state.request.take()?;
                match (state.fetch)(request.clone()).await {
                    Ok((items, next_page_token)) => {
                        state.buffered.extend(items);
                        if !next_page_token.is_empty() {
                            (state.set_page_token)(&mut request, next_page_token);
                            state.request = Some(request);
                        }
                    }
                    Err(err) => return Some((Err(err), state)),
                }
            }
        });
        Self {
            stream: Box::pin(stream),
        }
    }

    /// The next item, fetching the next page when it is needed; `None`
    /// after the last one.
    #[allow(clippy::should_implement_trait)] // Stream::next is StreamExt's.
    pub async fn next(&mut self) -> Option<Result<T, Error>> {
        self.stream.next().await
    }
}

impl<T> Stream for Pages<T> {
    type Item = Result<T, Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.stream.as_mut().poll_next(cx)
    }
}

impl<T> fmt::Debug for Pages<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pages").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ApiError;

    #[tokio::test]
    async fn pages_are_followed_until_the_token_is_empty() {
        let mut pages = Pages::new(
            String::new(),
            |token: String| async move {
                match token.as_str() {
                    "" => Ok((vec![1, 2], "p2".to_owned())),
                    "p2" => Ok((vec![], "p3".to_owned())),
                    "p3" => Ok((vec![3], String::new())),
                    other => panic!("token {other}"),
                }
            },
            |request, token| *request = token,
        );
        let mut got = Vec::new();
        while let Some(item) = pages.next().await {
            got.push(item.unwrap());
        }
        assert_eq!(got, [1, 2, 3]);
    }

    #[tokio::test]
    async fn a_failed_page_ends_the_stream_with_its_error() {
        let mut pages = Pages::new(
            String::new(),
            |token: String| async move {
                match token.as_str() {
                    "" => Ok((vec![1], "p2".to_owned())),
                    _ => Err(Error::from(ApiError::from_response(400, b""))),
                }
            },
            |request, token| *request = token,
        );
        assert_eq!(pages.next().await.unwrap().unwrap(), 1);
        assert!(pages.next().await.unwrap().is_err());
        assert!(pages.next().await.is_none());
    }
}
