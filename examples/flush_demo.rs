use lite_job_redis::{JobQueue, QueueConfig, RedisConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🧹 Flush Demo - Clear Queue Jobs\n");

    let config = RedisConfig::new("redis://127.0.0.1:6379");
    let queue_config = QueueConfig::new("monitored_queue");
    let queue = JobQueue::new(queue_config, config).await?;

    // Check before flush
    println!("1️⃣  Before flush:");
    let (regular, scheduled) = queue.get_job_counts().await?;
    println!("   Regular jobs: {}", regular);
    println!("   Scheduled jobs: {}", scheduled);
    println!("   Total pending: {}\n", regular + scheduled);

    // Flush the queue
    println!("2️⃣  Flushing queue...");
    queue.flush().await?;
    println!("   ✅ Queue flushed!\n");

    // Check after flush
    println!("3️⃣  After flush:");
    let (regular, scheduled) = queue.get_job_counts().await?;
    println!("   Regular jobs: {}", regular);
    println!("   Scheduled jobs: {}", scheduled);
    println!("   Total pending: {}\n", regular + scheduled);

    if regular == 0 && scheduled == 0 {
        println!("✅ Success! Queue is now empty.");
    } else {
        println!("⚠️  Warning: Queue still has jobs.");
    }

    Ok(())
}
