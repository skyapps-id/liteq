use liteq::{JobQueue, QueueConfig, RedisConfig};
use tokio::time::{sleep, Duration};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Task {
    id: u32,
    text: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let config = RedisConfig::new("redis://127.0.0.1:6379");

    println!("=== Retry Test ===\n");
    println!("Try stopping Redis to see retry attempts");
    println!("Then start Redis, will auto-reconnect\n");

    let queue_config = QueueConfig::new("test_queue");
    let queue = JobQueue::new(queue_config, config).await?;

    println!("Sending 3 tasks with delays between them...\n");

    for i in 1..=3 {
        let task = Task {
            id: i,
            text: format!("Task #{}", i),
        };

        
        println!("Sending task #{}...", i);
        match queue.enqueue(task).send().await {
            Ok(job_id) => println!("✓ Task #{} sent! Job ID: {}\n", i, job_id),
            Err(e) => println!("✗ Task #{} failed: {}\n", i, e),
        }

        // Wait 3 seconds to give time to stop/start Redis
        sleep(Duration::from_secs(3)).await;
    }

    println!("✅ Done!");

    Ok(())
}
