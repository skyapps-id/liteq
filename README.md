# lite-job-redis

High-performance job queue library for Rust using Redis with automatic retry and exponential backoff.

## Features

- **High Performance**: Optimized for high traffic with connection pooling and batch operations
- **Auto-Retry**: Automatic retry with exponential backoff when Redis connection fails
- **Resilient**: Continues working even when Redis is temporarily down
- **ETA Support**: Schedule jobs to run at specific times using Redis sorted sets
- **Configurable Retry**: Custom retry settings (attempts, delays, backoff)
- **Type Safe**: Full Rust type safety with generics
- **Async/Await**: Built on Tokio for efficient async processing
- **Simple API**: Easy to use with minimal setup

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
lite-job-redis = "0.1"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```

## Quick Start

### Producer (Sending Jobs)

```rust
use lite_job_redis::{Job, JobQueue, QueueConfig, RedisConfig, RetryConfig};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Task {
    id: u32,
    text: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configure Redis with retry settings
    let retry_config = RetryConfig::new()
        .with_max_attempts(10)
        .with_initial_delay(500)
        .with_max_delay(30000);

    let redis_config = RedisConfig::new("redis://127.0.0.1:6379");
    let queue_config = QueueConfig::new("my_queue");

    // Create queue with retry
    let queue = JobQueue::new(queue_config, redis_config)
        .await?
        .with_retry_config(retry_config);

    // Create and enqueue a job
    let task = Task {
        id: 1,
        text: "Hello from queue".to_string(),
    };

    let job = Job::new(task, "my_queue");
    let job_id = queue.enqueue(job).await?;
    
    println!("Job sent: {}", job_id);

    Ok(())
}
```

### Consumer (Processing Jobs)

```rust
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
    let retry_config = RetryConfig::new()
        .with_max_attempts(10)
        .with_initial_delay(500)
        .with_max_delay(30000);

    let redis_config = RedisConfig::new("redis://127.0.0.1:6379");
    let queue_config = QueueConfig::new("my_queue");

    let queue = JobQueue::new(queue_config, redis_config)
        .await?
        .with_retry_config(retry_config);

    println!("Waiting for jobs...");

    loop {
        if let Some(job) = queue.dequeue::<Task>().await? {
            println!("Received job: {} - {}", job.payload.id, job.payload.text);
        }

        sleep(Duration::from_millis(500)).await;
    }
}
```

## Auto-Retry System

The library automatically handles Redis connection failures with exponential backoff:

### What Happens When Redis Goes Down

```
⚠️  Redis operation failed (attempt 1/10): Broken pipe. Retrying in 500ms...
⚠️  Redis operation failed (attempt 2/10): Connection refused. Retrying in 1000ms...
⚠️  Redis operation failed (attempt 3/10): Connection refused. Retrying in 2000ms...
✅ Redis reconnected successfully! (attempt 4/10 recovered from: Broken pipe)
```

### Retry Configuration

```rust
let retry_config = RetryConfig::new()
    .with_max_attempts(10)      // Maximum retry attempts
    .with_initial_delay(500)    // Initial delay in milliseconds
    .with_max_delay(30000);     // Maximum delay in milliseconds

let queue = JobQueue::new(queue_config, redis_config)
    .await?
    .with_retry_config(retry_config);
```

### Default Values

- **Max Attempts**: 10 retries
- **Initial Delay**: 500ms
- **Max Delay**: 30 seconds
- **Backoff Multiplier**: 2x (exponential)

### Retryable Errors

✅ Broken pipe  
✅ Connection refused  
✅ Connection reset  
✅ Timeout  
✅ Multiplexed connection terminated  
✅ Driver unexpectedly terminated  

For more details, see [RETRY.md](RETRY.md)

## Examples

### Run Producer

```bash
cargo run --example queue_producer
```

Sends jobs to the queue. If Redis is down, it will automatically retry.

### Run Consumer

```bash
cargo run --example queue_consumer
```

Processes jobs from the queue. Auto-reconnects if Redis is down.

### Test Retry Logic

```bash
cargo run --example retry_test
```

Demonstrates retry behavior when Redis is restarted.

## Configuration

### Redis Configuration

```rust
let config = RedisConfig::new("redis://127.0.0.1:6379")
    .with_key_prefix("my-app");    // Key prefix for all keys
```

### Queue Configuration

```rust
let queue_config = QueueConfig::new("my_queue");
```

### Retry Configuration

```rust
let retry_config = RetryConfig::new()
    .with_max_attempts(20)         // More retries for critical systems
    .with_initial_delay(1000)      // Start with 1 second delay
    .with_max_delay(60000);        // Max delay 1 minute
```

## Job Features

### Retry Count

```rust
let job = Job::new(task, "my_queue")
    .with_retries(3);  // Retry job up to 3 times if processing fails
```

### Metadata

```rust
let job = Job::new(task, "my_queue")
    .with_metadata(serde_json::json!({
        "priority": "high",
        "category": "important"
    }));
```

### ETA Scheduling

```rust
let job = Job::new(task, "my_queue")
    .with_eta(Utc::now() + chrono::Duration::hours(2));
```

## Architecture

- **Queue**: Redis list for pending jobs
- **Retry Logic**: Exponential backoff with configurable attempts
- **Connection Management**: Fresh connection per retry attempt
- **Type Safety**: Generic-based type system

## Benefits

🚀 **Resilient** - Continues working when Redis is temporarily down  
⏱️ **Auto-Recovery** - Automatically reconnects when Redis comes back  
📊 **Observable** - Retry attempts logged with `tracing`  
🎯 **Flexible** - Configure retry behavior to your needs  
💾 **Persistent** - Jobs stored in Redis even if consumer is down  

## Testing Retry Logic

**Terminal 1** - Run consumer:
```bash
cargo run --example queue_consumer
```

**Terminal 2** - Run producer:
```bash
cargo run --example queue_producer
```

**Terminal 3** - Stop/start Redis:
```bash
# Stop Redis
redis-cli shutdown

# Wait a few seconds, then start Redis
redis-server
```

You'll see the retry logs showing automatic reconnection!

## Documentation

- [README.md](README.md) - Getting started guide
- [RETRY.md](RETRY.md) - Retry system documentation

## License

MIT

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.
