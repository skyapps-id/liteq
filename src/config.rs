use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedisConfig {
    pub url: String,
    pub pool_size: usize,
    pub min_idle: usize,
    pub key_prefix: String,
}

impl RedisConfig {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            pool_size: 20,
            min_idle: 2,
            key_prefix: "lite-job".to_string(),
        }
    }

    pub fn with_pool_size(mut self, size: usize) -> Self {
        self.pool_size = size;
        self
    }

    pub fn with_min_idle(mut self, min_idle: usize) -> Self {
        self.min_idle = min_idle;
        self
    }

    pub fn make_key(&self, key: &str) -> String {
        format!("{}:{}", self.key_prefix, key)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueConfig {
    pub name: String,
}

impl QueueConfig {
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into() }
    }
}
