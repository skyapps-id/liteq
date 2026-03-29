use liteq::{JobQueue, QueueConfig, RedisConfig, SubscriberRegistry};
use std::env;
use std::time::Duration as StdDuration;
use tokio::time::sleep;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    // Get queue name from command line argument, or use default
    let args: Vec<String> = env::args().collect();
    let queue_name = if args.len() > 1 {
        args[1].clone()
    } else {
        "orders".to_string() // default queue name
    };
    
    println!("🔍 Monitoring Demo: {}\n", queue_name);

    let config = RedisConfig::new("redis://127.0.0.1:6379");
    let queue_config = QueueConfig::new(&queue_name);
    let queue = JobQueue::new(queue_config, config).await?;

    let (regular_count, scheduled_count) = queue.get_job_counts().await?;
    let stats = queue.get_queue_stats().await?;

    println!("📊 Queue Stats:");
    println!("   Regular: {}, Scheduled: {}, Total: {}\n", 
        regular_count, scheduled_count, stats.total_pending);

    let mut registry = SubscriberRegistry::new();
    let handler = |_data: Vec<u8>| -> liteq::JobResult<()> {
        Ok(())
    };

    registry.register(&queue_name, handler).build();

    println!("⏳ Real-time monitoring (10 seconds)...\n");

    let start = std::time::Instant::now();
    let duration = StdDuration::from_secs(10);

    while start.elapsed() < duration {
        let (regular, scheduled) = queue.get_job_counts().await?;
        let total = regular + scheduled;

        println!("  [{}] Regular: {}, Scheduled: {}, Total: {}",
            chrono::Utc::now().format("%H:%M:%S%.3f"),
            regular, scheduled, total
        );

        if total > 100 {
            println!("       ⚠️  High backlog: {} jobs", total);
        }

        sleep(StdDuration::from_millis(500)).await;
    }

    println!("\n✅ Monitoring complete!\n");

    Ok(())
}
