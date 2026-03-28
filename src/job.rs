use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job<T> {
    pub id: String,
    pub payload: T,
    pub queue: String,
    pub created_at: DateTime<Utc>,
    pub eta: Option<DateTime<Utc>>,
    pub status: JobStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum JobStatus {
    Pending,
    Scheduled,
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
            status: JobStatus::Pending,
        }
    }

    pub fn with_eta(mut self, eta: DateTime<Utc>) -> Self {
        self.eta = Some(eta);
        self.status = JobStatus::Scheduled;
        self
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error>
    where
        T: Serialize,
    {
        serde_json::to_string(self)
    }

    pub fn from_json(json: &str) -> Result<Self, serde_json::Error>
    where
        T: DeserializeOwned,
    {
        serde_json::from_str(json)
    }
}
