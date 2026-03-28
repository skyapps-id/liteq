use crate::config::{QueueConfig, RedisConfig};
use crate::error::{JobError, JobResult};
use crate::job::Job;
use crate::retry::{retry_async, RetryConfig};
use chrono::Utc;
use redis::{AsyncCommands, Script};
use std::sync::Arc;

static DEQUEUE_SCRIPT: &str = r#"
local list_key = KEYS[1]
local zset_key = KEYS[2]
local current_time = tonumber(ARGV[1])

-- 1. Check scheduled jobs that are ready (timestamp <= current_time)
local ready_jobs = redis.call('ZRANGEBYSCORE', zset_key, '-inf', current_time, 'LIMIT', 0, 1)
if ready_jobs and #ready_jobs > 0 then
    local job_json = ready_jobs[1]
    redis.call('ZREM', zset_key, job_json)
    return job_json
end

-- 2. Get from regular queue
local job = redis.call('LPOP', list_key)
return job
"#;

pub struct JobQueue {
    config: Arc<QueueConfig>,
    redis_config: Arc<RedisConfig>,
    client: redis::Client,
    retry_config: RetryConfig,
    dequeue_script: Script,
}

impl JobQueue {
    pub async fn new(
        queue_config: QueueConfig,
        redis_config: RedisConfig,
    ) -> JobResult<Self> {
        let client = redis::Client::open(redis_config.url.clone())?;
        let dequeue_script = Script::new(DEQUEUE_SCRIPT);
        
        Ok(Self {
            config: Arc::new(queue_config),
            redis_config: Arc::new(redis_config),
            client,
            retry_config: RetryConfig::new()
                .with_max_attempts(10)
                .with_initial_delay(500)
                .with_max_delay(30000),
            dequeue_script,
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
        let schedule_key = self.redis_config.make_key(&format!("schedule:{}", self.config.name));
        let client = self.client.clone();
        
        let has_eta = job.eta.is_some();
        
        retry_async(
            || async {
                let mut conn = client.get_multiplexed_async_connection().await
                    .map_err(JobError::from)?;
                
                if has_eta {
                    let eta_timestamp = job.eta.unwrap().timestamp();
                    conn.zadd::<_, _, _, ()>(&schedule_key, &job_json, eta_timestamp).await
                        .map_err(JobError::from)?;
                } else {
                    conn.rpush::<_, _, ()>(&queue_key, &job_json).await
                        .map_err(JobError::from)?;
                }
                
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
        let schedule_key = self.redis_config.make_key(&format!("schedule:{}", self.config.name));
        let client = self.client.clone();
        let current_time = Utc::now().timestamp();
        
        let result: Option<String> = retry_async(
            || async {
                let mut conn = client.get_multiplexed_async_connection().await
                    .map_err(JobError::from)?;
                
                let job_json: Option<String> = self.dequeue_script
                    .key(&queue_key)
                    .key(&schedule_key)
                    .arg(current_time)
                    .invoke_async(&mut conn)
                    .await
                    .map_err(JobError::from)?;
                
                Ok(job_json)
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

    /// Get the count of scheduled and regular jobs for monitoring
    pub async fn get_job_counts(&self) -> JobResult<(usize, usize)> {
        let queue_key = self.redis_config.make_key(&format!("queue:{}", self.config.name));
        let schedule_key = self.redis_config.make_key(&format!("schedule:{}", self.config.name));
        let client = self.client.clone();

        let (regular_count, scheduled_count): (usize, usize) = retry_async(
            || async {
                let mut conn = client.get_multiplexed_async_connection().await
                    .map_err(JobError::from)?;

                let regular: usize = conn.llen(&queue_key).await
                    .map_err(JobError::from)?;
                let scheduled: usize = conn.zcard(&schedule_key).await
                    .map_err(JobError::from)?;

                Ok((regular, scheduled))
            },
            Some(self.retry_config.clone()),
        ).await?;

        Ok((regular_count, scheduled_count))
    }

    /// Batch dequeue multiple ready jobs at once for high-throughput scenarios
    ///
    /// Returns up to `batch_size` jobs that are ready to process
    /// Prioritizes scheduled jobs that are ready, then regular jobs
    pub async fn dequeue_batch<T>(&self, batch_size: usize) -> JobResult<Vec<Job<T>>>
    where
        T: for<'de> serde::Deserialize<'de>,
    {
        if batch_size == 0 {
            return Ok(Vec::new());
        }

        let queue_key = self.redis_config.make_key(&format!("queue:{}", self.config.name));
        let schedule_key = self.redis_config.make_key(&format!("schedule:{}", self.config.name));
        let client = self.client.clone();
        let current_time = Utc::now().timestamp();

        let jobs = retry_async(
            || async {
                let mut conn = client.get_multiplexed_async_connection().await
                    .map_err(JobError::from)?;

                let mut result = Vec::new();

                // First, try to get ready scheduled jobs (up to batch_size)
                let ready_jobs: Vec<String> = conn
                    .zrangebyscore_limit(&schedule_key, "-inf", current_time, 0, batch_size as isize)
                    .await
                    .map_err(JobError::from)?;

                for job_json in ready_jobs {
                    // Atomic ZREM to ensure no race condition
                    let removed: i32 = conn.zrem(&schedule_key, &job_json).await
                        .map_err(JobError::from)?;

                    if removed > 0 {
                        if let Ok(job) = Job::from_json(&job_json) {
                            result.push(job);
                        }
                    }

                    if result.len() >= batch_size {
                        break;
                    }
                }

                // If we haven't filled the batch, get regular jobs
                while result.len() < batch_size {
                    let job_json: Option<String> = conn.lpop(&queue_key, None).await
                        .map_err(JobError::from)?;

                    match job_json {
                        Some(json) => {
                            if let Ok(job) = Job::from_json(&json) {
                                result.push(job);
                            }
                        }
                        None => break,
                    }
                }

                Ok(result)
            },
            Some(self.retry_config.clone()),
        ).await?;

        Ok(jobs)
    }

    /// Flush all jobs from this queue (both LIST and ZSET)
    /// 
    /// This removes ALL pending jobs from the queue:
    /// - Regular jobs in LIST
    /// - Scheduled jobs in ZSET
    /// 
    /// Use with caution - this operation cannot be undone!
    pub async fn flush(&self) -> JobResult<()> {
        let queue_key = self.redis_config.make_key(&format!("queue:{}", self.config.name));
        let schedule_key = self.redis_config.make_key(&format!("schedule:{}", self.config.name));
        let client = self.client.clone();

        retry_async(
            || async {
                let mut conn = client.get_multiplexed_async_connection().await
                    .map_err(JobError::from)?;

                // Delete both LIST and ZSET
                conn.del::<_, ()>(&[&queue_key, &schedule_key]).await
                    .map_err(JobError::from)?;

                Ok(())
            },
            Some(self.retry_config.clone()),
        ).await?;

        Ok(())
    }

    /// Get detailed queue statistics for monitoring and observability
    pub async fn get_queue_stats(&self) -> JobResult<QueueStats> {
        let (regular_count, scheduled_count) = self.get_job_counts().await?;

        Ok(QueueStats {
            queue_name: self.config.name.clone(),
            regular_jobs: regular_count,
            scheduled_jobs: scheduled_count,
            total_pending: regular_count + scheduled_count,
        })
    }
}

/// Queue statistics for monitoring
#[derive(Debug, Clone)]
pub struct QueueStats {
    pub queue_name: String,
    pub regular_jobs: usize,
    pub scheduled_jobs: usize,
    pub total_pending: usize,
}
