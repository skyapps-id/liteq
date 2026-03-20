use lite_job_redis::{Job, JobQueue, QueueConfig, RedisConfig, RetryConfig};
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

    println!("=== Queue Producer ===\n");

    // Setup queue with custom retry config (optional)
    // Default: 10 attempts, 500ms initial delay, 30s max delay
    let retry_config = RetryConfig::new()
        .with_max_attempts(10)
        .with_initial_delay(500)
        .with_max_delay(30000);

    let queue_config = QueueConfig::new("my_queue");
    let queue = JobQueue::new(queue_config, config)
        .await?
        .with_retry_config(retry_config);

    println!("Sending 1 task to queue 'my_queue'...\n");
    println!("💡 If Redis is down, will auto-retry up to 10x with exponential backoff\n");

    let task = Task {
        id: 1,
        text: "Hello this is task from queue".to_string(),
    };

    let job = Job::new(task, "my_queue");
    let job_id = queue.enqueue(job).await?;
    
    println!("✓ Task sent!");
    println!("  Job ID: {}", job_id);
    println!("  Queue: my_queue");

    println!("\n✅ Done! Task stored in queue.");
    println!("   (Consumer will process task even if Redis is temporarily down)");
    println!("\n💡 Tips:");
    println!("   - Stop Redis to test auto-retry");
    println!("   - Start Redis before 10 attempts expire");
    println!("   - Check logs to see retry attempts");

    Ok(())
}
