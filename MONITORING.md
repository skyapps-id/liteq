# Monitoring & Observability Guide

## Overview

liteq provides comprehensive monitoring and observability features for production deployments, including:

- **Real-time job counts** - Track scheduled and regular jobs
- **Health checks** - Monitor pool status, error rates, and performance
- **Batch processing** - High-throughput dequeue for large workloads
- **Queue statistics** - Detailed metrics for capacity planning

---

## Table of Contents

- [Monitoring Job Counts](#monitoring-job-counts)
- [Health Checks](#health-checks)
- [Batch Dequeue](#batch-dequeue)
- [Production Examples](#production-examples)

---

## Monitoring Job Counts

### get_job_counts()

Track the number of pending jobs in real-time:

```rust
use liteq::{JobQueue, QueueConfig, RedisConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let queue = JobQueue::new(
        QueueConfig::new("my_queue"),
        RedisConfig::new("redis://127.0.0.1:6379")
    ).await?;

    // Get job counts
    let (regular_count, scheduled_count) = queue.get_job_counts().await?;

    println!("Regular jobs: {}", regular_count);
    println!("Scheduled jobs: {}", scheduled_count);
    println!("Total pending: {}", regular_count + scheduled_count);

    Ok(())
}
```

**Returns:**
- `regular_count` - Number of jobs in LIST (immediate processing)
- `scheduled_count` - Number of jobs in ZSET (delayed processing)

### get_queue_stats()

Get detailed queue statistics:

```rust
let stats = queue.get_queue_stats().await?;

println!("Queue: {}", stats.queue_name);
println!("Regular jobs: {}", stats.regular_jobs);
println!("Scheduled jobs: {}", stats.scheduled_jobs);
println!("Total pending: {}", stats.total_pending);
```

**Use Cases:**
- Dashboard metrics
- Alert on backlog threshold
- Auto-scaling decisions
- Capacity planning

---

## Health Checks

### Basic Health Check

```rust
use liteq::SubscriberRegistry;

let registry = SubscriberRegistry::new();

// Register queues...
registry.register("my_queue", handler).build();

// Check health of specific queue
let health = registry.health_check("my_queue").await;

println!("Status: {:?}", health.status);
println!("Error rate: {:.2}%", health.error_rate * 100.0);
println!("Scheduled jobs: {}", health.scheduled_jobs_count);
println!("Regular jobs: {}", health.regular_jobs_count);
println!("Total pending: {}", health.total_pending_jobs);
```

### Health Status

The health check returns one of three statuses:

| Status | Condition |
|--------|-----------|
| **Healthy** | Error rate < 10%, available connections ≥ 2, backlog < 10K |
| **Degraded** | Error rate 10-50%, OR low connections, OR large backlog |
| **Unhealthy** | Error rate > 50%, OR no available connections |

### Pool Status

Monitor connection pool health:

```rust
let health = registry.health_check("my_queue").await;

let pool = &health.pool_status;
println!("Max size: {}", pool.max_size);
println!("Current size: {}", pool.size);
println!("Available: {}", pool.available);
println!("Active: {}", pool.active);
println!("Waiting: {}", pool.waiting);
println!("Scheduled jobs: {}", pool.scheduled_jobs);
println!("Regular jobs: {}", pool.regular_jobs);
```

### Performance Metrics

Track operation performance:

```rust
let metrics = &health.metrics;

println!("Total operations: {}", metrics.total_operations);
println!("Successful: {}", metrics.successful_operations);
println!("Failed: {}", metrics.failed_operations);

if let Some(avg_latency) = metrics.average_latency() {
    println!("Avg latency: {:.2}ms", avg_latency);
}

println!("Last operation: {:?}", metrics.last_operation_time);
```

---

## Batch Dequeue

### High-Throughput Processing

For scenarios requiring high throughput, use batch dequeue:

```rust
use liteq::{Job, JobQueue, QueueConfig, RedisConfig};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let queue = JobQueue::new(
        QueueConfig::new("high_throughput_queue"),
        RedisConfig::new("redis://127.0.0.1:6379")
    ).await?;

    // Dequeue up to 100 jobs at once
    let batch = queue.dequeue_batch::<MyType>(100).await?;

    println!("Dequeued {} jobs", batch.len());

    for job in batch {
        // Process job...
        process_job(job)?;
    }

    Ok(())
}
```

**Benefits:**
- Reduced network roundtrips (up to 100x improvement)
- Higher throughput for batch processing
- Efficient for CPU-intensive operations

**Batch Priority:**
1. Ready scheduled jobs (ETA ≤ now) - up to batch_size
2. Regular jobs - fills remaining batch capacity

**Use Cases:**
- Batch ETL jobs
- Bulk email sending
- Mass data processing
- Report generation

---

## Production Examples

### 1. Monitoring Dashboard

```rust
use std::time::{Duration, interval};
use tokio::time::sleep;

async fn monitoring_dashboard(queue: &JobQueue) -> JobResult<()> {
    let mut interval = interval(Duration::from_secs(5));

    loop {
        interval.tick().await;

        let (regular, scheduled) = queue.get_job_counts().await?;
        let stats = queue.get_queue_stats().await?;

        // Send to monitoring system (Prometheus, Datadog, etc.)
        metrics::gauge!("queue_regular_jobs", regular as f64, "queue" => stats.queue_name);
        metrics::gauge!("queue_scheduled_jobs", scheduled as f64, "queue" => stats.queue_name);
        metrics::gauge!("queue_total_pending", stats.total_pending as f64, "queue" => stats.queue_name);

        // Alert on threshold
        if stats.total_pending > 10000 {
            alert!("High backlog detected: {} jobs", stats.total_pending);
        }

        // Log for dashboards
        tracing::info!(
            queue = %stats.queue_name,
            regular = regular,
            scheduled = scheduled,
            total = stats.total_pending,
            "Queue metrics"
        );
    }
}
```

### 2. Auto-Scaling Workers

```rust
async fn auto_scale_workers(registry: &SubscriberRegistry, queue_name: &str) -> JobResult<()> {
    let mut interval = interval(Duration::from_secs(30));

    loop {
        interval.tick().await;

        let (regular, scheduled) = registry.get_job_counts(queue_name).await
            .unwrap_or((0, 0));
        let total = regular + scheduled;

        // Scale based on backlog
        if total > 5000 {
            // Scale up: add more workers
            scale_up_workers(queue_name, 5).await?;
        } else if total < 100 {
            // Scale down: remove idle workers
            scale_down_workers(queue_name, 3).await?;
        }
    }
}
```

### 3. Health Check Endpoint

```rust
use actix_web::{web, HttpResponse};
use serde::Serialize;

#[derive(Serialize)]
struct HealthResponse {
    status: String,
    queues: Vec<QueueHealthData>,
}

#[derive(Serialize)]
struct QueueHealthData {
    name: String,
    status: String,
    scheduled_jobs: usize,
    regular_jobs: usize,
    total_pending: usize,
    error_rate: f64,
}

async fn health_endpoint(
    registry: web::Data<SubscriberRegistry>
) -> HttpResponse {
    let health_checks = registry.health_check_all().await;

    let queues: Vec<QueueHealthData> = health_checks
        .into_iter()
        .map(|(name, health)| QueueHealthData {
            name,
            status: format!("{:?}", health.status),
            scheduled_jobs: health.scheduled_jobs_count,
            regular_jobs: health.regular_jobs_count,
            total_pending: health.total_pending_jobs,
            error_rate: health.error_rate,
        })
        .collect();

    let overall_status = if queues.iter().all(|q| q.status == "Healthy") {
        "healthy"
    } else if queues.iter().any(|q| q.status == "Unhealthy") {
        "unhealthy"
    } else {
        "degraded"
    };

    HttpResponse::Ok().json(HealthResponse {
        status: overall_status.to_string(),
        queues,
    })
}
```

### 4. Alerting on Thresholds

```rust
async fn alert_on_backlog(queue: &JobQueue, queue_name: &str) -> JobResult<()> {
    let mut interval = interval(Duration::from_secs(60));

    loop {
        interval.tick().await;

        let stats = queue.get_queue_stats().await?;

        // Critical alert
        if stats.total_pending > 50000 {
            send_alert!(
                severity = "critical",
                queue = queue_name,
                message = format!("Critical backlog: {} jobs", stats.total_pending),
                action = "Scale up workers immediately"
            );
        }
        // Warning alert
        else if stats.total_pending > 10000 {
            send_alert!(
                severity = "warning",
                queue = queue_name,
                message = format!("High backlog: {} jobs", stats.total_pending),
                action = "Consider scaling up workers"
            );
        }
    }
}
```

### 5. Scheduled Job Monitoring

```rust
async fn monitor_scheduled_jobs(queue: &JobQueue) -> JobResult<()> {
    let mut interval = interval(Duration::from_secs(10));

    loop {
        interval.tick().await;

        let (regular, scheduled) = queue.get_job_counts().await?;

        // Track scheduled job trends
        if scheduled > 1000 {
            tracing::warn!(
                scheduled_jobs = scheduled,
                "High number of scheduled jobs pending"
            );

            // Could analyze ETA distribution
            // to predict processing load
        }

        // Calculate scheduled vs regular ratio
        let total = regular + scheduled;
        if total > 0 {
            let scheduled_ratio = scheduled as f64 / total as f64;

            metrics::gauge!("scheduled_job_ratio", scheduled_ratio);

            if scheduled_ratio > 0.8 {
                tracing::info!(
                    "High scheduled job ratio: {:.1}%",
                    scheduled_ratio * 100.0
                );
            }
        }
    }
}
```

---

## Best Practices

### 1. Monitoring Frequency

```rust
// ✅ Good: Moderate frequency (5-10 seconds)
let mut interval = interval(Duration::from_secs(5));

// ❌ Bad: Too frequent (high overhead)
let mut interval = interval(Duration::from_millis(100));

// ❌ Bad: Too infrequent (missed issues)
let mut interval = interval(Duration::from_secs(300));
```

### 2. Alert Thresholds

Set thresholds based on your workload:

```rust
const BACKLOG_WARNING: usize = 10000;    // Adjust based on capacity
const BACKLOG_CRITICAL: usize = 50000;   // Adjust based on SLA
const ERROR_RATE_WARNING: f64 = 0.1;     // 10%
const ERROR_RATE_CRITICAL: f64 = 0.5;    // 50%
```

### 3. Batch Sizing

Choose batch size based on job processing time:

```rust
// Quick jobs (< 100ms): Large batches
let batch = queue.dequeue_batch::<Type>(100).await?;

// Medium jobs (100ms - 1s): Medium batches
let batch = queue.dequeue_batch::<Type>(20).await?;

// Slow jobs (> 1s): Small batches
let batch = queue.dequeue_batch::<Type>(5).await?;
```

### 4. Health Check Integration

```rust
// Kubernetes liveness/readiness probes
async fn liveness() -> &'static str {
    "OK"
}

async fn readiness(registry: web::Data<SubscriberRegistry>) -> HttpResponse {
    let health = registry.health_check_all().await;

    if health.values().all(|h| h.status != HealthStatus::Unhealthy) {
        HttpResponse::Ok().body("Ready")
    } else {
        HttpResponse::ServiceUnavailable().body("Not Ready")
    }
}
```

---

## Summary

| Feature | Method | Use Case |
|---------|--------|----------|
| **Job Counts** | `get_job_counts()` | Real-time monitoring |
| **Queue Stats** | `get_queue_stats()` | Detailed metrics |
| **Health Check** | `health_check()` | System health |
| **Batch Dequeue** | `dequeue_batch()` | High throughput |

These monitoring features enable:
- ✅ Real-time observability
- ✅ Proactive alerting
- ✅ Auto-scaling decisions
- ✅ Performance optimization
- ✅ Capacity planning

For more examples, see `examples/monitoring_demo.rs`.
