# liteq

High-performance job queue library for Rust using Redis with auto-retry, connection pooling, dependency injection, and multi-consumer fair distribution.

## Features

- **🚀 High Performance**: ZSET optimization for scheduled jobs, 50% reduction in network roundtrips
- **⚡ Auto-Scaling**: Multi-consumer fair distribution with auto-registration and heartbeat
- **🔄 Auto-Retry**: Automatic retry with exponential backoff when Redis connection fails
- **🛡️ Resilient**: RabbitMQ-style connection supervisor - continues working even when Redis is temporarily down
- **🔌 Persistent Connections**: ConnectionManager with configurable timeouts (30s connection, 20s response)
- **📡 PubSub Auto-Reconnect**: Redis PubSub consumers automatically reconnect after connection loss
- **💉 Dependency Injection**: Built-in support for injecting shared state into job handlers via `.data()`
- **📊 Monitoring**: Health checks, job counts, and queue statistics
- **📝 Structured Logging**: Uses `tracing` for structured, production-ready logging
- **🔒 Type Safe**: Full Rust type safety with generics
- **⚙️ Async/Await**: Built on Tokio for efficient async processing
- **🎨 Builder Pattern**: Fluent API for configuring queues and workers

## Installation

Add to your `Cargo.toml`:

```toml
[dependencies]
liteq = "1.1"
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tracing = "0.1"
tracing-subscriber = "0.3"
chrono = "0.4"
```

## Tested Environments

✅ **Production-Tested** with multiple Redis providers:

| Provider | Type | Status | Notes |
|----------|------|--------|-------|
| **Self-hosted** | Redis 7.x | ✅ Fully tested | Local & Docker deployments |
| **Upstash** | Redis Cloud | ✅ Fully tested | Recommended for serverless |
| **Aiven** | Valkey 7.x | ✅ Fully tested | Use default 30s/20s timeouts |

**Configuration Recommendations:**

```rust
// Self-hosted Redis (local/Docker)
RedisConfig::new("redis://127.0.0.1:6379")
    .with_connection_timeout(5)
    .with_response_timeout(3);

// Upstash (Redis Cloud)
RedisConfig::new("rediss://default:password@your-redis.upstash.io:6379")
    .with_connection_timeout(30)  // Default, works well
    .with_response_timeout(20);

// Aiven Valkey (Redis-compatible)
RedisConfig::new("rediss://user:pass@aiven-valkey.aivencloud.com:6379")
    .with_connection_timeout(30)  // Default, optimized for cloud
    .with_response_timeout(20);
```

## Quick Start

### Producer (Send Jobs with ETA Scheduling)

```rust
use liteq::{JobQueue, QueueConfig, RedisConfig};
use chrono::Utc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let queue = JobQueue::new(
        QueueConfig::new("orders"),
        RedisConfig::new("redis://127.0.0.1:6379")
    ).await?;

    // Regular job (process immediately)
    queue.enqueue(
        serde_json::json!({"order_id": 123, "item": "widget"})
    ).send().await?;

    // Scheduled job (process in 2 hours)
    queue.enqueue(
        serde_json::json!({"order_id": 124, "item": "widget"})
    ).with_eta(Utc::now() + chrono::Duration::hours(2))
    .send().await?;

    Ok(())
}
```

### Consumer (Process Jobs with Multi-Instance Support)

```rust
use liteq::{JobResult, SubscriberRegistry};
use std::sync::Arc;

#[derive(Clone)]
struct AppData {
    db_url: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut registry = SubscriberRegistry::new();
    let app_data = Arc::new(AppData {
        db_url: "postgresql://localhost:5432/mydb".to_string(),
    });

    // Auto-registers and enables fair distribution!
    registry.register("orders", handle_orders)
        .with_data(app_data)
        .with_pool_size(20)
        .with_concurrency(5)
        .build();

    registry.run().await?;
    Ok(())
}

fn handle_orders(data: Vec<u8>, app_data: Arc<AppData>) -> JobResult<()> {
    // Deserialize payload directly
    let order: serde_json::Value = serde_json::from_slice(&data)?;
    println!("Processing order: {}", order);

    Ok(())
}
```

**Run Multiple Instances for Fair Distribution:**
```bash
# Terminal 1
cargo run --example queue_consumer

# Terminal 2
cargo run --example queue_consumer

# Terminal 3
cargo run --example queue_producer
```

Jobs are automatically distributed 50:50 between instances! 🎯

## Multi-Consumer Fair Distribution

**Run multiple consumer instances for automatic fair job distribution!**

Each consumer instance auto-registers with Redis and gets a unique ID. Jobs are distributed using **modulo arithmetic** for perfect round-robin:

```bash
# Terminal 1
cargo run --example queue_consumer

# Terminal 2
cargo run --example queue_consumer
```

