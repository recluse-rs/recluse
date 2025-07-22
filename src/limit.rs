use std::{marker::PhantomData, pin::Pin, sync::Arc, future::Future};

use leaky_bucket::RateLimiter;
use tower::{Layer, Service};

/// Errors that can occur inside the limiter.
#[derive(Debug, thiserror::Error)]
pub enum LeakyBucketRateLimiterError<E> {
    // An error occured while polling the inner service for readiness
    #[error("Error polling inner service: {0}")]
    InnerPoll(E),

    // An error occured while calling the inner service
    #[error("Error calling inner service: {0}")]
    InnerCall(E),
}

/// A [`tower::Layer`] that implements the "[leaky bucket](https://en.wikipedia.org/wiki/Leaky_bucket)" algorithm
/// (via [`leaky_bucket`] crate) as a rate limiter.
/// 
/// Multiple instances of this layer can share a single limiter:
/// ```rust
/// let limiter = Arc::new(leaky_bucket::RateLimiter::builder()
///     /* set limiter arguments */
///     .build());
/// 
/// let svc1 = tower::ServiceBuilder::new()
///     .layer(LeakyBucketRateLimiterLayer::new(limiter.clone()))
///     /* rest of the tower */
/// 
/// let svc2 = tower::ServiceBuilder::new()
///     .layer(LeakyBucketRateLimiterLayer::new(limiter))
///     /* rest of the tower */
/// ```
pub struct LeakyBucketRateLimiterLayer<T> {
    limiter: Arc<RateLimiter>,
    _t: PhantomData<T>,
}

impl<T> LeakyBucketRateLimiterLayer<T> {
    /// Create a new instance of the layer that will use the given rate limiter (which can be shared).
    pub fn new(limiter: Arc<RateLimiter>) -> Self {
        Self {
            limiter,
            _t: PhantomData::default(),
        }
    }
}

impl<S, T> Layer<S> for LeakyBucketRateLimiterLayer<T>
where S: Service<T> {
    type Service = LeakyBucketRateLimiter<S>;
    
    fn layer(&self, inner: S) -> Self::Service {
        LeakyBucketRateLimiter {
            inner,
            limiter: self.limiter.clone(),
        }
    }
}

/// A [`tower::Service`] that implements the "[leaky bucket](https://en.wikipedia.org/wiki/Leaky_bucket)" algorithm
/// (via [`leaky_bucket`] crate) as a rate limiter.
pub struct LeakyBucketRateLimiter<S> {
    inner: S,
    limiter: Arc<RateLimiter>,
}

impl<S, R> Service<R> for LeakyBucketRateLimiter<S>
where
    S: Service<R> + Send + 'static,
    <S as Service<R>>::Response: Default,
    <S as Service<R>>::Error: Send,
    <S as Service<R>>::Future: Send,
    R: Send + 'static {
    type Response = S::Response;
    type Error = LeakyBucketRateLimiterError<S::Error>;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;
    
    fn poll_ready(&mut self, cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
            .map_err(LeakyBucketRateLimiterError::InnerPoll)
    }

    fn call(&mut self, request: R) -> Self::Future {
        let limiter = self.limiter.clone();
        let fut = self.inner.call(request);
        
        Box::pin(async move {
            limiter.acquire_one().await;
            
            fut.await
                .map_err(LeakyBucketRateLimiterError::InnerCall)
        })
    }
}