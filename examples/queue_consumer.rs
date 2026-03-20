use lite_job_redis::{JobQueue, QueueConfig, RedisConfig, RetryConfig};
use serde::{Deserialize, Serialize};
use tokio::time::{sleep, Duration};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Task {
    id: u32,
    text: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let config = RedisConfig::new("redis://127.0.0.1:6379");

    // Setup queue with custom retry config (optional)
    // Default: 10 attempts, 500ms initial delay, 30s max delay
    let retry_config = RetryConfig::new()
        .with_max_attempts(20)
        .with_initial_delay(500)
        .with_max_delay(30000);

    let queue_config = QueueConfig::new("my_queue");
    let queue = JobQueue::new(queue_config, config)
        .await?
        .with_retry_config(retry_config);

    println!("Waiting for tasks from queue 'my_queue'...");
    println!("Press Ctrl+C to stop");

    loop {
        // Try to dequeue job
        if let Some(job) = queue.dequeue::<Task>().await? {
            println!("\n🎉 TASK RECEIVED!");
            println!("   Job ID: {}", job.id);
            println!("   Queue: my_queue");
            println!("   ID: {}", job.payload.id);
            println!("   Text: {}", job.payload.text);
            println!("   Retries: {}", job.retries);
            println!();
        }

        // Wait a bit before checking again
        sleep(Duration::from_millis(500)).await;
    }
}
