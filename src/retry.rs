use crate::circuit_breaker::CircuitBreaker;
use crate::error::{JobError, JobResult};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, warn, error};
use rand::Rng;

/// Retry configuration for Redis operations
#[derive(Clone)]
pub struct RetryConfig {
    pub max_attempts: u32,
    pub initial_delay_ms: u64,
    pub max_delay_ms: u64,
    pub backoff_multiplier: f64,
    pub jitter_ms: u64,
    pub circuit_breaker_threshold: Option<u32>,
    pub circuit_breaker_timeout_secs: Option<u64>,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_attempts: 5,
            initial_delay_ms: 100,
            max_delay_ms: 10000,
            backoff_multiplier: 2.0,
            jitter_ms: 100,
            circuit_breaker_threshold: Some(10),
            circuit_breaker_timeout_secs: Some(30),
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

    pub fn with_jitter(mut self, jitter_ms: u64) -> Self {
        self.jitter_ms = jitter_ms;
        self
    }
}

thread_local! {
    static CIRCUIT_BREAKER: std::cell::RefCell<Option<Arc<CircuitBreaker>>> = const { std::cell::RefCell::new(None) };
}

/// Execute an async operation with retry logic, exponential backoff with jitter, and circuit breaker
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

    // Initialize circuit breaker if configured
    if let Some(threshold) = config.circuit_breaker_threshold {
        let timeout = Duration::from_secs(config.circuit_breaker_timeout_secs.unwrap_or(30));
        
        CIRCUIT_BREAKER.with(|breaker| {
            if breaker.borrow().is_none() {
                *breaker.borrow_mut() = Some(Arc::new(CircuitBreaker::new(threshold, timeout)));
            }
        });
    }

    for attempt in 1..=config.max_attempts {
        // Check circuit breaker before attempting
        if config.circuit_breaker_threshold.is_some() {
            CIRCUIT_BREAKER.with(|breaker| {
                if let Some(b) = breaker.borrow().as_ref() {
                    let allowed = tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current().block_on(b.is_request_allowed())
                    });
                    
                    if !allowed {
                        warn!("Circuit breaker is OPEN - blocking request");
                        return Err(JobError::QueueError("Circuit breaker is open".to_string()));
                    }
                }
                Ok(())
            })?;
        }

        match operation().await {
            Ok(result) => {
                // Record success in circuit breaker
                if config.circuit_breaker_threshold.is_some() {
                    CIRCUIT_BREAKER.with(|breaker| {
                        if let Some(b) = breaker.borrow().as_ref() {
                            tokio::task::block_in_place(|| {
                                tokio::runtime::Handle::current().block_on(b.record_success())
                            });
                        }
                    });
                }

                // Log success if we recovered from a failure
                if attempt > 1 {
                    if let Some(ref first_err) = first_error_msg {
                        info!(
                            "Redis reconnected successfully! (attempt {}/{} recovered from: {})",
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

                // Record failure in circuit breaker
                if is_connection_error
                    && config.circuit_breaker_threshold.is_some() {
                        CIRCUIT_BREAKER.with(|breaker| {
                            if let Some(b) = breaker.borrow().as_ref() {
                                tokio::task::block_in_place(|| {
                                    tokio::runtime::Handle::current().block_on(b.record_failure())
                                });
                            }
                        });
                    }

                if is_connection_error && attempt < config.max_attempts {
                    // Add jitter to prevent thundering herd
                    let jitter = if config.jitter_ms > 0 {
                        let mut rng = rand::thread_rng();
                        rng.gen_range(0..config.jitter_ms)
                    } else {
                        0
                    };

                    warn!(
                        "Redis operation failed (attempt {}/{}): {}. Retrying in {:?}+{:?}ms...",
                        attempt,
                        config.max_attempts,
                        err,
                        current_delay,
                        jitter
                    );

                    sleep(current_delay).await;
                    
                    // Sleep for jitter if set
                    if jitter > 0 {
                        sleep(Duration::from_millis(jitter)).await;
                    }

                    // Exponential backoff
                    current_delay = std::cmp::min(
                        Duration::from_millis(config.max_delay_ms),
                        Duration::from_millis((current_delay.as_millis() as f64 * config.backoff_multiplier) as u64),
                    );
                } else {
                    error!(
                        "Redis operation failed after {} attempts: {}",
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
