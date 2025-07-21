use std::{marker::PhantomData, pin::Pin, sync::LazyLock};
use log::debug;
pub use reqwest::{Client, Method, Request, Url};
use serde::de::DeserializeOwned;
use tower::{Layer, Service};

/// Errors that occur can in scope of a downloader.
#[derive(Debug, thiserror::Error)]
pub enum DownloaderError<E> {
    /// An error occured in the HTTP client
    #[error("HTTP client error: {0}")]
    HttpClient(reqwest::Error),

    // An error occured while deserializing content
    #[error("Deserialization error: {0}")]
    Deserialization(serde_json::Error),

    // An error occured while polling the inner service for readiness
    #[error("Error polling inner service: {0}")]
    InnerPoll(E),

    // An error occured while calling the inner service
    #[error("Error calling inner service: {0}")]
    InnerCall(E),
}

/// A [`tower::Layer`] that provides convenience by downloading a page given a [`reqwest::Request`] object
/// and passing a [`String`] to your processing function.
/// 
/// Typically inserted right before your service:
/// ```rust
/// let svc = tower::ServiceBuilder::new()
///     // other layers like throttling, retries
///     .layer(BodyDownloaderLayer)
///     .service_fn(processing_fn);
/// ```
pub struct BodyDownloaderLayer;

impl<S> Layer<S> for BodyDownloaderLayer
where S: Service<String> {
    type Service = BodyDownloader<S>;

    fn layer(&self, inner: S) -> Self::Service {
        BodyDownloader::new(inner)
    }
}

/// This [`tower::Service`] wraps an inner service, downloads a page from a given [`reqwest::Request`]
/// and passes the result [`String`] to the inner service.
/// 
/// Recommended to inject using [`BodyDownloaderLayer`] but you can also use it directly with `layer_fn`:
/// ```rust
/// let svc = tower::ServiceBuilder::new()
///     // other layers like throttling, retries
///     .layer_fn(BodyDownloader::new)
///     .service_fn(processing_fn);
/// ```
pub struct BodyDownloader<S>
where
    S: Service<String> {
    client: Client,
    inner: S
}

// TODO: Provide a hatch to modifying the properties (probably per-website), like headers, user agents.
const HTTP_CLIENT: LazyLock<Client> = LazyLock::new(|| Client::new());

impl<S> BodyDownloader<S>
where
    S: Service<String> {
    pub fn new(inner: S) -> Self {
        Self {
            client: HTTP_CLIENT.clone(),
            inner,
        }
    }
}

impl<S> Service<Request> for BodyDownloader<S>
where
    S: Service<String> + Send + Clone + 'static,
    <S as Service<String>>::Future: Send,
    <S as Service<String>>::Error: Send {
    type Response = ();
    type Error = DownloaderError<S::Error>;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;
    
    fn poll_ready(&mut self, cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
            .map_err(|e| DownloaderError::InnerPoll(e))
    }

    fn call(&mut self, request: Request) -> Self::Future {
        let client = self.client.clone();
        
        // A switcheroo recommended by `tower`:
        // https://docs.rs/tower/latest/tower/trait.Service.html#be-careful-when-cloning-inner-services
        // Clone the inner service...
        let clone = self.inner.clone();
        // ..but take the service that was ready
        let mut inner = std::mem::replace(&mut self.inner, clone);

        Box::pin(async move {
            debug!("BodyDownloader received {} request for {}", request.method(), request.url());
            
            let text = client.execute(request).await
                .map_err(|e| DownloaderError::HttpClient(e))?
                .text().await
                .map_err(|e| DownloaderError::HttpClient(e))?;
            
            inner.call(text).await
                .map_err(|e| DownloaderError::InnerCall(e))?;
            
            Ok(())
        })
    }
}

/// A [`tower::Layer`] that provides convenience by downloading a JSON resource given a [`reqwest::Request`] object
/// and deserializing it via [`serde`] to a concrete type.
/// 
/// Typically inserted right before your service:
/// ```rust
/// let svc = tower::ServiceBuilder::new()
///     // other layers like throttling, retries
///     .layer(JsonDownloaderLayer::<Foo>)
///     .service_fn(processing_fn);
/// ```
pub struct JsonDownloaderLayer<T> {
    _t: PhantomData<T>,
}

impl<T> JsonDownloaderLayer<T> {
    pub fn new() -> Self {
        Self { _t: PhantomData::default(), }
    }
}

impl<S, T> Layer<S> for JsonDownloaderLayer<T>
where
    S: Service<T>,
    T: DeserializeOwned {
    type Service = JsonDownloader<S, T>;

    fn layer(&self, inner: S) -> Self::Service {
        JsonDownloader::new(inner)
    }
}

/// This [`tower::Service`] wraps an inner service, downloads a page from a given [`reqwest::Request`]
/// and deserializes the result into a concrete type before passing it to the inner service.
/// 
/// Recommended to inject using [`JsonDownloaderLayer`] but you can also use it directly with `layer_fn`:
/// ```rust
/// let svc = tower::ServiceBuilder::new()
///     // other layers like throttling, retries
///     .layer_fn(JsonDownloader::new)
///     .service_fn(processing_fn);
/// ```
pub struct JsonDownloader<S, T>
where
    S: Service<T>,
    T: DeserializeOwned {
    client: Client,
    inner: S,
    _t: PhantomData<T>,
}

impl<S, T> JsonDownloader<S, T>
where
    S: Service<T>,
    T: DeserializeOwned {
    pub fn new(inner: S) -> Self {
        Self {
            client: HTTP_CLIENT.clone(),
            inner,
            _t: PhantomData::default(),
        }
    }
}

impl<S, T> Service<Request> for JsonDownloader<S, T>
where
    S: Service<T> + Send + Clone + 'static,
    <S as Service<T>>::Future: Send,
    <S as Service<T>>::Error: Send,
    T: DeserializeOwned {
    type Response = ();
    type Error = DownloaderError<S::Error>;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;
    
    fn poll_ready(&mut self, cx: &mut std::task::Context<'_>) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
            .map_err(|e| DownloaderError::InnerPoll(e))
    }

    fn call(&mut self, request: Request) -> Self::Future {
        let client = self.client.clone();
        
        // A switcheroo recommended by `tower`:
        // https://docs.rs/tower/latest/tower/trait.Service.html#be-careful-when-cloning-inner-services
        // Clone the inner service...
        let clone = self.inner.clone();
        // ..but take the service that was ready
        let mut inner = std::mem::replace(&mut self.inner, clone);

        Box::pin(async move {
            debug!("JsonDownloader received {} request for {}", request.method(), request.url());
            
            let text = client.execute(request).await
                .map_err(|e| DownloaderError::HttpClient(e))?
                .text().await
                .map_err(|e| DownloaderError::HttpClient(e))?;

            let obj = serde_json::from_str(&text)
                .map_err(|e| DownloaderError::Deserialization(e))?;
            
            inner.call(obj).await
                .map_err(|e| DownloaderError::InnerCall(e))?;
            
            Ok(())
        })
    }
}

pub fn string_to_get_reqwest(url: String) -> Result<reqwest::Request, String> {
    let url = Url::parse(&url)
        .map_err(|e| format!("Error ({}) when parsing the following url: {}", e, url))?;
    
    Ok(Request::new(Method::GET, url))
}