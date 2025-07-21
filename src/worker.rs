use std::sync::{atomic::AtomicUsize, Arc, LazyLock};

use tokio::sync::mpsc;
use tower::{Service, ServiceExt};

/// A single message in the work pipe.
#[derive(Clone)]
pub enum WorkMessage<T> {
    /// A unit of work. Only provided externally.
    Work(T),

    /// A signal to shutdown the worker. Only emitted internally.
    Shutdown,
}

/// Errors that can occur while processing the work pipe.
#[derive(Debug, thiserror::Error)]
pub enum WorkPipeError<T, E> {
    #[error("Service not ready: {0}")]
    ServiceNotReady(E),
    
    #[error("Service call failed: {0}")]
    ServiceCall(E),
    
    #[error("Failed to send shutdown signal")]
    ShutdownSend(mpsc::error::SendError<WorkMessage<T>>),
}

/// Represents the input of a work pipe.
/// Receives work units and sends them to the worker.
pub struct WorkPipe<T> {
    tx: mpsc::Sender<WorkMessage<T>>,
    work_counter: Arc<AtomicUsize>,
}

impl<T> WorkPipe<T> {
    /// Submit a unit of work into the pipe for processing by the worker.
    /// Will block asynchronously if the work pipe buffer is full.
    pub async fn submit_work(&self, work: T) -> Result<(), mpsc::error::SendError<WorkMessage<T>>> {
        self.work_counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.tx.send(WorkMessage::Work(work)).await?;
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
    tx: mpsc::Sender<WorkMessage<T>>,
    rx: mpsc::Receiver<WorkMessage<T>>,
    work_counter: Arc<AtomicUsize>,
}

impl<T> Worker<T> {
    /// Consume the worker and begin work, calling the given service for each work item.
    /// Exits when all work on this pipe is done.
    /// Typically spawned as a separate thread/task by your runtime (e.g. tokio).
    pub async fn work<S>(mut self, mut service: S) -> Result<(), WorkPipeError<T, S::Error>>
    where
        S: Service<T> + Send,
        S::Error: std::fmt::Display + Send + Sync + 'static,
        S::Future: Send,
    {
        loop {
            match self.rx.recv().await {
                Some(WorkMessage::Work(work)) => {
                    service.ready().await
                        .map_err(|e| WorkPipeError::ServiceNotReady(e))?
                        .call(work).await
                        .map_err(|e| WorkPipeError::ServiceCall(e))?;
                    
                    self.work_completed().await
                        .map_err(|e| WorkPipeError::ShutdownSend(e))?;
                },
                Some(WorkMessage::Shutdown) => break,
                None => break,
            }
        }

        Ok(())
    }

    /// Signal work completed on current item.
    async fn work_completed(&self) -> Result<(), mpsc::error::SendError<WorkMessage<T>>> {
        let prev_count = self.work_counter.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);

        if prev_count == 1 {
            self.tx.send(WorkMessage::Shutdown).await?;
        }

        Ok(())
    }

}

/// A builder for WorkPipe objects.
/// Ultimately produces a pair of ([`WorkPipe`], [`Worker`]) which is a thin wrapper around [`tokio::sync::mpsc::channel`] with a work counter.
pub struct WorkPipeBuilder {
    buffer_size: usize,
    work_counter: Arc<AtomicUsize>,
}

static DEFAULT_BUFFER_SIZE: usize = 100;
static GLOBAL_COUNTER: LazyLock<Arc<AtomicUsize>> = LazyLock::new(|| Arc::new(AtomicUsize::new(0)));

impl WorkPipeBuilder {
    /// Create a new builder with default settings:
    /// - buffer size of 100
    /// - a global shared work counter
    pub fn new() -> Self {
        Self {
            buffer_size: DEFAULT_BUFFER_SIZE,
            work_counter: GLOBAL_COUNTER.clone(),
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

    /// Consume the builder and produce a pair of ([`WorkPipe`], [`Worker`]).
    pub fn build<T>(self) -> (WorkPipe<T>, Worker<T>) {
        let (tx, rx) = mpsc::channel(self.buffer_size);
        
        let pipe = WorkPipe {
            tx: tx.clone(),
            work_counter: self.work_counter.clone()
        };

        let worker = Worker {
            tx,
            rx,
            work_counter: self.work_counter
        };

        (pipe, worker)
    }
}

impl Default for WorkPipeBuilder {
    fn default() -> Self {
        Self::new()
    }
}