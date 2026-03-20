use crate::config::RedisConfig;
use crate::error::{JobError, JobResult};
use crate::retry::{retry_async, RetryConfig};
use redis::aio::ConnectionManager;
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
                let mut conn = client.get_multiplexed_async_connection().await
                    .map_err(JobError::from)?;
                conn.publish(&full_channel, &serialized).await
                    .map_err(JobError::from)
            },
            Some(self.retry_config.clone()),
        ).await
    }

    #[allow(deprecated)]
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
        let conn = retry_async(
            || async {
                client.get_async_connection().await
                    .map_err(JobError::from)
            },
            Some(self.retry_config.clone()),
        ).await?;
        
        let mut pubsub = conn.into_pubsub();
        
        for channel in &full_channels {
            pubsub.subscribe(channel).await?;
        }
        
        info!("Subscribed to channels: {:?}", full_channels);
        
        let mut stream = pubsub.on_message();
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

    #[allow(deprecated)]
    pub async fn psubscribe<F, T>(&self, patterns: Vec<String>, callback: F) -> JobResult<()>
    where
        F: Fn(String, T) + Send + Sync + 'static,
        T: for<'de> Deserialize<'de> + Send + 'static,
    {
        let full_patterns: Vec<String> = patterns
            .iter()
            .map(|p| self.config.make_key(p))
            .collect();

        info!("Subscribing to patterns: {:?}", full_patterns);
        
        let client = self.client.clone();
        let conn = retry_async(
            || async {
                client.get_async_connection().await
                    .map_err(JobError::from)
            },
            Some(self.retry_config.clone()),
        ).await?;
        
        let mut pubsub = conn.into_pubsub();
        
        for pattern in &full_patterns {
            pubsub.psubscribe(pattern).await?;
        }
        
        info!("Subscribed to patterns: {:?}", full_patterns);
        
        let mut stream = pubsub.on_message();
        while let Some(msg) = stream.next().await {
            let pattern_name: String = msg.get_channel_name().to_string();
            let payload_str: String = msg.get_payload()?;
            
            match serde_json::from_str::<T>(&payload_str) {
                Ok(deserialized) => {
                    debug!("Received message from pattern: {}", pattern_name);
                    callback(pattern_name, deserialized);
                }
                Err(e) => {
                    error!("Failed to deserialize message: {}", e);
                }
            }
        }
        
        Ok(())
    }

    pub async fn publish_event<T>(&self, event: &Event<T>) -> JobResult<u64>
    where
        T: Serialize,
    {
        self.publish(&event.event_type, event).await
    }

    #[allow(deprecated)]
    pub async fn unsubscribe(&self, channels: Vec<String>) -> JobResult<()> {
        let full_channels: Vec<String> = channels
            .iter()
            .map(|ch| self.config.make_key(ch))
            .collect();

        let client = self.client.clone();
        let conn = retry_async(
            || async {
                client.get_async_connection().await
                    .map_err(JobError::from)
            },
            Some(self.retry_config.clone()),
        ).await?;
        
        let mut pubsub = conn.into_pubsub();
        
        for channel in &full_channels {
            pubsub.unsubscribe(channel).await?;
        }
        
        info!("Unsubscribed from channels: {:?}", full_channels);
        
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
