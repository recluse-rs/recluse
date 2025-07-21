use std::{pin::Pin, task::{Context, Poll}};

/// Generic select implementation that works with any two futures.
/// 
/// Usage:
/// 
/// ```rust
/// let either_fut = Select::new(fut1, fut2).await;
/// 
/// match either_fut {
///     Either::Left(value) => { /**/ },
///     Either::Right(value) => { /**/ },
/// }
/// ```
pub struct Select<F1, F2> {
    future1: Pin<Box<F1>>,
    future2: Pin<Box<F2>>,
}

impl<F1, F2> Select<F1, F2>
where
    F1: Future,
    F2: Future,
{
    pub fn new(future1: F1, future2: F2) -> Self {
        Self {
            future1: Box::pin(future1),
            future2: Box::pin(future2),
        }
    }
}

#[derive(Debug)]
pub enum Either<A, B> {
    Left(A),
    Right(B),
}

impl<F1, F2> Future for Select<F1, F2>
where
    F1: Future,
    F2: Future,
{
    type Output = Either<F1::Output, F2::Output>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Try polling the first future
        if let Poll::Ready(output) = self.future1.as_mut().poll(cx) {
            return Poll::Ready(Either::Left(output));
        }

        // Try polling the second future
        if let Poll::Ready(output) = self.future2.as_mut().poll(cx) {
            return Poll::Ready(Either::Right(output));
        }

        // Neither future is ready
        Poll::Pending
    }
}