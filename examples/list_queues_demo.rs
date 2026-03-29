use liteq::{JobQueue, RedisConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🔍 List All Queues Demo\n");

    let config = RedisConfig::new("redis://127.0.0.1:6379");

    println!("1️⃣  List all queues in Redis:\n");
    
    let queues = JobQueue::list_all(&config).await?;
    
    if queues.is_empty() {
        println!("  ℹ️  No queues found in Redis\n");
        println!("💡 Run producer first to create queues:");
        println!("   cargo run --example queue_producer\n");
        return Ok(());
    }
    
    println!("  Found {} queue(s):\n", queues.len());
    for (idx, queue_name) in queues.iter().enumerate() {
        println!("   {}. {}", idx + 1, queue_name);
    }

    println!("\n2️⃣  Get detailed stats for all queues:\n");
    
    let all_stats = JobQueue::get_all_queue_stats(&config).await?;
    
    for stats in &all_stats {
        println!("  📦 Queue: {}", stats.queue_name);
        println!("     • Regular jobs: {}", stats.regular_jobs);
        println!("     • Scheduled jobs: {}", stats.scheduled_jobs);
        println!("     • Total pending: {}\n", stats.total_pending);
    }

    println!("3️⃣  Summary:\n");
    
    let total_regular: usize = all_stats.iter().map(|s| s.regular_jobs).sum();
    let total_scheduled: usize = all_stats.iter().map(|s| s.scheduled_jobs).sum();
    let total_all: usize = all_stats.iter().map(|s| s.total_pending).sum();
    
    println!("  📊 Total Across All Queues:");
    println!("     • Regular jobs: {}", total_regular);
    println!("     • Scheduled jobs: {}", total_scheduled);
    println!("     • Total pending: {}\n", total_all);

    println!("✅ List All Queues Demo Complete!\n");

    println!("💡 Usage:");
    println!("  • Monitoring dashboard - See all queues at once");
    println!("  • Debugging - Check what queues exist");
    println!("  • Health check - Get status of all queues");
    println!("  • Capacity planning - Overview of all queues\n");

    Ok(())
}