**Features:**
- ✅ **Auto-Registration**: Zero configuration needed
- ✅ **Fair Distribution**: Perfect 50:50 (or 33:33:33, etc) split
- ✅ **Heartbeat**: 30s TTL, auto-failure detection
- ✅ **Auto-Recovery**: Consumer crash → jobs redistributed automatically
- ✅ **No Starvation**: Every instance gets equal job share

**How It Works:**
```
Job 1 → Consumer 1 (1 % 2 = 1)
Job 2 → Consumer 0 (2 % 2 = 0)
Job 3 → Consumer 1 (3 % 2 = 1)
Job 4 → Consumer 0 (4 % 2 = 0)
Perfect round-robin!
```

For details, see [MULTI_CONSUMER_FLOW.md](MULTI_CONSUMER_FLOW.md)

## ETA Scheduling with ZSET Optimization

**Schedule jobs to run at specific times with zero performance penalty!**

```rust
use chrono::Utc;

// Process this job in 2 hours
queue.enqueue(task)
    .with_eta(Utc::now() + chrono::Duration::hours(2))
    .send().await?;
```

**Benefits:**
- ✅ **50% Reduction** in network roundtrips for scheduled jobs
- ✅ **40% Reduction** in CPU usage (no redundant JSON parsing)
- ✅ **No Job Bouncing**: Jobs stay in Redis until ready
- ✅ **Priority Queue**: Ready scheduled jobs processed before regular jobs

**How It Works:**
- Jobs with ETA → Stored in ZSET (score = ETA timestamp)
- Jobs without ETA → Stored in LIST (process immediately)
- Dequeue script checks ZSET first (ready ETA jobs), then LIST

For details, see [PERFORMANCE_IMPROVEMENTS.md](PERFORMANCE_IMPROVEMENTS.md)

## Redis PubSub with Auto-Reconnect

**Publish/Subscribe messaging that survives Redis restarts!**

```rust
use liteq::{RedisPubSub, RedisConfig};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct Event {
    message: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = RedisConfig::new("redis://127.0.0.1:6379");
    let pubsub = RedisPubSub::new(config).await?;

    // Subscribe with auto-reconnect on connection loss
    pubsub.subscribe(
        vec!["events".to_string()],
        |channel, event: Event| {
            println!("Received on {}: {:?}", channel, event);
        }
    ).await?;

    Ok(())
}
```

**Features:**
- ✅ **Auto-Reconnect**: Automatically reconnects after Redis disconnect (5s delay)
- ✅ **Auto Re-Subscribe**: Re-subscribes to all channels after reconnection
- ✅ **Persistent**: Continues trying to reconnect until successful
- ✅ **Non-Blocking**: Runs in background task, returns immediately
- ✅ **Type-Safe**: Generic types with automatic deserialization

**Run PubSub reconnect test:**
```bash
# Terminal 1: Start subscriber
cargo run --example test_pubsub_reconnect

# Terminal 2: Test auto-reconnect
redis-cli PUBLISH lite-job:test_channel '{"text":"Hello"}'
redis-cli shutdown  # Stop Redis
redis-server      # Start Redis
redis-cli PUBLISH lite-job:test_channel '{"text":"Still works!"}'  # ✅ Consumer receives it!
```

## Health Checks & Monitoring

**Monitor queue health and job counts in real-time:**

```rust
// Get job counts
let (regular, scheduled) = queue.get_job_counts().await?;

// Get detailed statistics
let stats = queue.get_queue_stats().await?;
println!("Total pending: {}", stats.total_pending);

// Health check
let health = registry.health_check("orders").await?;
println!("Status: {:?}", health.status);
```

**Run monitoring demo:**
```bash
cargo run --example monitoring_demo orders
```

For details, see [MONITORING.md](MONITORING.md)

## Dependency Injection

Inject shared state into your job handlers:

```rust
registry.register("queue_name", handler)
    .with_data(shared_state)
    .with_pool_size(10)
    .with_concurrency(3)
    .build();
```

The `data` is passed as `Arc<T>` to your handler function, making it easy to share:
- Database connections
- API clients
- Configuration
- Counters/metrics

## Connection Supervision & Auto-Retry

The library automatically handles Redis connection failures with RabbitMQ-style supervision:

**Features:**
- ✅ **Persistent Retry**: Continues retrying until Redis comes back (20 retries with exponential backoff)
- ✅ **Configurable Timeouts**: 30s connection timeout, 20s response timeout (prevents hangs)
- ✅ **Worker Coordination**: Workers wait for "ready" signal before processing
- ✅ **Clean Logs**: Single reconnection log (not N×workers spamming)
- ✅ **Auto-Recovery**: Reconnect → all workers notified simultaneously
- ✅ **Exponential Backoff**: Smart backoff after failures (1s → 60s max)

