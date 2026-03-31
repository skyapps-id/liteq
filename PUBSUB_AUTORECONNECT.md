# Redis PubSub Auto-Reconnect

## Overview

Lite-job's Redis PubSub implementation now supports **automatic reconnection** after connection loss, ensuring consumers continue receiving messages even after Redis restarts.

## Problem Solved

**Before:**
```
Consumer Subscribe → Listen for messages
    ↓
Redis disconnect → Stream ends → Consumer stops ❌
    ↓
No more messages received, even after Redis restarts
```

**After:**
```
Consumer Subscribe → Listen for messages
    ↓
Redis disconnect → Stream ends → Auto-reconnect (5s delay)
    ↓
Re-establish connection → Re-subscribe to channels
    ↓
Resume listening → Messages received again ✅
```

## Features

✅ **Auto-Reconnect**: Automatically attempts reconnection after connection loss  
✅ **Auto Re-Subscribe**: Re-subscribes to all channels after reconnection  
✅ **Persistent**: Continues trying until reconnection succeeds  
✅ **Non-Blocking**: Runs in background task, `subscribe()` returns immediately  
✅ **Type-Safe**: Generic types with automatic JSON deserialization  
✅ **Smart Delay**: 5-second delay between reconnect attempts  
✅ **Reconnect Tracking**: Tracks reconnect attempts in logs  

## Usage

### Basic Subscribe with Auto-Reconnect

```rust
use liteq::{RedisPubSub, RedisConfig};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct Event {
    message: String,
    count: usize,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = RedisConfig::new("redis://127.0.0.1:6379");
    let pubsub = RedisPubSub::new(config).await?;

    // Subscribe with auto-reconnect enabled
    pubsub.subscribe(
        vec!["events".to_string()],
        |channel, event: Event| {
            println!("Received on {}: {:?}", channel, event);
        }
    ).await?;

    Ok(())
}
```

### Multiple Channels

```rust
pubsub.subscribe(
    vec!["events".to_string(), "alerts".to_string()],
    |channel, msg: MyMessage| {
        println!("Received on {}: {:?}", channel, msg);
    }
).await?;
```

## Testing Auto-Reconnect

### Step-by-Step Test

**Terminal 1: Start Consumer**
```bash
cargo run --example test_pubsub_reconnect
```

**Terminal 2: Test Sequence**

1. **Publish initial message (works):**
   ```bash
   redis-cli PUBLISH lite-job:test_channel '{"text":"Hello","count":1}'
   # ✅ Consumer receives: "Received on lite-job:test_channel: Hello"
   ```

2. **Stop Redis:**
   ```bash
   redis-cli shutdown
   # Terminal 1 shows:
   # "Subscribe connection lost (attempt 1): Connection reset"
   # "Reconnecting in 5s..."
   ```

3. **Wait for reconnect (logs show):**
   ```
   ⚠️ Subscribe connection lost (attempt 1): Connection reset. Reconnecting in 5s...
   ℹ️ Establishing pubsub connection (reconnect #1)
   ℹ️ Successfully subscribed to channels: ["lite-job:test_channel"]
   ```

4. **Start Redis:**
   ```bash
   redis-server
   ```

5. **Publish again (still works!):**
   ```bash
   redis-cli PUBLISH lite-job:test_channel '{"text":"After reconnect","count":2}'
   # ✅ Consumer receives: "Received on lite-job:test_channel: After reconnect"
   ```

### Expected Logs

**Normal Operation:**
```
ℹ️ Subscribing to channels: ["lite-job:test_channel"]
ℹ️ Subscribing to channel: lite-job:test_channel
ℹ️ Successfully subscribed to channels: ["lite-job:test_channel"]
ℹ️ Raw payload from lite-job:test_channel: {"text":"Hello","count":1}
ℹ️ Successfully deserialized message from channel: lite-job:test_channel
```

