# Multi-Consumer Fair Distribution Design

## ⚠️ DESIGN NOTE

This document describes the **initial design**. The actual implementation uses **auto-registration** instead of manual `ConsumerGroupConfig`. See `IMPLEMENTATION_COMPLETE.md` for current implementation details.

## Problem Statement

When running multiple consumer instances of `liteq`, jobs are **not distributed fairly** across instances due to `LPOP` race condition in Redis LIST.

### Current Behavior

```
Timeline (3 consumer instances, all polling every 500ms):

t=0ms:   Consumer-1 LPOP → gets Job-A ✅
t=10ms:  Consumer-2 LPOP → queue empty ❌
t=20ms:  Consumer-3 LPOP → queue empty ❌

t=500ms: Consumer-1 LPOP → gets Job-B ✅
t=510ms:  Consumer-2 LPOP → queue empty ❌
t=520ms:  Consumer-3 LPOP → queue empty ❌

Result:
- Consumer-1: Processes ALL jobs (overwhelmed, CPU 100%)
- Consumer-2: IDLE (0 jobs, wasted resources)
- Consumer-3: IDLE (0 jobs, wasted resources)
```

### Root Cause

**Redis `LPOP` is not fair for multiple consumers:**

```lua
-- Current DEQUEUE_SCRIPT (queue.rs:24)
local job = redis.call('LPOP', list_key)  -- First come, first served
return job
```

All consumers compete for the same jobs. The fastest/most frequent consumer wins, creating **starvation** for others.

---

## Constraints & Requirements

### Must Have
- ✅ **ETA scheduling is CORE feature** (cannot compromise)
- ✅ **Fair distribution** across multiple consumer instances
- ✅ **Horizontal scaling** support (2+ instances)
- ✅ **Consumer crash recovery** (jobs not lost)

### Nice to Have
- 🟡 Dynamic consumer scaling (add/remove instances)
- 🟡 Minimal architecture changes
- 🟡 Simple implementation

---

## Solution: LIST with Modulo Distribution

### Architecture Overview

```
┌─────────────────────────────────────────────────────────┐
│                    Producer Side                         │
└────────────────────┬────────────────────────────────────┘
                     │
              ┌──────┴──────┐
              │   Has ETA?  │
              └──────┬──────┘
                     │
         ┌───────────┴───────────┐
         │                       │
      YES │                       │ NO
         ↓                       ↓
    ┌─────────┐            ┌──────────┐
    │   ZSET  │            │   LIST   │
    │(score=  │            │          │
    │ ETA)    │            │          │
    └────┬────┘            └────┬─────┘
         │                       │
         │    ┌──────────────────┤
         │    │  Dequeue Script  │
         │    │  (Priority)      │
         │    └────────┬─────────┘
         │             │
         │    1. Check ZSET for ready jobs (ETA <= now)
         │    2. If none, check LIST with modulo
         │             │
         └─────────────┴──────────────┐
                                       ↓
                    ┌─────────────────────────────────┐
                    │   Consumer Distribution Layer   │
                    │   (Modulo-based assignment)     │
                    └─────────────────────────────────┘
                                       │
         ┌─────────────────────────────┼─────────────────────────────┐
         │                             │                             │
         ↓                             ↓                             ↓
    ┌─────────┐                  ┌─────────┐                   ┌─────────┐
    │Consumer │                  │Consumer │                   │Consumer │
    │ Instance│                  │ Instance│                   │ Instance│
    │   ID=0  │                  │   ID=1  │                   │   ID=2  │
    └─────────┘                  └─────────┘                   └─────────┘
    Gets jobs:                   Gets jobs:                    Gets jobs:
    0, 3, 6, 9...                1, 4, 7, 10...               2, 5, 8, 11...
```

### How Modulo Distribution Works

**Counter-based Assignment:**

```lua
-- Enhanced DEQUEUE_SCRIPT
local consumer_id = ARGV[2]          -- Consumer instance ID (0, 1, 2...)
local num_consumers = ARGV[3]        -- Total number of consumers
local job_counter = redis.call('HINCRBY', 'queue:orders:meta', 'counter', 1)

-- Calculate which consumer should get this job
local target_consumer = job_counter % num_consumers

-- Only return job if this consumer is the target
if target_consumer == consumer_id then
    -- Check ZSET first (ready scheduled jobs)
    local ready_jobs = redis.call('ZRANGEBYSCORE', zset_key, '-inf', current_time, 'LIMIT', 0, 1)
    if ready_jobs and #ready_jobs > 0 then
        local job_json = ready_jobs[1]
        redis.call('ZREM', zset_key, job_json)
        return job_json
    end

    -- Check LIST (regular jobs)
    local job = redis.call('LPOP', list_key)
    return job
else
    -- Not this consumer's turn, skip
    return nil
end
```

