use std::{marker::PhantomData, pin::Pin, sync::Arc};

use leaky_bucket::RateLimiter;
use tower::{Layer, Service};

pub struct LeakyBucketRateLimiterLayer<T> {
    limiter: Arc<RateLimiter>,
    _t: PhantomData<T>,
}

impl<T> LeakyBucketRateLimiterLayer<T> {
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

pub struct LeakyBucketRateLimiter<S> {
    inner: S,
    limiter: Arc<RateLimiter>,
}

impl<S, R> Service<R> for LeakyBucketRateLimiter<S>
where
    S: Service<R> + Send + 'static,
    <S as Service<R>>::Response: Default,
    <S as Service<R>>::Error: std::error::Error + Send + Sync,
    <S as Service<R>>::Future: Send,
    R: Send + 'static {
    type Response = S::Response;
    type Error = Box<dyn std::error::Error + Send + Sync>;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;
    
    fn poll_ready(&mut self, cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx).map_err(Into::into)
    }

    fn call(&mut self, request: R) -> Self::Future {
        let limiter = self.limiter.clone();
        let fut = self.inner.call(request);
        
        Box::pin(async move {
            limiter.acquire_one().await;
            
            fut.await.map_err(Into::<Self::Error>::into)
        })
    }
}