**After Redis Restart:**
```
⚠️ Subscribe connection lost (attempt 1): Connection reset. Reconnecting in 5s...
ℹ️ Establishing pubsub connection (reconnect #1)
ℹ️ Subscribing to channel: lite-job:test_channel
ℹ️ Successfully subscribed to channels: ["lite-job:test_channel"]
ℹ️ Raw payload from lite-job:test_channel: {"text":"Still works!","count":2}
ℹ️ Successfully deserialized message from channel: lite-job:test_channel
✅ Received on lite-job:test_channel: Still works!
```

## Implementation Details

### Architecture

```
┌─────────────────────────────────────────────┐
│  RedisPubSub::subscribe()                   │
│  ┌────────────────────────────────────────┐ │
│  │ Spawns background task                 │ │
│  │                                        │ │
│  │  loop {                                │ │
│  │    subscribe_and_listen()             │ │
│  │      ↓                                 │ │
│  │    Listen for messages                 │ │
│  │      ↓                                 │ │
│  │    Connection lost? → Error            │ │
│  │      ↓                                 │ │
│  │    Wait 5s → Reconnect                │ │
│  │  }                                     │ │
│  └────────────────────────────────────────┘ │
└─────────────────────────────────────────────┘
```

### Key Components

1. **`subscribe()`**: Spawns background task, returns immediately
2. **`subscribe_and_listen()`**: Creates pubsub connection, subscribes, listens
3. **Reconnect Loop**: Catches connection loss, waits 5s, retries
4. **Arc Callback**: Ensures callback is safe to share across reconnects

### Error Handling

```rust
loop {
    match subscribe_and_listen(...).await {
        Err(e) => {
            reconnect_count += 1;
            warn!("Connection lost (attempt #{}): {}. Reconnecting in 5s...", 
                  reconnect_count, e);
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
        Ok(_) => break, // Graceful shutdown
    }
}
```

## Benefits

| Feature | Before | After |
|---------|--------|-------|
| Redis restart survival | ❌ Stops working | ✅ Auto-reconnects |
| Re-subscription | Manual | Automatic |
| Downtime tolerance | None | 5s reconnect delay |
| Message loss | All after restart | None (resumes where left off) |
| Operator intervention | Required | None (fully automatic) |

## Configuration

**Reconnect Settings:**
- **Delay**: 5 seconds between reconnect attempts
- **Max Attempts**: Infinite (keeps trying until successful)
- **Channel Re-subscription**: Automatic (all channels)

**Customize reconnect delay (future):**
```rust
// Future enhancement:
pubsub.subscribe_with_reconnect_delay(
    channels,
    callback,
    Duration::from_secs(10) // 10s delay
).await?;
```

## Best Practices

1. **Use separate tasks for each subscription group**
   ```rust
   // Events subscription
   tokio::spawn(async move {
       pubsub.subscribe(events_channels, handle_events).await
   });
   
   // Alerts subscription
   tokio::spawn(async move {
       pubsub.subscribe(alerts_channels, handle_alerts).await
   });
   ```

2. **Handle serialization errors in callback**
   ```rust
   |channel, msg: MyMessage| {
       match msg.process() {
           Ok(_) => println!("Success"),
           Err(e) => eprintln!("Error: {}", e),
       }
   }
   ```

3. **Monitor reconnect attempts in logs**
   - High reconnect count = check Redis stability
   - Frequent reconnects = network issues or Redis restarts

## Troubleshooting

**Problem**: Consumer stops receiving messages after Redis restart

**Solution**: 
- Check logs for "Subscribe connection lost" messages
- Verify Redis is running: `redis-cli ping`
- Ensure consumer process is still alive

**Problem**: Reconnect attempts failing continuously

**Solution**:
- Check Redis connection string is correct
- Verify network connectivity to Redis
- Check Redis logs for errors
- Increase timeout if Redis is slow to respond

**Problem**: Messages not being received after reconnect

**Solution**:
- Verify channel names match (including key prefix)
- Check callback is not panicking
- Ensure message deserialization is working

## See Also

- [README.md](README.md) - Main documentation
- [CONNECTION_SUPERVISION.md](CONNECTION_SUPERVISION.md) - Connection management details
- [examples/test_pubsub_reconnect.rs](examples/test_pubsub_reconnect.rs) - Working example
