//! Bound body reads and retain the transport stream when a parser rejects it.
//!
//! Salvo consumes its body inside `Limited`. On an error that wrapper drops
//! the receiver, which can reset HTTP/1 connections before the JSON response
//! reaches a still-sending client. Retain the original stream, then discard a
//! bounded remainder. No task, full-body buffer or second consumer is created.
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::BodyExt;
use salvo::http::body::{Body, Frame, ReqBody, SizeHint};
use salvo::prelude::Request;

struct State {
    body: ReqBody,
    consumed: usize,
    deadlines: Option<ReadDeadlines>,
    timed_out: bool,
}

struct ReadDeadlines {
    idle: Duration,
    idle_at: tokio::time::Instant,
    total_at: tokio::time::Instant,
    wake: Pin<Box<tokio::time::Sleep>>,
}

#[derive(Clone)]
pub(super) struct RetainedBody(Arc<Mutex<State>>);

impl RetainedBody {
    #[cfg(test)]
    pub fn install(request: &mut Request) -> Self {
        Self::install_with_timeouts(request, Duration::from_secs(30), Duration::from_secs(900))
    }

    pub fn install_with_timeouts(request: &mut Request, idle: Duration, total: Duration) -> Self {
        let now = tokio::time::Instant::now();
        let idle_at = now + idle;
        let total_at = now + total;
        let retained = Self(Arc::new(Mutex::new(State {
            body: request.take_body(),
            consumed: 0,
            deadlines: Some(ReadDeadlines {
                idle,
                idle_at,
                total_at,
                wake: Box::pin(tokio::time::sleep_until(idle_at.min(total_at))),
            }),
            timed_out: false,
        })));
        request.replace_body(ReqBody::Boxed {
            inner: Box::pin(retained.clone()),
            // The original body still owns its fusewire and emits its events.
            fusewire: None,
        });
        retained
    }

    pub fn timed_out(&self) -> bool {
        self.0.lock().is_ok_and(|state| state.timed_out)
    }

    pub async fn drain(&self, total_byte_budget: usize, deadline: Duration) -> bool {
        // A read deadline may be the reason parsing failed. The rejection has
        // its own short deadline, so it can still finish an already-sent body.
        match self.0.lock() {
            Ok(mut state) => state.deadlines = None,
            Err(_) => return false,
        }
        let mut reader = self.clone();
        tokio::time::timeout(deadline, async move {
            loop {
                let consumed = match reader.0.lock() {
                    Ok(state) => state.consumed,
                    Err(_) => return false,
                };
                if consumed > total_byte_budget {
                    return false;
                }
                match reader.frame().await {
                    Some(Ok(_)) => {}
                    Some(Err(_)) => return false,
                    None => return true,
                }
            }
        })
        .await
        .unwrap_or(false)
    }
}

pub(super) fn exceeds_limit(error: &salvo::http::ParseError) -> bool {
    if matches!(error, salvo::http::ParseError::PayloadTooLarge) {
        return true;
    }
    let mut cause = std::error::Error::source(error);
    while let Some(error) = cause {
        if error.is::<http_body_util::LengthLimitError>() {
            return true;
        }
        cause = error.source();
    }
    false
}

pub(super) fn close_if_incomplete(
    request: &Request,
    response: &mut salvo::Response,
    drained: bool,
) {
    if !drained
        && matches!(
            request.version(),
            salvo::http::Version::HTTP_10 | salvo::http::Version::HTTP_11
        )
    {
        response
            .headers_mut()
            .insert("connection", "close".parse().unwrap());
    }
}

