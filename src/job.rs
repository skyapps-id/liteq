use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Generate a unique job ID without UUID
/// Format: {timestamp_ms}-{random_u32}
/// Example: "1712345678901-1234567890"
fn generate_job_id() -> String {
    let timestamp_ms = Utc::now().timestamp_millis();
    let random: u32 = rand::random();
    format!("{}-{}", timestamp_ms, random)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job<T> {
    pub id: String,
    pub payload: T,
    pub queue: String,
    pub eta: Option<DateTime<Utc>>,
}

impl<T> Job<T> {
    /// Creates new job with payload
    pub fn new(payload: T) -> Self
    where
        T: Serialize,
    {
        Self {
            id: generate_job_id(),
            payload,
            queue: String::new(),
            eta: None,
        }
    }

    /// Sets scheduled execution time (ETA)
    pub fn with_eta(mut self, eta: DateTime<Utc>) -> Self {
        self.eta = Some(eta);
        self
    }

    /// Serializes job to JSON
    pub fn to_json(&self) -> Result<String, serde_json::Error>
    where
        T: Serialize,
    {
        serde_json::to_string(self)
    }

    /// Deserializes job from JSON
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error>
    where
        T: DeserializeOwned,
    {
        serde_json::from_str(json)
    }
}
