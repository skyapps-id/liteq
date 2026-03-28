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

    let queue_config = QueueConfig::new("orders");
    let queue = JobQueue::new(queue_config, config).await?;

    println!("🚀 Producer Demo - ZSET Optimization\n");

    println!("Scenario: Mix of regular and scheduled jobs\n");

    println!("1️⃣  Sending 10 regular jobs (no ETA)...");
    for i in 1..=3 {
        let task = Task {
            id: i,
            text: format!("Process order immediately #{}", i),
        };

        let job = Job::new(task, "orders");
        let job_id = queue.enqueue(job).await?;

        println!("   ✓ Regular job {} → LIST: {}", i, job_id);
    }

    /*  println!("2️⃣  Sending scheduled job (ETA +10 seconds)...");
    let task2 = Task {
        id: 2,
        text: "Send reminder email".to_string(),
    };
    let eta = Utc::now() + Duration::seconds(10);
    let job2 = Job::new(task2, "orders").with_eta(eta);
    let job_id2 = queue.enqueue(job2).await?;
    println!("   ✓ Scheduled job → ZSET: {}", job_id2);
    println!("   ⏰ ETA: 10 seconds from now\n"); */

    /* println!("3️⃣  Sending another scheduled job (ETA +5 seconds, ready sooner)...");
    let task3 = Task {
        id: 3,
        text: "Process payment".to_string(),
    };
    let eta_sooner = Utc::now() + Duration::seconds(5);
    let job3 = Job::new(task3, "orders").with_eta(eta_sooner);
    let job_id3 = queue.enqueue(job3).await?;
    println!("   ✓ Scheduled job → ZSET: {}", job_id3);
    println!("   ⏰ ETA: 5 seconds from now\n"); */

    /* println!("4️⃣  Sending ready scheduled job (ETA -5 seconds, should process first)...");
    let task4 = Task {
        id: 4,
        text: "Urgent notification".to_string(),
    };
    let eta_past = Utc::now() - Duration::seconds(5);
    let job4 = Job::new(task4, "orders").with_eta(eta_past);
    let job_id4 = queue.enqueue(job4).await?;
    println!("   ✓ Ready scheduled job → ZSET: {}", job_id4);
    println!("   ⏰ ETA: Past (ready now)\n"); */

    println!("💡 ZSET Optimization:");
    println!("   ✓ No more roundtrips: LPOP → parse → RPUSH back");
    println!("   ✓ Single atomic Lua script operation");
    println!("   ✓ Scheduled jobs in ZSET, regular jobs in LIST");
    println!("   ✓ Priority: Ready scheduled > Regular > Future scheduled\n");

    println!("📊 Job Processing Order Expected:");
    println!("   1. Job #4 (Ready scheduled - ETA past)");
    println!("   2. Job #1 (Regular - no ETA)");
    println!("   3. Job #3 (Scheduled - ETA +5s)");
    println!("   4. Job #2 (Scheduled - ETA +10s)\n");

    println!("✅ All tasks enqueued!");
    println!("   Run consumer to see them processed in order\n");

    Ok(())
}
