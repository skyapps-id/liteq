use crate::config::{QueueConfig, RedisConfig, ConsumerInfo};
use crate::error::{JobError, JobResult};
use crate::job::Job;
use crate::retry::{retry_async, RetryConfig};
use chrono::Utc;
use redis::{AsyncCommands, Cmd, Script};
use std::collections::HashSet;
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

static DEQUEUE_CONSUMER_SCRIPT: &str = r#"
local list_key = KEYS[1]
local zset_key = KEYS[2]
local meta_key = KEYS[3]
local current_time = tonumber(ARGV[1])
local consumer_id = tonumber(ARGV[2])
local num_consumers = tonumber(ARGV[3])

-- Get current counter
local current_counter = tonumber(redis.call('HGET', meta_key, 'counter') or '0')

-- Bootstrap: if counter is 0, let ANY consumer take first job
if current_counter == 0 then
    -- 1. Check ready scheduled jobs (get first element)
    local ready_jobs = redis.call('ZRANGEBYSCORE', zset_key, '-inf', current_time)
    if ready_jobs and #ready_jobs > 0 then
        local job_json = ready_jobs[1]
        redis.call('ZREM', zset_key, job_json)
        redis.call('HINCRBY', meta_key, 'counter', 1)
        return job_json
    end

    -- 2. Get from regular queue
    local job = redis.call('LPOP', list_key)
    if job then
        redis.call('HINCRBY', meta_key, 'counter', 1)
        return job
    end

    return nil
end

-- Normal operation: use modulo distribution
local target_consumer = current_counter % num_consumers

-- Only return job if this consumer is the target
if target_consumer == consumer_id then
    -- 1. Check ready scheduled jobs (get first element)
    local ready_jobs = redis.call('ZRANGEBYSCORE', zset_key, '-inf', current_time)
    if ready_jobs and #ready_jobs > 0 then
        local job_json = ready_jobs[1]
        redis.call('ZREM', zset_key, job_json)
        -- Increment counter ONLY when job is actually taken
        redis.call('HINCRBY', meta_key, 'counter', 1)
        return job_json
    end

    -- 2. Get from regular queue
    local job = redis.call('LPOP', list_key)
    if job then
        -- Increment counter ONLY when job is actually taken
        redis.call('HINCRBY', meta_key, 'counter', 1)
        return job
    end
end

-- Not this consumer's turn or no jobs available
return nil
"#;

pub struct JobQueue {
    config: Arc<QueueConfig>,
    redis_config: Arc<RedisConfig>,
    client: redis::Client,
    retry_config: RetryConfig,
    dequeue_script: Script,
    dequeue_consumer_script: Script,
}

impl JobQueue {
    /// Creates a new JobQueue instance
    pub async fn new(
        queue_config: QueueConfig,
        redis_config: RedisConfig,
    ) -> JobResult<Self> {
        let client = redis::Client::open(redis_config.url.clone())?;
        let dequeue_script = Script::new(DEQUEUE_SCRIPT);
        let dequeue_consumer_script = Script::new(DEQUEUE_CONSUMER_SCRIPT);

        Ok(Self {
            config: Arc::new(queue_config),
            redis_config: Arc::new(redis_config),
            client,
            retry_config: RetryConfig::new()
                .with_max_attempts(10)
                .with_initial_delay(500)
                .with_max_delay(30000),
            dequeue_script,
            dequeue_consumer_script,
        })
    }

    /// Configures retry settings
    pub fn with_retry_config(mut self, config: RetryConfig) -> Self {
        self.retry_config = config;
        self
    }

    /// Adds job to queue (with ETA → ZSET, without ETA → LIST)
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

    /// Gets next job (prioritizes ready scheduled jobs, then regular)
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