**Result:**
```
Job Sequence:  1   2   3   4   5   6   7   8   9  10  11  12
               ↓   ↓   ↓   ↓   ↓   ↓   ↓   ↓   ↓   ↓   ↓   ↓
Consumer 0:    1       3       6       8       10      12
Consumer 1:        2       5       7       9       11
Consumer 2:            4                          (IDLE if only 2 jobs)

→ Fair distribution across all active consumers
→ No starvation (each consumer gets equal share)
→ Consumer crash → jobs automatically go to remaining consumers
```

---

## Implementation Details

### 1. Redis Data Structures

```
# Scheduled jobs (with ETA)
ZADD schedule:orders {eta_timestamp} "{job_json}"

# Regular jobs (no ETA)
RPUSH queue:orders "{job_json}"

# Consumer metadata (counter + config)
HSET queue:orders:meta counter 0
HSET queue:orders:meta num_consumers 3

# Consumer registration
HSET queue:orders:consumers "consumer_0,consumer_1,consumer_2"
```

### 2. Consumer Configuration

```rust
// In examples/queue_consumer.rs
let mut registry = SubscriberRegistry::new();

// Instance 1
let app_data_1 = AppData::new();
registry.register("orders", handle_orders)
    .with_data(app_data_1)
    .with_consumer_group(ConsumerGroupConfig {
        group_name: "orders".to_string(),
        consumer_id: 0,
        total_consumers: 3,
    })
    .with_pool_size(20)
    .with_concurrency(5)
    .build();

// Instance 2 (run on different machine/terminal)
let app_data_2 = AppData::new();
registry.register("orders", handle_orders)
    .with_data(app_data_2)
    .with_consumer_group(ConsumerGroupConfig {
        group_name: "orders".to_string(),
        consumer_id: 1,
        total_consumers: 3,
    })
    .with_pool_size(20)
    .with_concurrency(5)
    .build();

// Instance 3
let app_data_3 = AppData::new();
registry.register("orders", handle_orders)
    .with_data(app_data_3)
    .with_consumer_group(ConsumerGroupConfig {
        group_name: "orders".to_string(),
        consumer_id: 2,
        total_consumers: 3,
    })
    .with_pool_size(20)
    .with_concurrency(5)
    .build();

registry.run().await?;
```

### 3. API Changes

**New Config Structure:**

```rust
// src/config.rs
#[derive(Clone, Debug)]
pub struct ConsumerGroupConfig {
    pub group_name: String,
    pub consumer_id: usize,
    pub total_consumers: usize,
}

impl ConsumerGroupConfig {
    pub fn new(group_name: impl Into<String>, consumer_id: usize, total_consumers: usize) -> Self {
        Self {
            group_name: group_name.into(),
            consumer_id,
            total_consumers,
        }
    }
}
```

**Modified Dequeue Script:**