**Default connection settings:**
- **Connection Timeout**: 30 seconds (time to establish connection)
- **Response Timeout**: 20 seconds (time for Redis operations)
- **Max Retries**: 20 attempts
- **Retry Delay**: 1s initial, up to 60s (exponential backoff)
- **Check Interval**: 5 seconds (health check frequency)

**Customize timeouts:**
```rust
RedisConfig::new("redis://127.0.0.1:6379")
    .with_connection_timeout(15)  // 15 seconds
    .with_response_timeout(10);   // 10 seconds
```

**What happens when Redis goes down:**
```
Redis Disconnect → Detection (5s)
    ↓
Attempt Reconnect (with exponential backoff)
    ↓
Redis Reconnect → Notify all workers
    ↓
Resume normal operations ✅
```

## Builder Pattern

Configure queues with the fluent builder API:

```rust
SubscriberRegistry::new()
    .with_redis("redis://127.0.0.1:6379")
    .register("queue1", handler1)
        .with_pool_size(20)
        .with_concurrency(5)
        .build()
    .register("queue2", handler2)
        .with_data(shared_data)
        .with_pool_size(10)
        .build();
```

Available builder methods:
- `.with_redis(url)` - Set Redis connection URL
- `.register(queue, handler)` - Register a handler
- `.with_data(data)` - Inject shared state (optional)
- `.with_pool_size(size)` - Set connection pool size
- `.with_concurrency(num)` - Set number of concurrent workers
- `.with_min_idle(num)` - Set minimum idle connections
- `.build()` - Complete registration

## Examples

```bash
# Run producer (send jobs)
cargo run --example queue_producer

# Run consumer (process jobs)
cargo run --example queue_consumer

# Monitor queue stats
cargo run --example monitoring_demo orders

# Test PubSub auto-reconnect
cargo run --example test_pubsub_reconnect
```

**Testing Multiple Instances:**
```bash
# Terminal 1
cargo run --example queue_consumer

# Terminal 2
cargo run --example queue_consumer

# Terminal 3
cargo run --example queue_producer
```

You'll see fair 50:50 distribution! 🎯

**Testing Redis Reconnect:**
```bash
# Terminal 1: Run consumer
cargo run --example queue_consumer

# Terminal 2: Stop Redis
redis-cli shutdown

# Terminal 3: Watch auto-reconnect logs
# Will show: "Reconnection successful after X failures"

# Terminal 2: Start Redis
redis-server

# Terminal 1: Consumer resumes automatically ✅
```

## Architecture

- **ZSET**: Scheduled jobs with ETA (score = timestamp)
- **LIST**: Regular jobs (FIFO)
- **ConnectionManager**: Persistent connections with auto-reconnect and configurable timeouts
- **Connection Supervisor**: RabbitMQ-style supervision with exponential backoff retry
- **PubSub Auto-Reconnect**: Automatic reconnection and re-subscription on connection loss
- **Worker Management**: Multiple workers per queue with configurable concurrency
- **Multi-Consumer**: Auto-registration with heartbeat for fair distribution
- **Retry Logic**: Exponential backoff with configurable attempts and jitter
- **Structured Logging**: All operations logged with tracing
- **Dependency Injection**: Per-queue shared state via `.data()`

## Benefits

🚀 **High Performance** - ZSET optimization reduces roundtrips by 50%
⚡ **Auto-Scaling** - Multi-consumer fair distribution with zero config
🔄 **Resilient** - Survives Redis restarts with automatic reconnection
🔌 **Persistent** - ConnectionManager with 30s/20s timeouts prevents connection hangs
📡 **Reactive** - PubSub consumers auto-reconnect and resume after Redis restarts
📊 **Observable** - Health checks and monitoring with tracing
🎯 **Flexible** - Configure per-queue pool sizes, worker counts, and timeouts
🔒 **Type Safe** - Full type safety with generics and Arc<T> for shared state
💾 **Persistent** - Jobs stored in Redis even if consumer is down  

## Documentation

- [CHANGELOG.md](CHANGELOG.md) - Version history and changes
- [README.md](README.md) - Getting started guide
- [PRODUCTION_DEPLOYMENT.md](PRODUCTION_DEPLOYMENT.md) - Production deployment with Upstash, Aiven Valkey
- [MULTI_CONSUMER_FLOW.md](MULTI_CONSUMER_FLOW.md) - Multi-consumer fair distribution
- [PERFORMANCE_IMPROVEMENTS.md](PERFORMANCE_IMPROVEMENTS.md) - ZSET optimization details
- [MONITORING.md](MONITORING.md) - Health checks and monitoring guide
- [CONNECTION_SUPERVISION.md](CONNECTION_SUPERVISION.md) - Connection supervision details
- [PUBSUB_AUTORECONNECT.md](PUBSUB_AUTORECONNECT.md) - PubSub auto-reconnect feature
- [IMPLEMENTATION_COMPLETE.md](IMPLEMENTATION_COMPLETE.md) - Implementation details

## License

MIT

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.
