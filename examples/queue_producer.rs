use chrono::{Duration, Utc};
use lite_job_redis::{Job, JobQueue, QueueConfig, RedisConfig};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Task {
    id: u32,
    text: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let config = RedisConfig::new("redis://127.0.0.1:6379");

    let queue_config = QueueConfig::new("my_queue");
    let queue = JobQueue::new(queue_config, config).await?;

    println!("Sending 1 task to queue 'my_queue'...\n");

    let task = Task {
        id: 1,
        text: "Hello this is task from queue".to_string(),
    };

    let eta = Utc::now() + Duration::seconds(10);
    let job = Job::new(task, "my_queue").with_eta(eta);
    let job_id = queue.enqueue(job).await?;
    
    println!("✓ Task sent!");
    println!("  Job ID: {}", job_id);
    println!("  Queue: my_queue");

    println!("\n✅ Done! Task stored in queue.");

    Ok(())
}