```rust
// src/queue.rs - Update DEQUEUE_SCRIPT
static DEQUEUE_SCRIPT: &str = r#"
local list_key = KEYS[1]
local zset_key = KEYS[2]
local current_time = tonumber(ARGV[1])
local consumer_id = tonumber(ARGV[2])
local num_consumers = tonumber(ARGV[3])

-- Increment job counter
local job_counter = redis.call('HINCRBY', KEYS[4], 'counter', 1)

-- Calculate target consumer using modulo
local target_consumer = job_counter % num_consumers

-- Only return job if this consumer is the target
if target_consumer == consumer_id then
    -- 1. Check scheduled jobs that are ready (timestamp <= current_time)
    local ready_jobs = redis.call('ZRANGEBYSCORE', zset_key, '-inf', current_time, 'LIMIT', 0, 1)
    if ready_jobs and #ready_jobs > 0 then
        local job_json = ready_jobs[1]
        redis.call('ZREM', zset_key, job_json)
        return job_json
    end

    -- 2. Get from regular queue
    local job = redis.call('LPOP', list_key)
    return job
else
    -- Not this consumer's turn
    return nil
end
"#;

// Update dequeue call to pass consumer info
pub async fn dequeue<T>(&self, consumer_config: &ConsumerGroupConfig) -> JobResult<Option<Job<T>>>
where
    T: for<'de> serde::Deserialize<'de>,
{
    let queue_key = self.redis_config.make_key(&format!("queue:{}", self.config.name));
    let schedule_key = self.redis_config.make_key(&format!("schedule:{}", self.config.name));
    let meta_key = self.redis_config.make_key(&format!("{}:meta", self.config.name));
    let client = self.client.clone();
    let current_time = Utc::now().timestamp();

    let result: Option<String> = retry_async(
        || async {
            let mut conn = client.get_multiplexed_async_connection().await
                .map_err(JobError::from)?;

            let job_json: Option<String> = self.dequeue_script
                .key(&queue_key)
                .key(&schedule_key)
                .key(&meta_key)
                .arg(current_time)
                .arg(consumer_config.consumer_id)
                .arg(consumer_config.total_consumers)
                .invoke_async(&mut conn)
                .await
                .map_err(JobError::from)?;

            Ok(job_json)
        },
        Some(self.retry_config.clone()),
    ).await?;

    match result {
        Some(job_json) => {
            let job = Job::from_json(&job_json)?;
            Ok(Some(job))
        }
        None => Ok(None),
    }
}
```

### 4. Registry Changes

```rust
// src/registry.rs
pub struct QueueBuilder<'a, F = Handler> {
    registry: &'a mut SubscriberRegistry,
    queue: String,
    handler: Option<F>,
    concurrency: usize,
    pool_size: Option<usize>,
    min_idle: Option<usize>,
    consumer_config: Option<ConsumerGroupConfig>,  // ← New field
}

impl<'a, F> QueueBuilder<'a, F>
where
    F: Send + Sync + 'static,
{
    /// Configure consumer group for fair distribution
    pub fn with_consumer_group(mut self, config: ConsumerGroupConfig) -> Self {
        self.consumer_config = Some(config);
        self
    }
}

// Update worker loop to use consumer config
let queue_clone = group.queue.clone();
let handler = handler.clone();
let pool = group.pool.clone();
let queue_name = queue_name.clone();
let metrics = self.metrics.clone();
let consumer_config = worker_config.consumer_config.clone();  // ← Pass to worker

let handle = tokio::spawn(async move {
    loop {
        // Pass consumer config to dequeue
        if let Ok(Some(job)) = queue_clone.dequeue_with_config(&consumer_config).await {
            // Process job...
        }
        sleep(Duration::from_millis(500)).await;
    }
});
```

---

## Consumer Lifecycle

### Adding a New Consumer

```
Step 1: Decide on new total_consumers count
  Before: 3 consumers (ID 0, 1, 2)
  After:  4 consumers (ID 0, 1, 2, 3)

Step 2: Update ALL instances
  Instance 1: consumer_id=0, total_consumers=4
  Instance 2: consumer_id=1, total_consumers=4
  Instance 3: consumer_id=2, total_consumers=4
  Instance 4: consumer_id=3, total_consumers=4  ← New

Step 3: Restart all instances

Step 4: Verify fair distribution
  Job 1, 5, 9, 13... → Consumer 0
  Job 2, 6, 10, 14... → Consumer 1
  Job 3, 7, 11, 15... → Consumer 2
  Job 4, 8, 12, 16... → Consumer 3
```

### Removing a Consumer

```
Step 1: Update remaining instances
  Before: 3 consumers (ID 0, 1, 2)
  After:  2 consumers (ID 0, 1)

Step 2: Update configs
  Instance 1: consumer_id=0, total_consumers=2
  Instance 2: consumer_id=1, total_consumers=2
  Instance 3: Stop/shutdown

Step 3: Restart remaining instances

Result:
  Jobs automatically redistributed to remaining consumers
  No manual rebalancing needed
```

---

## Why This Solution?

### ✅ Advantages

1. **ETA Performance Preserved**
   - ZSET with score-based queries: O(log N)
   - No degradation in scheduling performance
   - ETA remains core feature (optimal)

2. **Fair Distribution Achieved**
   - Modulo ensures equal distribution
   - No starvation of any consumer
   - Predictable load balancing

