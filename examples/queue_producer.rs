use chrono::{Duration, Utc};
use liteq::{JobQueue, QueueConfig, RedisConfig};
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

    println!("Sending regular jobs...");
    for i in 1..=3 {
        let task = Task {
            id: i,
            text: format!("Process order immediately #{}", i),
        };

        let job_id = queue.enqueue(task).send().await?;

        println!("   ✓ Regular job {} → LIST: {}", i, job_id);
    }

    println!("Sending scheduled job (ETA +10 seconds)...");
    let task2 = Task {
        id: 2,
        text: "Send reminder email".to_string(),
    };
    let eta = Utc::now() + Duration::seconds(10);
    let job_id2 = queue.enqueue(task2).with_eta(eta).send().await?;
    println!("   ✓ Scheduled job → ZSET: {}", job_id2);

    println!("Sending scheduled job (ETA +5 seconds)...");
    let task3 = Task {
        id: 3,
        text: "Process payment".to_string(),
    };
    let eta_sooner = Utc::now() + Duration::seconds(5);
    let job_id3 = queue.enqueue(task3).with_eta(eta_sooner).send().await?;
    println!("   ✓ Scheduled job → ZSET: {}", job_id3);

    println!("Sending ready scheduled job (ETA -5 seconds)...");
    let task4 = Task {
        id: 4,
        text: "Urgent notification".to_string(),
    };
    let eta_past = Utc::now() - Duration::seconds(5);
    let job_id4 = queue.enqueue(task4).with_eta(eta_past).send().await?;
    println!("   ✓ Ready scheduled job → ZSET: {}\n", job_id4);

    println!("✅ All tasks enqueued!\n");

    Ok(())
}
