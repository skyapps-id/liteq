use thiserror::Error;

pub type JobResult<T> = Result<T, JobError>;

#[derive(Debug, Error)]
pub enum JobError {
    #[error("Redis connection error: {0}")]
    RedisConnection(#[from] redis::RedisError),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Job not found: {0}")]
    JobNotFound(String),

    #[error("Queue error: {0}")]
    QueueError(String),

    #[error("Worker error: {0}")]
    WorkerError(String),

    #[error("Handler not found for job: {0}")]
    HandlerNotFound(String),

    #[error("Job processing failed: {0}")]
    ProcessingFailed(String),

    #[error("Maximum retries exceeded")]
    MaxRetriesExceeded,

    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),
}
