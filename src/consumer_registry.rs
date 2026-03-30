use crate::config::{RedisConfig, ConsumerInfo};
use crate::error::{JobError, JobResult};
use crate::retry::{retry_async, RetryConfig};
use chrono::Utc;
use redis::Client;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

/// Generate a unique consumer ID without UUID
/// Format: {timestamp_ms}-{random_u32}
/// Example: "1712345678901-1234567890"
fn generate_consumer_id() -> String {
    let timestamp_ms = Utc::now().timestamp_millis();
    let random: u32 = rand::random();
    format!("{}-{}", timestamp_ms, random)
}

pub struct ConsumerRegistry {
    redis_config: Arc<RedisConfig>,
    retry_config: RetryConfig,
}

impl ConsumerRegistry {
    pub fn new(redis_config: RedisConfig) -> Self {
        Self {
            redis_config: Arc::new(redis_config),
            retry_config: RetryConfig::new()
                .with_max_attempts(5)
                .with_initial_delay(200)
                .with_max_delay(5000)
                .with_jitter(50),
        }
    }

    /// Helper to create a ConnectionManager with proper timeout configuration
    async fn create_connection_manager(&self) -> JobResult<redis::aio::ConnectionManager> {
        let client = Client::open(self.redis_config.url.clone())?;

        let manager_config = redis::aio::ConnectionManagerConfig::new()
            .set_connection_timeout(Some(Duration::from_secs(self.redis_config.connection_timeout_secs)))
            .set_response_timeout(Some(Duration::from_secs(self.redis_config.response_timeout_secs)))
            .set_number_of_retries(20)
            .set_min_delay(Duration::from_secs(1))
            .set_max_delay(Duration::from_secs(60));

        redis::aio::ConnectionManager::new_with_config(client, manager_config)
            .await
            .map_err(JobError::from)
    }

    /// Register this consumer and start heartbeat
    pub async fn register_and_start_heartbeat(
        &self,
        queue_name: &str,
    ) -> JobResult<ConsumerInfo> {
        let my_uuid = generate_consumer_id();
        let started_at = Utc::now().timestamp();
        let consumer_key = self.redis_config.make_key(&format!(
            "consumers:{}:{}",
            queue_name, my_uuid
        ));

        // Register this consumer using ConnectionManager
        retry_async(
            || async {
                let mut conn_manager = Self::create_connection_manager(self).await?;

                // Set consumer metadata
                redis::cmd("HSET")
                    .arg(&consumer_key)
                    .arg("uuid")
                    .arg(&my_uuid)
                    .arg("started_at")
                    .arg(started_at)
                    .arg("last_heartbeat")
                    .arg(started_at)
                    .query_async::<()>(&mut conn_manager)
                    .await
                    .map_err(JobError::from)?;

                // Set TTL (30 seconds)
                redis::cmd("EXPIRE")
                    .arg(&consumer_key)
                    .arg(30)
                    .query_async::<()>(&mut conn_manager)
                    .await
                    .map_err(JobError::from)?;

                Ok(())
            },
            Some(self.retry_config.clone()),
        ).await?;

        // Wait for other consumers to register (critical for fair distribution!)
        tracing::info!(
            queue = %queue_name,
            consumer_uuid = %my_uuid,
            "Waiting for other consumers to register..."
        );
        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

        // Multiple retries to ensure all consumers are registered
        let mut final_id = 0;
        let mut final_total = 1;

        for attempt in 1..=5 {
            let (id, total) = get_consumer_position(&self.redis_config, queue_name, &my_uuid).await?;

            tracing::info!(
                queue = %queue_name,
                consumer_uuid = %my_uuid,
                attempt = attempt,
                id = id,
                total = total,
                "Consumer registration attempt"
            );

            // Update if we found more consumers
            if total > final_total {
                final_id = id;
                final_total = total;
                tracing::info!(
                    queue = %queue_name,
                    consumer_uuid = %my_uuid,
                    new_total = total,
                    "Consumer count increased, updating"
                );
            } else if attempt == 1 {
                final_id = id;
                final_total = total;
            }

            // If this is not the last attempt, wait and retry
            if attempt < 5 {
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            }
        }

        let (my_id, total) = (final_id, final_total);

        let consumer_info = ConsumerInfo {
            uuid: my_uuid.clone(),
            id: my_id,
            total,
            queue_name: queue_name.to_string(),
        };

        // Clone values for logging before moving
        let log_queue_name = queue_name.to_string();
        let log_uuid = my_uuid.clone();

        // Start heartbeat task
        let redis_config = self.redis_config.clone();
        let queue_name = queue_name.to_string();
        let error_uuid = log_uuid.clone();
        tokio::spawn(async move {
            if let Err(e) = Self::heartbeat_task(
                redis_config,
                consumer_key,
                queue_name,
                my_uuid,
            ).await {
                tracing::error!(
                    error = %e,
                    "Heartbeat task failed for consumer {}",
                    error_uuid
                );
            }
        });

        tracing::info!(
            queue = %log_queue_name,
            consumer_uuid = %log_uuid,
            consumer_id = my_id,
            total_consumers = total,
            "Registered consumer (ID: {}/{})",
            my_id, total
        );

        Ok(consumer_info)
    }

