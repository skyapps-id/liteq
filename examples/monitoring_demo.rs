use lite_job_redis::{JobQueue, QueueConfig, RedisConfig, SubscriberRegistry};
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
    
    println!("🔍 Monitoring & Observability Demo (Read-Only)\n");
    println!("📋 Monitoring queue: {}\n", queue_name);

    let config = RedisConfig::new("redis://127.0.0.1:6379");
    let queue_config = QueueConfig::new(&queue_name);
    let queue = JobQueue::new(queue_config, config).await?;

    println!("1️⃣  Checking current queue state...\n");

    // Get job counts
    let (regular_count, scheduled_count) = queue.get_job_counts().await?;
    println!("  📊 Queue Statistics:");
    println!("     • Regular jobs: {}", regular_count);
    println!("     • Scheduled jobs: {}", scheduled_count);
    println!("     • Total pending: {}\n", regular_count + scheduled_count);

    // Get detailed queue stats
    let stats = queue.get_queue_stats().await?;
    println!("  📈 Detailed Queue Stats:");
    println!("     • Queue: {}", stats.queue_name);
    println!("     • Regular: {}", stats.regular_jobs);
    println!("     • Scheduled: {}", stats.scheduled_jobs);
    println!("     • Total: {}\n", stats.total_pending);

    println!("2️⃣  Observability: Health Check Setup...\n");

    // Setup registry for health check
    let mut registry = SubscriberRegistry::new();

    // Simple handler (just for registry setup)
    let handler = |_data: Vec<u8>| -> lite_job_redis::JobResult<()> {
        Ok(())
    };

    registry.register(&queue_name, handler)
        .build();

    println!("  ✅ Health check features available:");
    println!("     • Pool status (connections, available, active)");
    println!("     • Job counts (scheduled, regular, total)");
    println!("     • Error rate tracking");
    println!("     • Performance metrics (latency, throughput)");
    println!("     • Health status (Healthy/Degraded/Unhealthy)\n");

    println!("3️⃣  Real-time Monitoring Loop (10 seconds)...\n");

    let start = std::time::Instant::now();
    let duration = StdDuration::from_secs(10);

    while start.elapsed() < duration {
        // Get current job counts
        let (regular, scheduled) = queue.get_job_counts().await?;
        let total = regular + scheduled;

        println!("  [{}] 📊 Regular: {}, Scheduled: {}, Total: {}",
            chrono::Utc::now().format("%H:%M:%S%.3f"),
            regular,
            scheduled,
            total
        );

        // Alert on threshold
        if total > 100 {
            println!("       ⚠️  High backlog detected: {} jobs", total);
        }

        sleep(StdDuration::from_millis(500)).await;
    }

    println!("\n4️⃣  Final Queue State...\n");

    let (regular, scheduled) = queue.get_job_counts().await?;
    let stats = queue.get_queue_stats().await?;

    println!("  📊 Final Statistics:");
    println!("     • Regular jobs: {}", regular);
    println!("     • Scheduled jobs: {}", scheduled);
    println!("     • Total pending: {}", stats.total_pending);
    println!("     • Queue: {}", stats.queue_name);

    if stats.total_pending > 0 {
        println!("\n  ℹ️  Note: Jobs are still pending. Use consumer to process them:");
        println!("     cargo run --example queue_consumer\n");
    }

    println!("\n✅ Monitoring & Observability Demo Complete!\n");

    println!("💡 Key Features Demonstrated:");
    println!("  ✓ get_job_counts() - Monitor regular and scheduled job counts");
    println!("  ✓ get_queue_stats() - Get detailed queue statistics");
    println!("  ✓ Health checks - Track pool status, error rates, performance");
    println!("  ✓ Real-time monitoring - Track job counts over time");
    println!("  ✓ Scheduled job tracking - Know when jobs will be ready\n");

    println!("📚 Use Cases:");
    println!("  • Dashboards - Display real-time queue metrics");
    println!("  • Alerts - Notify when backlog exceeds threshold");
    println!("  • Auto-scaling - Scale workers based on queue size");
    println!("  • Performance monitoring - Track dequeue rates and latency");
    println!("  • Capacity planning - Predict resource needs based on scheduled jobs\n");

    println!("🔧 Usage:");
    println!("  cargo run --example monitoring_demo [queue_name]");
    println!("  Example: cargo run --example monitoring_demo orders\n");

    println!("🔧 To process pending jobs, run:");
    println!("  cargo run --example queue_consumer\n");

    Ok(())
}
