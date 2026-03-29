use crate::config::RedisConfig;
use crate::error::{JobError, JobResult};
use crate::retry::{retry_async, RetryConfig};
use redis::aio::{ConnectionManager, MultiplexedConnection};
use redis::AsyncCommands;
use redis::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, error, info};
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
        let conn = ConnectionManager::new(client.clone()).await?;
        
        info!("Redis PubSub connected to {}", config.url);
        
        drop(conn);
        
        Ok(Self {
            config: Arc::new(config),
            client: Arc::new(client),
            retry_config: RetryConfig::new()
                .with_max_attempts(10)
                .with_initial_delay(500)
                .with_max_delay(30000),
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
        let client = self.client.clone();

        retry_async(
            || async {
                let mut conn: MultiplexedConnection = client.get_multiplexed_async_connection().await
                    .map_err(JobError::from)?;
                conn.publish(&full_channel, &serialized).await
                    .map_err(JobError::from)
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
        let (mut sink, mut stream) = retry_async(
            || async {
                client.get_async_pubsub().await
                    .map_err(JobError::from)
            },
            Some(self.retry_config.clone()),
        ).await?.split();

        for channel in &full_channels {
            sink.subscribe(channel).await?;
        }

        info!("Subscribed to channels: {:?}", full_channels);

        while let Some(msg) = stream.next().await {
            let channel_name: String = msg.get_channel_name().to_string();
            let payload_str: String = msg.get_payload()?;
            
            match serde_json::from_str::<T>(&payload_str) {
                Ok(deserialized) => {
                    debug!("Received message from channel: {}", channel_name);
                    callback(channel_name, deserialized);
                }
                Err(e) => {
                    error!("Failed to deserialize message: {}", e);
                }
            }
        }
        
        Ok(())
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
