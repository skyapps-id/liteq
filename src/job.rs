use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job<T> {
    pub id: String,
    pub payload: T,
    pub queue: String,
    pub created_at: DateTime<Utc>,
    pub eta: Option<DateTime<Utc>>,
    pub retries: u32,
    pub max_retries: u32,
    pub retry_count: u32,
    pub metadata: serde_json::Value,
    pub status: JobStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum JobStatus {
    Pending,
    Scheduled,
    Processing,
    Completed,
    Failed,
    Retrying,
}

impl<T> Job<T> {
    pub fn new(payload: T, queue: impl Into<String>) -> Self
    where
        T: Serialize,
    {
        Self {
            id: Uuid::new_v4().to_string(),
            payload,
            queue: queue.into(),
            created_at: Utc::now(),
            eta: None,
            retries: 0,
            max_retries: 3,
            retry_count: 0,
            metadata: serde_json::json!({}),
            status: JobStatus::Pending,
        }
    }

    pub fn with_eta(mut self, eta: DateTime<Utc>) -> Self {
        self.eta = Some(eta);
        self.status = JobStatus::Scheduled;
        self
    }

    pub fn with_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = metadata;
        self
    }

    pub fn with_queue(mut self, queue: impl Into<String>) -> Self {
        self.queue = queue.into();
        self
    }

    pub fn get_metadata(&self, key: &str) -> Option<&serde_json::Value> {
        self.metadata.get(key)
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error>
    where
        T: Serialize,
    {
        serde_json::to_string(self)
    }

    pub fn from_json(json: &str) -> Result<Self, serde_json::Error>
    where
        T: serde::de::DeserializeOwned,
    {
        serde_json::from_str(json)
    }
}
