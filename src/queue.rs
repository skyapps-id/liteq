use crate::config::{QueueConfig, RedisConfig, ConsumerInfo};
use crate::error::{JobError, JobResult};
use crate::job::Job;
use crate::retry::{retry_async, RetryConfig};
use chrono::{DateTime, Utc};
use redis::{AsyncCommands, Cmd, Script, Client};
use redis::aio::ConnectionManager;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

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

/// Builder for enqueuing jobs with optional scheduling
pub struct Enqueuer<'a, T> {
    queue: &'a JobQueue,
    payload: T,
    eta: Option<DateTime<Utc>>,
}

impl<'a, T> Enqueuer<'a, T>
where
    T: serde::Serialize + Send,
{
    /// Sets scheduled execution time (ETA)
    pub fn with_eta(mut self, eta: DateTime<Utc>) -> Self {
        self.eta = Some(eta);
        self
    }

    /// Execute the enqueue operation
    pub async fn send(self) -> JobResult<String> {
        let mut job = Job::new(self.payload);
        job.queue = self.queue.config.name.clone();

        if let Some(eta) = self.eta {
            job = job.with_eta(eta);
            let job_json = job.to_json()?;
            let schedule_key = self
                .queue
                .redis_config
                .make_key(&format!("schedule:{}", self.queue.config.name));
            let eta_timestamp = eta.timestamp();

            retry_async(
                || async {
                    let mut conn_manager = self.queue.create_connection_manager().await?;
                    conn_manager
                        .zadd::<_, _, _, ()>(&schedule_key, &job_json, eta_timestamp)
                        .await
                        .map_err(JobError::from)?;
                    Ok(())
                },
                Some(self.queue.retry_config.clone()),
            )
            .await?;
        } else {
            let job_json = job.to_json()?;
            let queue_key = self
                .queue
                .redis_config
                .make_key(&format!("queue:{}", self.queue.config.name));

            retry_async(
                || async {
                    let mut conn_manager = self.queue.create_connection_manager().await?;
                    conn_manager
                        .rpush::<_, _, ()>(&queue_key, &job_json)
                        .await
                        .map_err(JobError::from)?;
                    Ok(())
                },
                Some(self.queue.retry_config.clone()),
            )
            .await?;
        }

        Ok(job.id)
    }
}

pub struct JobQueue {
    config: Arc<QueueConfig>,
    redis_config: Arc<RedisConfig>,
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
        let dequeue_script = Script::new(DEQUEUE_SCRIPT);
        let dequeue_consumer_script = Script::new(DEQUEUE_CONSUMER_SCRIPT);

        Ok(Self {
            config: Arc::new(queue_config),
            redis_config: Arc::new(redis_config),
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

    /// Helper to create a ConnectionManager with proper timeout configuration
    async fn create_connection_manager(&self) -> JobResult<ConnectionManager> {
        let client = Client::open(self.redis_config.url.clone())?;

        let manager_config = redis::aio::ConnectionManagerConfig::new()
            .set_connection_timeout(Some(Duration::from_secs(self.redis_config.connection_timeout_secs)))
            .set_response_timeout(Some(Duration::from_secs(self.redis_config.response_timeout_secs)))
            .set_number_of_retries(20)
            .set_min_delay(Duration::from_secs(1))
            .set_max_delay(Duration::from_secs(60));

        ConnectionManager::new_with_config(client, manager_config)
            .await
            .map_err(JobError::from)
    }

    /// Creates a builder for enqueuing jobs (supports immediate and scheduled)
    pub fn enqueue<T>(&self, payload: T) -> Enqueuer<'_, T>
    where
        T: serde::Serialize + Send,
    {
        Enqueuer {
            queue: self,
            payload,
            eta: None,
        }
    }

    /// Gets next job (prioritizes ready scheduled jobs, then regular)
    pub async fn dequeue<T>(&self) -> JobResult<Option<Job<T>>>
    where
        T: for<'de> serde::Deserialize<'de>,
    {
        let queue_key = self.redis_config.make_key(&format!("queue:{}", self.config.name));
        let schedule_key = self.redis_config.make_key(&format!("schedule:{}", self.config.name));
        let current_time = Utc::now().timestamp();

        let result: Option<String> = retry_async(
            || async {
                let mut conn_manager = self.create_connection_manager().await?;

                let job_json: Option<String> = self.dequeue_script
                    .key(&queue_key)
                    .key(&schedule_key)
                    .arg(current_time)
                    .invoke_async(&mut conn_manager)
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
        let current_time = Utc::now().timestamp();

        let result: Option<String> = retry_async(
            || async {
                let mut conn_manager = self.create_connection_manager().await?;

                let job_json: Option<String> = self.dequeue_consumer_script
                    .key(&queue_key)
                    .key(&schedule_key)
                    .key(&meta_key)
                    .arg(current_time)
                    .arg(consumer_info.id)
                    .arg(consumer_info.total)
                    .invoke_async(&mut conn_manager)
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

        let (regular_count, scheduled_count): (usize, usize) = retry_async(
            || async {
                let mut conn_manager = self.create_connection_manager().await?;

                let regular: usize = conn_manager.llen(&queue_key).await
                    .map_err(JobError::from)?;
                let scheduled: usize = conn_manager.zcard(&schedule_key).await
                    .map_err(JobError::from)?;

                Ok((regular, scheduled))
            },
            Some(self.retry_config.clone()),
        ).await?;

        Ok((regular_count, scheduled_count))
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

        let manager_config = redis::aio::ConnectionManagerConfig::new()
            .set_connection_timeout(Some(Duration::from_secs(config.connection_timeout_secs)))
            .set_response_timeout(Some(Duration::from_secs(config.response_timeout_secs)))
            .set_number_of_retries(20)
            .set_min_delay(Duration::from_secs(1))
            .set_max_delay(Duration::from_secs(60));

        let mut conn_manager = redis::aio::ConnectionManager::new_with_config(client, manager_config)
            .await
            .map_err(JobError::from)?;

        // Get all queue keys (LIST)
        let queue_pattern = format!("{}:*queue:*", config.key_prefix);
        let queue_keys: Vec<String> = Cmd::keys(&queue_pattern)
            .query_async(&mut conn_manager)
            .await
            .map_err(JobError::from)?;

        // Get all schedule keys (ZSET)
        let schedule_pattern = format!("{}:*schedule:*", config.key_prefix);
        let schedule_keys: Vec<String> = Cmd::keys(&schedule_pattern)
            .query_async(&mut conn_manager)
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
