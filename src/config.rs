use once_cell::sync::OnceCell;
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;

static REDIS_SEMAPHORE: OnceCell<Semaphore> = OnceCell::new();

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedisConfig {
    pub url: String,
    pub pool_size: usize,
    pub cluster_mode: bool,
    pub key_prefix: String,
    pub job_ttl: u64,
    pub health_check_interval: u64,
}

impl RedisConfig {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            pool_size: 20,
            cluster_mode: false,
            key_prefix: "lite-job".to_string(),
            job_ttl: 86400,
            health_check_interval: 30,
        }
    }

    pub fn with_pool_size(mut self, size: usize) -> Self {
        self.pool_size = size;
        self
    }

    pub fn with_cluster_mode(mut self, enabled: bool) -> Self {
        self.cluster_mode = enabled;
        self
    }

    pub fn with_key_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.key_prefix = prefix.into();
        self
    }

    pub fn with_job_ttl(mut self, ttl: u64) -> Self {
        self.job_ttl = ttl;
        self
    }

    pub fn get_semaphore(&self) -> &'static Semaphore {
        REDIS_SEMAPHORE.get_or_init(|| Semaphore::new(self.pool_size))
    }

    pub fn make_key(&self, key: &str) -> String {
        format!("{}:{}", self.key_prefix, key)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueConfig {
    pub name: String,
    pub priority: i32,
    pub max_concurrent: usize,
    pub poll_interval: u64,
}

impl QueueConfig {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            priority: 0,
            max_concurrent: 100,
            poll_interval: 1000,
        }
    }

    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    pub fn with_max_concurrent(mut self, max: usize) -> Self {
        self.max_concurrent = max;
        self
    }

    pub fn with_poll_interval(mut self, interval: u64) -> Self {
        self.poll_interval = interval;
        self
    }
}