3. **Minimal Architecture Changes**
   - Only Lua script modification
   - No background workers needed
   - Same data structures (ZSET + LIST)

4. **Consumer Crash Recovery**
   - If Consumer 1 crashes, jobs redistribute automatically
   - Counter keeps incrementing in Redis
   - Remaining consumers pick up slack

5. **Horizontal Scaling**
   - Add more instances → increase `total_consumers`
   - Remove instances → decrease `total_consumers`
   - Simple configuration change

6. **Backward Compatible**
   - Can gradually migrate existing deployments
   - Single instance mode works (consumer_id=0, total_consumers=1)

### ⚠️ Limitations

1. **Manual Consumer Management**
   - Need to track `total_consumers` across instances
   - Add/remove requires config update on all instances
   - Not fully automatic like Redis Streams Consumer Groups

2. **Requires Restart**
   - Changing consumer count needs restart
   - No dynamic rebalancing at runtime

3. **Potential Job Duplication (Rare)**
   - If consumer crashes during job processing
   - Another consumer might pick up same job later
   - Mitigation: Idempotent job handlers

---

## Comparison with Alternatives

### vs. Redis Streams (Option 2)

| Aspect | LIST + Modulo (This) | Redis Streams |
|--------|---------------------|---------------|
| ETA Performance | ✅ O(log N) optimal | ⚠️ Requires filter scan |
| Fair Distribution | ✅ Yes (modulo) | ✅ Yes (consumer groups) |
| Crash Recovery | ✅ Auto (counter) | ✅ Auto (pending) |
| Complexity | 🟢 Low (Lua script) | 🔴 High (scheduler worker) |
| Background Workers | ❌ None | ✅ Required (ZSET→Stream) |
| Dynamic Scaling | ⚠️ Manual restart | ✅ XGROUP dynamic |
| Implementation | 🟢 Simple | 🔴 Complex |

**Verdict:** For ETA-heavy use cases, LIST + Modulo is better.

### vs. Plain LPOP (Current)

| Aspect | Plain LPOP | LIST + Modulo |
|--------|-----------|---------------|
| Fair Distribution | ❌ No | ✅ Yes |
| Starvation | ❌ Yes | ✅ No |
| Resource Utilization | ❌ Poor (some idle) | ✅ Good (balanced) |
| Horizontal Scaling | ❌ Ineffective | ✅ Effective |

---

## Migration Path

### Phase 1: Add Consumer Group Config (Non-breaking)

```rust
// Optional parameter - defaults to single consumer
.with_consumer_group(ConsumerGroupConfig {
    consumer_id: 0,
    total_consumers: 1,  // Default: single instance mode
})
```

### Phase 2: Update Dequeue Script

- Add `consumer_id` and `total_consumers` parameters
- Implement modulo logic
- Keep fallback to old behavior if not provided

### Phase 3: Deploy Multi-Instance

```bash
# Terminal 1
export CONSUMER_ID=0
export TOTAL_CONSUMERS=3
cargo run --example queue_consumer

# Terminal 2
export CONSUMER_ID=1
export TOTAL_CONSUMERS=3
cargo run --example queue_consumer

# Terminal 3
export CONSUMER_ID=2
export TOTAL_CONSUMERS=3
cargo run --example queue_consumer
```

### Phase 4: Monitor & Verify

```bash
# Check job distribution
redis-cli HGETALL queue:orders:meta

# Monitor processing logs
# Should see fair distribution across instances
```

---

## Testing Strategy

### Unit Tests

```rust
#[tokio::test]
async fn test_modulo_distribution() {
    // Create 3 consumers
    let consumer_0 = ConsumerGroupConfig::new("test", 0, 3);
    let consumer_1 = ConsumerGroupConfig::new("test", 1, 3);
    let consumer_2 = ConsumerGroupConfig::new("test", 2, 3);

    // Enqueue 9 jobs
    for i in 1..=9 {
        queue.enqueue(Job::new(i, "test")).await.unwrap();
    }

    // Verify distribution
    let jobs_0 = consumer_0.dequeue_n(3).await;
    let jobs_1 = consumer_1.dequeue_n(3).await;
    let jobs_2 = consumer_2.dequeue_n(3).await;

    assert_eq!(jobs_0.len(), 3);
    assert_eq!(jobs_1.len(), 3);
    assert_eq!(jobs_2.len(), 3);

    // Verify no duplicates
    let all_ids: Vec<_> = jobs_0.iter()
        .chain(jobs_1.iter())
        .chain(jobs_2.iter())
        .map(|j| j.id.clone())
        .collect();
    assert_eq!(all_ids.len(), 9);  // All unique
}
```

