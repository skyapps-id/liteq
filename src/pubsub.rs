use crate::config::RedisConfig;
use crate::error::{JobError, JobResult};
use crate::retry::{retry_async, RetryConfig};
use redis::AsyncCommands;
use redis::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info, warn};
use futures_util::StreamExt;

#[derive(Clone)]
pub struct RedisPubSub {
    config: Arc<RedisConfig>,
    client: Arc<Client>,
    retry_config: RetryConfig,
}

impl RedisPubSub {
    pub async fn new(config: RedisConfig) -> JobResult<Self> {
        let client = redis::Client::open(config.url.clone())?;

        info!("Redis PubSub created for {}", config.url);

        Ok(Self {
            config: Arc::new(config),
            client: Arc::new(client),
            retry_config: RetryConfig::new()
                .with_max_attempts(20)
                .with_initial_delay(1000)
                .with_max_delay(60000),
        })
    }

    pub fn with_retry_config(mut self, config: RetryConfig) -> Self {
        self.retry_config = config;
        self
    }

    pub async fn publish<T>(&self, channel: &str, message: &T) -> JobResult<u64>
    where
        T: Serialize,
    {
        let serialized = serde_json::to_string(message)?;
        let full_channel = self.config.make_key(channel);

        info!("Publishing to channel: {} with payload: {}", full_channel, serialized);

        retry_async(
            || async {
                // Use regular async connection for publish, not ConnectionManager
                // ConnectionManager uses MULTI/EXEC pipeline which isn't ideal for PUBSUB
                let mut conn = self.client.get_multiplexed_async_connection().await
                    .map_err(JobError::from)?;
                let result = conn.publish(&full_channel, &serialized).await
                    .map_err(JobError::from)?;
                debug!("Published to {} subscribers on channel {}", result, full_channel);
                Ok(result)
            },
            Some(self.retry_config.clone()),
        ).await
    }

    pub async fn subscribe<F, T>(&self, channels: Vec<String>, callback: F) -> JobResult<()>
    where
        F: Fn(String, T) + Send + Sync + 'static,
        T: for<'de> Deserialize<'de> + Send + 'static,
    {
        let full_channels: Vec<String> = channels
            .iter()
            .map(|ch| self.config.make_key(ch))
            .collect();

        info!("Subscribing to channels: {:?}", full_channels);

        let client = self.client.clone();
        let retry_config = self.retry_config.clone();
        let callback_arc = Arc::new(callback);

        // Spawn a task that will auto-reconnect on connection loss
        tokio::spawn(async move {
            let mut reconnect_count = 0u32;

            loop {
                match Self::subscribe_and_listen(
                    client.clone(),
                    &full_channels,
                    callback_arc.clone(),
                    retry_config.clone(),
                    reconnect_count,
                ).await {
                    Ok(_) => {
                        info!("Subscribe stream ended gracefully");
                        break;
                    }
                    Err(e) => {
                        reconnect_count += 1;
                        warn!(
                            "Subscribe connection lost (attempt {}): {}. Reconnecting in 5s...",
                            reconnect_count, e
                        );
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        });

        Ok(())
    }

    async fn subscribe_and_listen<F, T>(
        client: Arc<Client>,
        channels: &[String],
        callback: Arc<F>,
        retry_config: RetryConfig,
        reconnect_count: u32,
    ) -> JobResult<()>
    where
        F: Fn(String, T) + Send + Sync + 'static,
        T: for<'de> Deserialize<'de> + Send + 'static,
    {
        info!("Establishing pubsub connection (reconnect #{})", reconnect_count);

        let mut pubsub = retry_async(
            || async {
                client.get_async_pubsub().await
                    .map_err(JobError::from)
            },
            Some(retry_config),
        ).await?;

        // Re-subscribe to all channels
        for channel in channels {
            info!("Subscribing to channel: {}", channel);
            pubsub.subscribe(channel).await?;
        }

        info!("Successfully subscribed to channels: {:?}", channels);

        // Listen for messages
        let mut stream = pubsub.on_message();
        while let Some(msg) = stream.next().await {
            let channel_name: String = msg.get_channel_name().to_string();

            match msg.get_payload::<String>() {
                Ok(payload_str) => {
                    info!("Raw payload from {}: {}", channel_name, payload_str);

                    match serde_json::from_str::<T>(&payload_str) {
                        Ok(deserialized) => {
                            info!("Successfully deserialized message from channel: {}", channel_name);
                            callback(channel_name, deserialized);
                        }
                        Err(e) => {
                            error!("Failed to deserialize message from {}: {}. Payload: {}", channel_name, e, payload_str);
                        }
                    }
                }
                Err(e) => {
                    // Some messages (like subscribe confirmations) don't have payloads
                    debug!("Message from {} has no payload: {:?}", channel_name, e);
                }
            }
        }

        warn!("PubSub stream ended");
        Err(JobError::QueueError("PubSub connection lost".to_string()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event<T> {
    pub event_type: String,
    pub timestamp: i64,
    pub data: T,
}

impl<T> Event<T> {
    pub fn new(event_type: impl Into<String>, data: T) -> Self {
        Self {
            event_type: event_type.into(),
            timestamp: chrono::Utc::now().timestamp(),
            data,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_event_creation() {
        let data = "test_data".to_string();
        let event = Event::new("test_event", data);
        
        assert_eq!(event.event_type, "test_event");
        assert!(event.timestamp > 0);
    }
}