    /// Background task to refresh TTL every 10 seconds
    async fn heartbeat_task(
        redis_config: Arc<RedisConfig>,
        consumer_key: String,
        queue_name: String,
        my_uuid: String,
    ) -> JobResult<()> {
        let client = Client::open(redis_config.url.clone())?;

        let manager_config = redis::aio::ConnectionManagerConfig::new()
            .set_connection_timeout(Some(Duration::from_secs(redis_config.connection_timeout_secs)))
            .set_response_timeout(Some(Duration::from_secs(redis_config.response_timeout_secs)))
            .set_number_of_retries(20)
            .set_min_delay(Duration::from_secs(1))
            .set_max_delay(Duration::from_secs(60));

        let mut conn_manager = redis::aio::ConnectionManager::new_with_config(client, manager_config).await?;

        loop {
            sleep(Duration::from_secs(10)).await;

            // Refresh TTL using ConnectionManager
            let result = async {
                let now = Utc::now().timestamp();

                // Update heartbeat timestamp
                redis::cmd("HSET")
                    .arg(&consumer_key)
                    .arg("last_heartbeat")
                    .arg(now)
                    .query_async::<()>(&mut conn_manager)
                    .await
                    .map_err(JobError::from)?;

                // Refresh TTL
                redis::cmd("EXPIRE")
                    .arg(&consumer_key)
                    .arg(30)
                    .query_async::<()>(&mut conn_manager)
                    .await
                    .map_err(JobError::from)?;

                Ok::<(), JobError>(())
            }.await;

            match result {
                Ok(_) => {
                    tracing::debug!(
                        queue = %queue_name,
                        consumer_uuid = %my_uuid,
                        "Heartbeat sent"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        queue = %queue_name,
                        consumer_uuid = %my_uuid,
                        "Failed to send heartbeat, will retry"
                    );
                }
            }
        }
    }

    /// Query all active consumers for a queue
    pub async fn get_active_consumers(&self, queue_name: &str) -> JobResult<Vec<String>> {
        let pattern = self.redis_config.make_key(&format!("consumers:{}:*", queue_name));

        let mut conn_manager = self.create_connection_manager().await?;

        let keys: Vec<String> = redis::cmd("KEYS")
            .arg(&pattern)
            .query_async(&mut conn_manager)
            .await?;

        Ok(keys)
    }
}

/// Get consumer position (async helper)
pub async fn get_consumer_position(
    redis_config: &RedisConfig,
    queue_name: &str,
    my_uuid: &str,
) -> JobResult<(usize, usize)> {
    let pattern = redis_config.make_key(&format!("consumers:{}:*", queue_name));
    let client = Client::open(redis_config.url.clone())?;

    // Wait a bit for other consumers to register
    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

    let manager_config = redis::aio::ConnectionManagerConfig::new()
        .set_connection_timeout(Some(Duration::from_secs(redis_config.connection_timeout_secs)))
        .set_response_timeout(Some(Duration::from_secs(redis_config.response_timeout_secs)))
        .set_number_of_retries(20)
        .set_min_delay(Duration::from_secs(1))
        .set_max_delay(Duration::from_secs(60));

    let mut conn_manager = redis::aio::ConnectionManager::new_with_config(client, manager_config).await?;

    let keys: Vec<String> = redis::cmd("KEYS")
        .arg(&pattern)
        .query_async(&mut conn_manager)
        .await?;

    // Sort keys to ensure consistent ordering
    let mut sorted_keys = keys;
    sorted_keys.sort();

    // Find my position
    let my_key = redis_config.make_key(&format!("consumers:{}:{}", queue_name, my_uuid));

    // Verify my key exists
    if !sorted_keys.contains(&my_key) {
        tracing::warn!(
            queue = %queue_name,
            my_uuid = %my_uuid,
            "My key not found in registry, using fallback (single-consumer mode)"
        );
        return Ok((0, 1)); // Fallback to single-consumer mode
    }

    let my_position = sorted_keys
        .iter()
        .position(|k| k == &my_key)
        .unwrap_or(0);

    let total = sorted_keys.len().max(1); // At least 1 (me)

    tracing::info!(
        queue = %queue_name,
        my_uuid = %my_uuid,
        my_position = my_position,
        total_consumers = total,
        "Calculated consumer position"
    );

    Ok((my_position, total))
}