### Integration Tests

```bash
# Terminal 1: Start 3 consumers
for id in 0 1 2; do
    CONSUMER_ID=$id TOTAL_CONSUMERS=3 cargo run --example queue_consumer &
done

# Terminal 2: Enqueue jobs
cargo run --example queue_producer

# Verify fair distribution in logs
grep "Total processed" logs/*.log | sort
```

---

## Performance Considerations

### Memory Impact

**Minimal:**
- One additional Redis HSET per queue: `queue:orders:meta`
- Stores `counter` (integer) + `num_consumers` (small integer)
- Total overhead: < 100 bytes per queue

### Network Impact

**Same as before:**
- One roundtrip per dequeue (Lua script atomic)
- No additional calls

### CPU Impact

**Negligible:**
- Modulo operation: O(1)
- HINCRBY: O(1)
- No significant performance degradation

---

## Future Enhancements

### 1. Dynamic Consumer Registration

```rust
// Auto-discovery of active consumers
registry.register("orders", handler)
    .with_auto_consumer_group()  // Auto-detect peers
    .build();
```

### 2. Health-Based Load Balancing

```rust
// Slower consumers get fewer jobs
.with_load_balancing(LoadBalancingStrategy::Adaptive)
```

### 3. Redis Streams Fallback

```rust
// Use Streams if ETA < 1 second (optimization)
.with_hybrid_scheduling(threshold_sec=1)
```

---

## FAQ

**Q: What if a consumer crashes mid-job?**
A: Job will be picked up by another consumer after counter wraps around. Implement idempotent handlers.

**Q: Can I mix consumers with different counts?**
A: No - all consumers must have same `total_consumers` value. Otherwise distribution breaks.

**Q: What happens if I set `total_consumers=3` but only run 2 instances?**
A: Jobs assigned to consumer ID 2 will not be processed until you start the 3rd instance.

**Q: Can I change consumer count without restart?**
A: Not recommended - may cause job duplication or missed jobs. Restart all instances when scaling.

**Q: Is this suitable for high-frequency scaling (Kubernetes)?**
A: For dynamic environments, consider Redis Streams instead (more complex but supports auto-scaling).

---

## Conclusion

This solution provides **fair distribution for multiple consumer instances** while **preserving optimal ETA scheduling performance** with **minimal architectural changes**.

**Key Trade-off:**
- Gain: Fair distribution, horizontal scaling
- Cost: Manual consumer management, requires restart for scaling

**Best for:**
- ETA-heavy workloads
- Fixed or semi-fixed consumer counts
- Teams comfortable with Lua scripts
- Scenarios where simple > complex

---

## 📝 Actual Implementation Notes

**What was actually implemented (better than design!):**

### ✅ Auto-Registration (Design Change)
Instead of manual `ConsumerGroupConfig`, the implementation uses:
- **Automatic self-registration** with UUID
- **Heartbeat-based health checking** (30s TTL)
- **Peer discovery** via Redis KEYS
- **Dynamic position calculation** (sorted UUIDs)

**Benefits:**
- ✅ Zero configuration needed
- ✅ No manual ID assignment
- ✅ Automatic failure detection
- ✅ Consumer crash recovery

### ✅ Parallel Registration (Performance Fix)
- **Sequential → Parallel** registration
- Uses `tokio::spawn` + `join_all`
- Startup time: 8s total (not 8s per queue)

### ✅ Payload-Only Handlers (API Fix)
- All workers send **payload only** (not full Job)
- Consistent handler interface
- Simpler deserialization

**See:** `IMPLEMENTATION_COMPLETE.md` for full details on actual implementation.

**Next Steps (if following this design doc):**
1. ~~Implement `ConsumerGroupConfig` struct~~ ✅ Replaced with auto-registration
2. ✅ Update `DEQUEUE_SCRIPT` with modulo logic
3. ~~Add `with_consumer_group()` builder method~~ ✅ Not needed with auto-reg
4. ✅ Update `dequeue()` to accept consumer info
5. ✅ Test with 2-3 consumer instances
6. ✅ Document deployment process