impl Body for RetainedBody {
    type Data = Bytes;
    type Error = salvo::BoxedError;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Self::Error>>> {
        let Ok(mut state) = self.0.lock() else {
            return Poll::Ready(Some(Err(Box::new(std::io::Error::other(
                "Upload stream state unavailable",
            )))));
        };
        if let Some(deadlines) = &mut state.deadlines {
            let now = tokio::time::Instant::now();
            if now >= deadlines.idle_at
                || now >= deadlines.total_at
                || deadlines.wake.as_mut().poll(context).is_ready()
            {
                state.timed_out = true;
                return Poll::Ready(Some(Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "Request body idle or total read deadline exceeded",
                )))));
            }
        }
        // The lock only protects a synchronous poll; it is never held at await.
        match Pin::new(&mut state.body).poll_frame(context) {
            Poll::Ready(Some(Ok(frame))) => {
                if let Some(bytes) = frame.data_ref() {
                    state.consumed = state.consumed.saturating_add(bytes.len());
                    if !bytes.is_empty()
                        && let Some(deadlines) = &mut state.deadlines
                    {
                        deadlines.idle_at = tokio::time::Instant::now() + deadlines.idle;
                        let next = deadlines.idle_at.min(deadlines.total_at);
                        deadlines.wake.as_mut().reset(next);
                    }
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Ready(Some(Err(error))) => Poll::Ready(Some(Err(Box::new(error)))),
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.0.lock().is_ok_and(|state| state.body.is_end_stream())
    }

    fn size_hint(&self) -> SizeHint {
        self.0
            .lock()
            .map(|state| state.body.size_hint())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_body_util::{Limited, StreamBody};

    fn streamed_request(
        stream: impl futures_util::Stream<Item = Result<Frame<Bytes>, std::io::Error>>
        + Send
        + Sync
        + 'static,
    ) -> Request {
        let mut request = Request::new();
        request.replace_body(ReqBody::Boxed {
            inner: Box::pin(
                StreamBody::new(stream).map_err(|error| Box::new(error) as salvo::BoxedError),
            ),
            fusewire: None,
        });
        request
    }

    #[tokio::test]
    async fn parser_limit_does_not_discard_the_remaining_transport_stream() {
        let stream = futures_util::stream::iter(
            (0..4).map(|_| Ok(Frame::data(Bytes::from_static(b"data")))),
        );
        let mut request = streamed_request(stream);
        let retained = RetainedBody::install(&mut request);
        assert!(
            Limited::new(request.take_body(), 8)
                .collect()
                .await
                .is_err()
        );
        assert_eq!(retained.0.lock().unwrap().consumed, 12);
        assert!(retained.drain(16, Duration::from_secs(1)).await);
        assert_eq!(retained.0.lock().unwrap().consumed, 16);
    }

    #[tokio::test]
    async fn drain_budget_includes_bytes_already_consumed_by_the_parser() {
        let stream = futures_util::stream::iter(
            (0..100).map(|_| Ok(Frame::data(Bytes::from_static(b"data")))),
        );
        let mut request = streamed_request(stream);
        let retained = RetainedBody::install(&mut request);
        assert!(
            Limited::new(request.take_body(), 8)
                .collect()
                .await
                .is_err()
        );
        assert!(!retained.drain(16, Duration::from_secs(1)).await);
        // At most one incoming frame crosses the byte boundary. The other
        // 380 bytes are never polled, buffered or written to a temporary file.
        assert_eq!(retained.0.lock().unwrap().consumed, 20);
    }

    #[tokio::test]
    async fn a_stalled_body_cannot_keep_the_rejection_drain_alive() {
        let mut request = streamed_request(futures_util::stream::pending());
        let retained = RetainedBody::install(&mut request);
        assert!(!retained.drain(1024, Duration::from_millis(20)).await);
        assert_eq!(retained.0.lock().unwrap().consumed, 0);
    }

    #[tokio::test]
    async fn idle_read_deadline_is_a_visible_error_and_does_not_consume_the_stream() {
        let mut request = streamed_request(futures_util::stream::pending());
        let retained = RetainedBody::install_with_timeouts(
            &mut request,
            Duration::from_millis(20),
            Duration::from_secs(1),
        );
        let error = request.body_mut().frame().await.unwrap().unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Other);
        assert!(retained.timed_out());
        assert_eq!(retained.0.lock().unwrap().consumed, 0);
        assert!(!retained.drain(1024, Duration::from_millis(10)).await);
    }

    #[tokio::test]
    async fn making_progress_cannot_extend_the_total_read_deadline() {
        let stream = futures_util::stream::unfold((), |()| async {
            tokio::time::sleep(Duration::from_millis(5)).await;
            Some((Ok(Frame::data(Bytes::from_static(b"a"))), ()))
        });
        let mut request = streamed_request(stream);
        let retained = RetainedBody::install_with_timeouts(
            &mut request,
            Duration::from_secs(1),
            Duration::from_millis(30),
        );
        let result = request.take_body().collect().await;
        assert!(result.is_err());
        assert!(retained.timed_out());
        assert!(retained.0.lock().unwrap().consumed > 0);
    }
}
