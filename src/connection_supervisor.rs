use crate::config::RedisConfig;
use crate::error::{JobError, JobResult};
use crate::retry::RetryConfig;
use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use redis::cmd;
use redis::Client;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Notify, RwLock};
use tokio::time::sleep;
use tracing::{error, info, warn};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConnectionState {
    Connected,
    Disconnected,
}

pub struct ConnectionSupervisor {
    state: Arc<RwLock<ConnectionState>>,
    ready_notify: Arc<Notify>,
    config: RedisConfig,
    retry_config: RetryConfig,
    check_interval: Duration,
}

impl ConnectionSupervisor {
    pub fn new(config: RedisConfig) -> Self {
        Self {
            state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
            ready_notify: Arc::new(Notify::new()),
            config: config.clone(),
            retry_config: RetryConfig::new()
                .with_max_attempts(20)
                .with_initial_delay(500)
                .with_max_delay(30000)
                .with_jitter(100),
            check_interval: Duration::from_secs(5),
        }
    }

    pub async fn start(&self) -> JobResult<()> {
        let state = self.state.clone();
        let ready_notify = self.ready_notify.clone();
        let config = self.config.clone();
        let retry_config = self.retry_config.clone();
        let check_interval = self.check_interval;

        tokio::spawn(async move {
            Self::supervision_loop(
                state,
                ready_notify,
                config,
                retry_config,
                check_interval,
            ).await;
        });

        info!("Connection supervision started");
        Ok(())
    }

    pub async fn wait_ready(&self) -> JobResult<()> {
        loop {
            let state = self.state.read().await;
            if *state == ConnectionState::Connected {
                return Ok(());
            }
            drop(state);

            self.ready_notify.notified().await;
        }
    }

    pub async fn get_connection(&self) -> JobResult<ConnectionManager> {
        self.wait_ready().await?;

        let client = Client::open(self.config.url.as_str())
            .map_err(|e| JobError::InvalidConfig(format!("Failed to create Redis client: {}", e)))?;

        let manager_config = ConnectionManagerConfig::new()
            .set_connection_timeout(Some(Duration::from_secs(self.config.connection_timeout_secs)))
            .set_response_timeout(Some(Duration::from_secs(self.config.response_timeout_secs)))
            .set_number_of_retries(20)
            .set_min_delay(Duration::from_secs(1))
            .set_max_delay(Duration::from_secs(60));

        ConnectionManager::new_with_config(client, manager_config)
            .await
            .map_err(JobError::from)
    }

    async fn supervision_loop(
        state: Arc<RwLock<ConnectionState>>,
        ready_notify: Arc<Notify>,
        config: RedisConfig,
        retry_config: RetryConfig,
        check_interval: Duration,
    ) {
        let mut consecutive_failures = 0u32;
        let mut backoff_duration = Duration::from_secs(1);

        loop {
            let current_state = *state.read().await;

            match Self::test_connection(&config).await {
                Ok(_) => {
                    if current_state != ConnectionState::Connected {
                        info!("Redis connected - notifying workers");
                        *state.write().await = ConnectionState::Connected;
                        ready_notify.notify_waiters();
                    }
                    // Reset counters on success
                    consecutive_failures = 0;
                    backoff_duration = Duration::from_secs(1);
                }
                Err(_e) => {
                    consecutive_failures += 1;

                    if current_state != ConnectionState::Disconnected {
                        warn!("Redis disconnected - starting reconnection");
                        *state.write().await = ConnectionState::Disconnected;
                    }

                    // Apply exponential backoff after 3 failures
                    if consecutive_failures > 3 {
                        warn!(
                            "Multiple connection failures ({}) - backing off for {:?} before retry",
                            consecutive_failures, backoff_duration
                        );
                        sleep(backoff_duration).await;
                        // Increase backof for next time (exponential, max 60s)
                        backoff_duration = std::cmp::min(backoff_duration * 2, Duration::from_secs(60));
                    }

                    // Try reconnection with configured retry logic
                    match Self::attempt_reconnect(&config, &retry_config).await {
                        Ok(_) => {
                            info!(
                                "Reconnection successful after {} failures",
                                consecutive_failures
                            );
                            *state.write().await = ConnectionState::Connected;
                            ready_notify.notify_waiters();
                            consecutive_failures = 0;
                            backoff_duration = Duration::from_secs(1);
                        }
                        Err(reconnect_err) => {
                            error!(
                                "Reconnection attempt {} failed: {}",
                                consecutive_failures, reconnect_err
                            );
                        }
                    }
                }
            }

            sleep(check_interval).await;
        }
    }

    async fn test_connection(config: &RedisConfig) -> JobResult<()> {
        let client = redis::Client::open(config.url.clone())?;

        let manager_config = ConnectionManagerConfig::new()
            .set_connection_timeout(Some(Duration::from_secs(config.connection_timeout_secs)))
            .set_response_timeout(Some(Duration::from_secs(config.response_timeout_secs)))
            .set_number_of_retries(5)
            .set_min_delay(Duration::from_millis(500))
            .set_max_delay(Duration::from_secs(5));

        let mut conn_manager = ConnectionManager::new_with_config(client, manager_config)
            .await
            .map_err(JobError::from)?;

        cmd("PING")
            .query_async::<String>(&mut conn_manager)
            .await
            .map_err(JobError::from)?;
        Ok(())
    }

    async fn attempt_reconnect(config: &RedisConfig, retry_config: &RetryConfig) -> JobResult<()> {
        let max_attempts = 3; // Limit attempts since supervision_loop will keep retrying
        let initial_delay = Duration::from_millis(retry_config.initial_delay_ms);
        let mut current_delay = initial_delay;

        for attempt in 1..=max_attempts {
            match Self::test_connection(config).await {
                Ok(_) => {
                    info!("Reconnection successful on attempt {}", attempt);
                    return Ok(());
                }
                Err(e) => {
                    if attempt < max_attempts {
                        warn!(
                            "Reconnect attempt {}/{} failed: {}. Retrying in {:?}...",
                            attempt, max_attempts, e, current_delay
                        );
                        sleep(current_delay).await;

                        // Exponential backoff
                        current_delay = std::cmp::min(
                            Duration::from_millis(retry_config.max_delay_ms),
                            Duration::from_millis(
                                (current_delay.as_millis() as f64 * retry_config.backoff_multiplier)
                                    as u64
                            ),
                        );
                    } else {
                        error!("Reconnection failed after {} attempts: {}", attempt, e);
                        return Err(e);
                    }
                }
            }
        }

        Err(JobError::QueueError(
            "Reconnection exceeded max attempts".to_string(),
        ))
    }

    pub async fn state(&self) -> ConnectionState {
        *self.state.read().await
    }

    pub fn with_retry_config(mut self, config: RetryConfig) -> Self {
        self.retry_config = config;
        self
    }

    pub fn with_check_interval(mut self, interval: Duration) -> Self {
        self.check_interval = interval;
        self
    }
}

impl Clone for ConnectionSupervisor {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            ready_notify: self.ready_notify.clone(),
            config: self.config.clone(),
            retry_config: self.retry_config.clone(),
            check_interval: self.check_interval,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_connection_supervisor_creation() {
        let config = RedisConfig::new("redis://localhost:6379");
        let supervisor = ConnectionSupervisor::new(config);

        assert_eq!(supervisor.state().await, ConnectionState::Disconnected);
    }
}
