use crate::error::{JobError, JobResult};
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, warn, error};

/// Retry configuration for Redis operations
#[derive(Clone)]
pub struct RetryConfig {
    pub max_attempts: u32,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub backoff_multiplier: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            initial_delay_ms: 100,
            max_delay_ms: 10000,
            backoff_multiplier: 2.0,
        }
    }
}

impl RetryConfig {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_attempts(mut self, attempts: u32) -> Self {
        self.max_attempts = attempts;
        self
    }

    pub fn with_initial_delay(mut self, delay_ms: u64) -> Self {
        self.initial_delay_ms = delay_ms;
        self
    }

    pub fn with_max_delay(mut self, delay_ms: u64) -> Self {
        self.max_delay_ms = delay_ms;
        self
    }
}

/// Execute an async operation with retry logic and exponential backoff
pub async fn retry_async<F, Fut, T>(
    operation: F,
    config: Option<RetryConfig>,
) -> JobResult<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = JobResult<T>>,
{
    let config = config.unwrap_or_default();
    let mut current_delay = Duration::from_millis(config.initial_delay_ms);
    let mut first_error_msg: Option<String> = None;

    for attempt in 1..=config.max_attempts {
        match operation().await {
            Ok(result) => {
                // Log success if we recovered from a failure
                if attempt > 1 {
                    if let Some(ref first_err) = first_error_msg {
                        info!(
                            "✅ Redis reconnected successfully! (attempt {}/{} recovered from: {})",
                            attempt,
                            config.max_attempts,
                            first_err
                        );
                    }
                }
                return Ok(result);
            }
            Err(err) => {
                // Store first error message for reporting
                if first_error_msg.is_none() {
                    first_error_msg = Some(err.to_string());
                }

                // Check if it's a connection error (retryable)
                let is_connection_error = is_retryable_error(&err);

                if is_connection_error && attempt < config.max_attempts {
                    warn!(
                        "⚠️  Redis operation failed (attempt {}/{}): {}. Retrying in {:?}...",
                        attempt,
                        config.max_attempts,
                        err,
                        current_delay
                    );

                    sleep(current_delay).await;

                    // Exponential backoff
                    current_delay = std::cmp::min(
                        Duration::from_millis(config.max_delay_ms),
                        Duration::from_millis((current_delay.as_millis() as f64 * config.backoff_multiplier) as u64),
                    );
                } else {
                    error!(
                        "❌ Redis operation failed after {} attempts: {}",
                        attempt, err
                    );
                    return Err(err);
                }
            }
        }
    }

    Err(JobError::QueueError(
        "Maximum retry attempts exceeded".to_string(),
    ))
}

/// Check if an error is retryable (connection-related)
fn is_retryable_error(err: &JobError) -> bool {
    match err {
        JobError::RedisConnection(redis_err) => {
            let err_str = redis_err.to_string().to_lowercase();
            err_str.contains("broken pipe")
                || err_str.contains("connection refused")
                || err_str.contains("connection reset")
                || err_str.contains("timeout")
                || err_str.contains("io error")
                || err_str.contains("multiplexed")
                || err_str.contains("unexpectedly terminated")
                || err_str.contains("driver")
                || err_str.contains("terminated")
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_config_default() {
        let config = RetryConfig::default();
        assert_eq!(config.max_attempts, 5);
        assert_eq!(config.initial_delay_ms, 100);
        assert_eq!(config.max_delay_ms, 10000);
    }

    #[test]
    fn test_retry_config_builder() {
        let config = RetryConfig::new()
            .with_max_attempts(10)
            .with_initial_delay(200);

        assert_eq!(config.max_attempts, 10);
        assert_eq!(config.initial_delay_ms, 200);
    }

    #[test]
    fn test_is_retryable_error_broken_pipe() {
        let err = JobError::RedisConnection(
            redis::RedisError::from(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "broken pipe"))
        );
        assert!(is_retryable_error(&err));
    }

    #[test]
    fn test_is_retryable_error_multiplexed() {
        let err = JobError::RedisConnection(
            redis::RedisError::from(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "Multiplexed connection driver unexpectedly terminated"))
        );
        assert!(is_retryable_error(&err));
    }
}