    /// Gets next job with consumer-aware fair distribution
    pub async fn dequeue_with_consumer<T>(&self, consumer_info: &ConsumerInfo) -> JobResult<Option<Job<T>>>
    where
        T: for<'de> serde::Deserialize<'de>,
    {
        let queue_key = self.redis_config.make_key(&format!("queue:{}", self.config.name));
        let schedule_key = self.redis_config.make_key(&format!("schedule:{}", self.config.name));
        let meta_key = self.redis_config.make_key(&format!("{}:meta", self.config.name));
        let client = self.client.clone();
        let current_time = Utc::now().timestamp();

        let result: Option<String> = retry_async(
            || async {
                let mut conn = client.get_multiplexed_async_connection().await
                    .map_err(JobError::from)?;

                let job_json: Option<String> = self.dequeue_consumer_script
                    .key(&queue_key)
                    .key(&schedule_key)
                    .key(&meta_key)
                    .arg(current_time)
                    .arg(consumer_info.id)
                    .arg(consumer_info.total)
                    .invoke_async(&mut conn)
                    .await
                    .map_err(JobError::from)?;

                Ok(job_json)
            },
            Some(self.retry_config.clone()),
        ).await?;

        match result {
            Some(job_json) => {
                tracing::debug!(
                    queue = %self.config.name,
                    consumer_id = consumer_info.id,
                    total_consumers = consumer_info.total,
                    "Job dequeued by consumer {} of {}",
                    consumer_info.id,
                    consumer_info.total
                );
                let job = Job::from_json(&job_json)?;
                Ok(Some(job))
            }
            None => Ok(None),
        }
    }

    /// Returns (regular_count, scheduled_count)
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

    /// Gets up to batch_size ready jobs at once
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

    /// Deletes all jobs (cannot be undone!)
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

    /// Returns queue statistics
    pub async fn get_queue_stats(&self) -> JobResult<QueueStats> {
        let (regular_count, scheduled_count) = self.get_job_counts().await?;

        Ok(QueueStats {
            queue_name: self.config.name.clone(),
            regular_jobs: regular_count,
            scheduled_jobs: scheduled_count,
            total_pending: regular_count + scheduled_count,
        })
    }

    /// Lists all queue names in Redis
    pub async fn list_all(config: &RedisConfig) -> JobResult<Vec<String>> {
        let client = redis::Client::open(config.url.clone())?;
        let mut conn = client.get_multiplexed_async_connection().await
            .map_err(JobError::from)?;
        
        // Get all queue keys (LIST)
        let queue_pattern = format!("{}:*queue:*", config.key_prefix);
        let queue_keys: Vec<String> = Cmd::keys(&queue_pattern)
            .query_async(&mut conn)
            .await
            .map_err(JobError::from)?;
        
        // Get all schedule keys (ZSET)
        let schedule_pattern = format!("{}:*schedule:*", config.key_prefix);
        let schedule_keys: Vec<String> = Cmd::keys(&schedule_pattern)
            .query_async(&mut conn)
            .await
            .map_err(JobError::from)?;
        
        // Extract queue names from keys
        let mut queue_names = HashSet::new();
        
        for key in queue_keys {
            // Format: {prefix}:queue:{queue_name}
            if let Some(rest) = key.strip_prefix(&format!("{}:queue:", config.key_prefix)) {
                queue_names.insert(rest.to_string());
            }
        }
        
        for key in schedule_keys {
            // Format: {prefix}:schedule:{queue_name}
            if let Some(rest) = key.strip_prefix(&format!("{}:schedule:", config.key_prefix)) {
                queue_names.insert(rest.to_string());
            }
        }
        
        let mut result: Vec<String> = queue_names.into_iter().collect();
        result.sort();
        
        Ok(result)
    }

    /// Returns statistics for all queues
    pub async fn get_all_queue_stats(config: &RedisConfig) -> JobResult<Vec<QueueStats>> {
        let queue_names = Self::list_all(config).await?;
        
        let mut all_stats = Vec::new();
        
        for queue_name in queue_names {
            let queue_config = QueueConfig::new(&queue_name);
            let queue = JobQueue::new(queue_config, config.clone()).await?;
            
            let (regular, scheduled) = queue.get_job_counts().await?;
            all_stats.push(QueueStats {
                queue_name,
                regular_jobs: regular,
                scheduled_jobs: scheduled,
                total_pending: regular + scheduled,
            });
        }
        
        Ok(all_stats)
    }
}

#[derive(Debug, Clone)]
pub struct QueueStats {
    pub queue_name: String,
    pub regular_jobs: usize,
    pub scheduled_jobs: usize,
    pub total_pending: usize,
}
