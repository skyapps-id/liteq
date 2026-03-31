# RabbitMQ-Style Connection Management

## Overview

Lite-job now uses **centralized connection management** similar to RabbitMQ, eliminating retry storms and providing clean logs.

## Before vs After

### ❌ Before (Per-Operation Retry)
```
10 Workers → Each calls dequeue()
    ↓
Each → retry_async() independently
    ↓
Redis dies → 10 concurrent retry loops
    ↓
Logs:
  ⚠️ Redis operation failed (attempt 1/20) × 10 workers
  ⚠️ Redis operation failed (attempt 2/20) × 10 workers
  ⚠️ Redis operation failed (attempt 3/20) × 10 workers
  ... (200+ log messages!)
```

### ✅ After (Connection Supervisor)
```
Connection Supervisor (background task)
    ↓
Maintains connection state
    ↓
Redis dies → 1 retry loop (logged once)
    ↓
Workers wait for "ready" signal
    ↓
Logs:
  ⚠️ Redis disconnected - starting reconnection (1 log)
  ⚠️ Reconnect attempt 1/20 failed (1 log)
  ⚠️ Reconnect attempt 2/20 failed (1 log)
  ✅ Redis reconnected successfully (1 log)
  ... (5 log messages total!)
```

## Architecture

```
┌─────────────────────────────────────────────┐
│  ConnectionSupervisor (per pool)            │
│  ┌────────────────────────────────────────┐ │
│  │ State: Connected/Connecting/Disconnected │
│  │ Background supervision loop             │
│  │ Single retry logic                      │
│  └────────────────────────────────────────┘ │
│                    ↕                         │
│         wait_ready() / get_connection()      │
└─────────────────────────────────────────────┘
                          ↕
              ┌─────────────────┐
              │   RedisPool      │
              │   (wraps         │
              │    supervisor)   │
              └─────────────────┘
                    ↕
         ┌──────────────────────┐
         │ SubscriberRegistry    │
         │                      │
         │ ┌────┐ ┌────┐ ┌────┐│
         │ │W1  │ │W2  │ │W3  ││
         │ └────┘ └────┘ └────┘│
         └──────────────────────┘
```

## Features

1. **Single Retry Point**: Only supervisor retries connection
2. **Clean Logs**: 1 set of retry messages instead of N×workers
3. **No Thundering Herd**: Workers wait, don't compete
4. **Connection State Tracking**: Connected/Connecting/Disconnected
5. **Configurable Timeouts**: 30s connection timeout, 20s response timeout
6. **Persistent Retry**: Continues retrying with exponential backoff (1s → 60s max)
7. **Automatic Recovery**: Reconnects and notifies all workers
8. **Smart Backoff**: After 3 consecutive failures, applies exponential backoff

## Usage

```rust
let registry = SubscriberRegistry::new()
    .register("orders", handle_orders)
        .with_pool_size(20)
        .with_concurrency(5);

registry.run().await?;
```

**Behind the scenes:**
1. Creates `ConnectionSupervisor` per queue
2. Supervisor starts background task
3. Workers call `supervisor.wait_ready()` before operations
4. Single retry loop maintains connection
5. Clean logs!

## Testing

Run: `cargo run --example queue_consumer`

**Stop Redis:**
```
⚠️ Redis disconnected - starting reconnection  (1 log, not 10!)
⚠️ Multiple connection failures (4), backing off for 1s
⚠️ Reconnection attempt 5 failed: Connection refused
⚠️ Multiple connection failures (8), backing off for 2s
⚠️ Reconnection attempt 9 failed: Connection refused
✅ Reconnection successful on attempt 9
✅ Redis connected - notifying workers
```

**Start Redis:**
```
✅ Redis reconnected successfully
📝 Worker #0-0 continues processing
📝 Worker #0-1 continues processing
...
```

## Configuration

**Production-Tested Environments:**

✅ **Self-Hosted Redis** (local & Docker)
- Redis 7.x on Linux/macOS/Windows
- Docker containers
- Kubernetes deployments

✅ **Upstash** (Redis Cloud)
- Serverless Redis with automatic connection pooling
- Global edge replication
- Recommended for serverless applications

✅ **Aiven Valkey** (Redis-compatible)
- Valkey 7.x (Aiven's Redis-compatible service)
- Managed cloud service with high availability
- TLS connections required

**Default timeouts (optimized for all providers):**
- **Connection Timeout**: 30 seconds (time to establish connection)
- **Response Timeout**: 20 seconds (time for Redis operations)
- **Retry Attempts**: 20 attempts
- **Initial Backoff**: 1 second
- **Max Backoff**: 60 seconds (exponential)
- **Health Check Interval**: 5 seconds

**Customize timeouts:**
```rust
// Self-hosted Redis (faster, lower latency)
RedisConfig::new("redis://127.0.0.1:6379")
    .with_connection_timeout(5)   // Faster for local
    .with_response_timeout(3);    // Faster operations

// Upstash (cloud, slightly higher latency)
RedisConfig::new("rediss://default:PASS@your-redis.upstash.io:6379")
    .with_connection_timeout(30)  // Default, works well
    .with_response_timeout(20);

// Aiven Valkey (cloud, may need higher timeout for cold starts)
RedisConfig::new("rediss://user:pass@aiven-valkey.aivencloud.com:6379")
    .with_connection_timeout(30)  // Default, optimized for cloud
    .with_response_timeout(20);
```

## Benefits

| Metric | Before | After |
|--------|--------|-------|
| Retry logs when Redis down (10 workers) | 200+ | 5-10 |
| Concurrent retry attempts | 10 | 1 |
| Connection management | Distributed | Centralized |
| Log clarity | Spam | Clean |
| RabbitMQ-style | ❌ | ✅ |
| Connection timeout | None | 30s (configurable) |
| Response timeout | None | 20s (configurable) |
| Persistent retry | Give up after 20 | Infinite with backoff |
| Exponential backoff | ❌ | ✅ (1s → 60s) |
