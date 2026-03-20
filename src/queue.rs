use crate::config::{QueueConfig, RedisConfig};
use crate::error::{JobError, JobResult};
use crate::job::Job;
use crate::retry::{retry_async, RetryConfig};
use redis::AsyncCommands;
use std::sync::Arc;

pub struct JobQueue {
    config: Arc<QueueConfig>,
    redis_config: Arc<RedisConfig>,
    client: redis::Client,
    retry_config: RetryConfig,
}

impl JobQueue {
    pub async fn new(
        queue_config: QueueConfig,
        redis_config: RedisConfig,
    ) -> JobResult<Self> {
        let client = redis::Client::open(redis_config.url.clone())?;
        
        Ok(Self {
            config: Arc::new(queue_config),
            redis_config: Arc::new(redis_config),
            client,
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

    pub async fn enqueue<T>(&self, job: Job<T>) -> JobResult<String>
    where
        T: serde::Serialize,
    {
        let job_json = job.to_json()?;
        let queue_key = self.redis_config.make_key(&format!("queue:{}", self.config.name));
        let client = self.client.clone();
        
        retry_async(
            || async {
                let mut conn = client.get_multiplexed_async_connection().await
                    .map_err(JobError::from)?;
                conn.rpush::<_, _, ()>(&queue_key, &job_json).await
                    .map_err(JobError::from)?;
                Ok(())
            },
            Some(self.retry_config.clone()),
        ).await?;

        Ok(job.id)
    }

    pub async fn dequeue<T>(&self) -> JobResult<Option<Job<T>>>
    where
        T: for<'de> serde::Deserialize<'de>,
    {
        let queue_key = self.redis_config.make_key(&format!("queue:{}", self.config.name));
        let client = self.client.clone();
        
        let result: Option<String> = retry_async(
            || async {
                let mut conn = client.get_multiplexed_async_connection().await
                    .map_err(JobError::from)?;
                conn.lpop(&queue_key, None).await
                    .map_err(JobError::from)
            },
            Some(self.retry_config.clone()),
        ).await?;
        
        match result {
            Some(job_json) => {
                let job = Job::from_json(&job_json)?;
                Ok(Some(job))
            }
            None => Ok(None),
        }
    }

    pub async fn len(&self) -> JobResult<usize> {
        let queue_key = self.redis_config.make_key(&format!("queue:{}", self.config.name));
        let client = self.client.clone();
        
        retry_async(
            || async {
                let mut conn = client.get_multiplexed_async_connection().await
                    .map_err(JobError::from)?;
                conn.llen(&queue_key).await
                    .map_err(JobError::from)
            },
            Some(self.retry_config.clone()),
        ).await
    }

    pub async fn is_empty(&self) -> JobResult<bool> {
        let length = self.len().await?;
        Ok(length == 0)
    }

    pub async fn clear(&self) -> JobResult<()> {
        let queue_key = self.redis_config.make_key(&format!("queue:{}", self.config.name));
        let client = self.client.clone();
        
        retry_async(
            || async {
                let mut conn = client.get_multiplexed_async_connection().await
                    .map_err(JobError::from)?;
                conn.del::<_, ()>(&queue_key).await
                    .map_err(JobError::from)?;
                Ok(())
            },
            Some(self.retry_config.clone()),
        ).await
    }
}
