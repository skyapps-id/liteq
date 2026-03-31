use thiserror::Error;

pub type JobResult<T> = Result<T, JobError>;

#[derive(Debug, Error)]
pub enum JobError {
    #[error("Redis connection error: {0}")]
    RedisConnection(#[from] redis::RedisError),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Queue error: {0}")]
    QueueError(String),

    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("Task join error: {0}")]
    TaskJoinError(#[from] tokio::task::JoinError),

    #[error("Pool exhausted: {0}")]
    PoolExhausted(String),
}
