use std::sync::{atomic::AtomicUsize, Arc, LazyLock};

use log::{debug, trace, warn};
use tokio::sync::{mpsc, watch};
use tower::{Service, ServiceExt};

use crate::select::{Either, Select};

/// Errors that can occur while processing the work pipe.
#[derive(Debug, thiserror::Error)]
pub enum WorkPipeError<E> {
    #[error("Service not ready: {0}")]
    ServiceNotReady(E),
    
    #[error("Service call failed: {0}")]
    ServiceCall(E),

    #[error("Failed to receive shutdown signal: {0}")]
    ShutdownReceive(watch::error::RecvError),
    
    #[error("Failed to send shutdown signal: {0}")]
    ShutdownSend(watch::error::SendError<bool>),
}

/// Represents the input of a work pipe.
/// Receives work units and sends them to the worker.
pub struct WorkPipe<T> {
    tx: mpsc::Sender<T>,
    work_counter: Arc<AtomicUsize>,
}

impl<T> WorkPipe<T> {
    /// Submit a unit of work into the pipe for processing by the worker.
    /// Will block asynchronously if the work pipe buffer is full.
    pub async fn submit_work(&self, work: T) -> Result<(), mpsc::error::SendError<T>> {
        let prev = self.work_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        debug!("work_counter: {} -> {}", prev, prev + 1);
        
        self.tx.send(work).await?;
        
        Ok(())
    }
}

impl<T> Clone for WorkPipe<T> {
    fn clone(&self) -> Self {
        Self { tx: self.tx.clone(), work_counter: self.work_counter.clone() }
    }
}

/// Holds the receiving end of a work pipe and processes them as they come.
/// The actual processing service is user-provided.
pub struct Worker<T> {
    rx: mpsc::Receiver<T>,
    work_counter: Arc<AtomicUsize>,
    shutdown_rx: watch::Receiver<bool>,
    shutdown_tx: watch::Sender<bool>,
}

impl<T> Worker<T> {
    /// Consume the worker and begin work, calling the given service for each work item.
    /// Exits when all work on this pipe is done.
    /// Typically spawned as a separate thread/task by your runtime (e.g. tokio).
    pub async fn work<S>(mut self, mut service: S) -> Result<(), WorkPipeError<S::Error>>
    where
        S: Service<T> + Send,
        S::Error: std::fmt::Display + Send + Sync + 'static,
        S::Future: Send,
    {
        loop {
            let fut = Select::new(
                self.shutdown_rx.changed(),
                self.rx.recv()).await;
            
            match fut {
                Either::Left(Ok(_)) => {
                    if *self.shutdown_rx.borrow() {
                        trace!("Shutdown signal received");
                        break;
                    }
                },
                Either::Left(Err(why)) => return Err(WorkPipeError::ShutdownReceive(why)),
                Either::Right(Some(work)) => {
                    trace!("Received work");
                    let result = service.ready().await
                        .map_err(|e| WorkPipeError::ServiceNotReady(e))?
                        .call(work).await;

                    if let Err(why) = result {
                        warn!("Error while processing work: {why}");
                    }
                    
                    self.work_completed::<S>().await?;
                },
                Either::Right(None) => {
                    trace!("Channel closed");
                    break;
                },

            }
        }

        Ok(())
    }

    /// Signal work completed on current item.
    async fn work_completed<S>(&self) -> Result<(), WorkPipeError<S::Error>>
    where
        S: Service<T> + Send {
        let prev = self.work_counter.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        debug!("work_counter: {} -> {}", prev, prev - 1);

        if prev == 1 {
            self.shutdown_tx.send(true)
                .map_err(|e| WorkPipeError::ShutdownSend(e))?;
        }

        Ok(())
    }

}

/// A builder for WorkPipe objects.
/// Ultimately produces a pair of ([`WorkPipe`], [`Worker`]) which is a thin wrapper around [`tokio::sync::mpsc::channel`] with a work counter.
pub struct WorkPipeBuilder {
    buffer_size: usize,
    work_counter: Arc<AtomicUsize>,
    shutdown_rx: watch::Receiver<bool>,
    shutdown_tx: watch::Sender<bool>,
}

static DEFAULT_BUFFER_SIZE: usize = 100;
static GLOBAL_COUNTER: LazyLock<Arc<AtomicUsize>> = LazyLock::new(|| Arc::new(AtomicUsize::new(0)));
static GLOBAL_SHUTDOWN: LazyLock<(watch::Sender<bool>, watch::Receiver<bool>)> = LazyLock::new(|| watch::channel(false));

impl WorkPipeBuilder {
    /// Create a new builder with default settings:
    /// - buffer size of 100
    /// - a global shared work counter
    /// - a global shared shutdown signal
    pub fn new() -> Self {
        Self {
            buffer_size: DEFAULT_BUFFER_SIZE,
            work_counter: GLOBAL_COUNTER.clone(),
            shutdown_rx: GLOBAL_SHUTDOWN.1.clone(),
            shutdown_tx: GLOBAL_SHUTDOWN.0.clone(),
        }
    }

    /// Change the buffer size for the pipe.
    pub fn with_buffer_size(mut self, buffer_size: usize) -> Self {
        assert!(buffer_size > 0, "Buffer size must be positive!");
        self.buffer_size = buffer_size;
        self
    }

    /// Provide an external work counter to be used for this pipe.
    /// You should not normally have to change this from the default (global) counter.
    pub fn with_work_counter(mut self, counter: Arc<AtomicUsize>) -> Self {
        self.work_counter = counter;
        self
    }

    /// Provide an external shutdown [`tokio::sync::watch::channel`] for this pipe.
    /// Should almost always be associated with a counter (see [`Self::with_work_counter`])
    /// of the same lifetime and granularity.
    pub fn with_shutdown_channel(mut self, rx: watch::Receiver<bool>, tx: watch::Sender<bool>) -> Self {
        self.shutdown_rx = rx;
        self.shutdown_tx = tx;
        self
    }

    /// Consume the builder and produce a pair of ([`WorkPipe`], [`Worker`]).
    pub fn build<T>(self) -> (WorkPipe<T>, Worker<T>) {
        let (tx, rx) = mpsc::channel(self.buffer_size);
        
        let pipe = WorkPipe {
            tx,
            work_counter: self.work_counter.clone()
        };

        let worker = Worker {
            rx,
            work_counter: self.work_counter,
            shutdown_rx: self.shutdown_rx,
            shutdown_tx: self.shutdown_tx,
        };

        (pipe, worker)
    }
}

impl Default for WorkPipeBuilder {
    fn default() -> Self {
        Self::new()
    }
}