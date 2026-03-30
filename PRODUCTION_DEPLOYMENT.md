# Production Deployment Guide

## Overview

liteq has been **production-tested** with multiple Redis providers and is ready for deployment in various environments.

## Tested Platforms

### ✅ Self-Hosted Redis

**Versions Tested:**
- Redis 7.0.x
- Redis 7.2.x
- Docker containers (redis:7-alpine)
- Kubernetes deployments

**Configuration:**
```rust
RedisConfig::new("redis://127.0.0.1:6379")
    .with_connection_timeout(5)   // Lower timeout for local
    .with_response_timeout(3);
```

**Deployment Options:**

1. **Local Development**
   ```bash
   # Using Docker
   docker run -d -p 6379:6379 redis:7-alpine
   
   # Or local Redis server
   redis-server --port 6379
   ```

2. **Docker Compose**
   ```yaml
   services:
     redis:
       image: redis:7-alpine
       ports:
         - "6379:6379"
       command: redis-server --appendonly yes
     
     app:
       build: .
       depends_on:
         - redis
       environment:
         - REDIS_URL=redis://redis:6379
   ```

3. **Kubernetes**
   ```yaml
   apiVersion: v1
   kind: Deployment
   metadata:
     name: redis
   spec:
     template:
       spec:
         containers:
         - name: redis
           image: redis:7-alpine
           ports:
           - containerPort: 6379
   ```

### ✅ Upstash (Redis Cloud)

**Why Upstash:**
- Serverless Redis with automatic scaling
- Global edge replication (low latency worldwide)
- Free tier available for development
- Pay-per-request pricing

**Configuration:**
```rust
RedisConfig::new("rediss://default:YOUR_PASSWORD@your-redis.upstash.io:6379")
    .with_connection_timeout(30)  // Default, works well
    .with_response_timeout(20);
```

**Getting Started:**

1. **Create Upstash Redis Database:**
   - Go to https://upstash.com
   - Create a new Redis database
   - Select region closest to your users
   - Copy the connection URL (REST API or Redis)

2. **Use in Your Application:**
   ```rust
   // From Upstash dashboard → Details → REST API
   let redis_url = "rediss://default:abc123@xyz.upstash.io:6379";
   
   let config = RedisConfig::new(redis_url);
   let queue = JobQueue::new(QueueConfig::new("orders"), config).await?;
   ```

**Best Practices:**
- ✅ Use TLS (`rediss://`) for security
- ✅ Default timeouts (30s/20s) are optimal
- ✅ Enable automatic eviction if needed
- ✅ Monitor with Upstash dashboard

**Features Tested:**
- ✅ All queue operations (enqueue, dequeue)
- ✅ ETA scheduling with ZSET
- ✅ Multi-consumer fair distribution
- ✅ Auto-reconnection after network issues
- ✅ PubSub publish/subscribe

### ✅ Aiven Valkey

**What is Valkey:**
Valkey is an open-source Redis-compatible in-memory data store. Aiven's Valkey service provides:

- Managed Redis-compatible service
- High availability with automatic failover
- TLS encryption by default
- 24/7 support and monitoring

**Configuration:**
```rust
RedisConfig::new("rediss://avnadmin:YOUR_PASSWORD@aiven-valkey-user.aivencloud.com:6379")
    .with_connection_timeout(30)  // Default, optimized for cloud
    .with_response_timeout(20);
```

**Getting Started:**

1. **Create Aiven Valkey Service:**
   - Go to https://aiven.io
   - Create a new Valkey service
   - Choose plan (Hobbyist, Startup, Business, etc.)
   - Select cloud provider and region
   - Wait for service to be ready (~1-2 minutes)

2. **Get Connection Details:**
   - Go to service → Overview
   - Copy "Connection Information"
   - Use "Redis connection string" or "Valkey connection string"

3. **Configure Your Application:**
   ```rust
   // From Aiven dashboard → Overview → Connection Information
   let redis_url = "rediss://avnadmin:password@valkey-project.aivencloud.com:6379";
   
   let config = RedisConfig::new(redis_url);
   let registry = SubscriberRegistry::new()
       .with_redis(redis_url)
       .register("orders", handle_orders);
   ```

**Best Practices:**
- ✅ Always use TLS (`rediss://`) - required by Aiven
- ✅ Use default timeouts (30s/20s)
- ✅ Enable Aiven's integration metrics
- ✅ Set up alerts for high CPU/memory usage
- ✅ Use Aiven's service endpoints for low latency

**Features Tested:**
- ✅ TLS connections
- ✅ All queue operations
- ✅ Connection supervision with auto-reconnect
- ✅ PubSub with auto-reconnect
- ✅ Multi-consumer distribution
- ✅ Heartbeat and consumer registration

## Configuration Comparison

| Provider | Connection Timeout | Response Timeout | Protocol | Notes |
|----------|-------------------|------------------|----------|-------|
| **Self-hosted** | 5s | 3s | `redis://` | Lower latency |
| **Upstash** | 30s | 20s | `rediss://` | Default optimal |
| **Aiven Valkey** | 30s | 20s | `rediss://` | TLS required |

## Deployment Recommendations

### For Development

**Use Self-Hosted Redis:**
```bash
# Quick start with Docker
docker run -d -p 6379:6379 redis:7-alpine
```

**Configuration:**
```rust
RedisConfig::new("redis://127.0.0.1:6379")
    .with_connection_timeout(5)
    .with_response_timeout(3);
```

### For Serverless/AWS Lambda

**Use Upstash:**
- Global edge replication
- Automatic scaling
- No connection pooling needed

**Configuration:**
```rust
RedisConfig::new(upstash_url)
    .with_connection_timeout(30)
    .with_response_timeout(20);
```

### For Production/High Availability

**Use Aiven Valkey:**
- Automatic failover
- 99.99% uptime SLA
- Backups and replication

**Configuration:**
```rust
RedisConfig::new(aiven_url)
    .with_connection_timeout(30)
    .with_response_timeout(20);
```

## Environment Variables

**Recommended pattern:**

```rust
let redis_url = std::env::var("REDIS_URL")
    .expect("REDIS_URL must be set");

let config = RedisConfig::new(&redis_url);

// Adjust timeouts based on environment
if std::env::var("ENVIRONMENT").as_deref() == Ok("production") {
    // Use cloud Redis
    let config = RedisConfig::new(&redis_url)
        .with_connection_timeout(30)
        .with_response_timeout(20);
} else {
    // Local development
    let config = RedisConfig::new(&redis_url)
        .with_connection_timeout(5)
        .with_response_timeout(3);
}
```

**Docker Compose:**
```yaml
services:
  app:
    environment:
      - REDIS_URL=redis://redis:6379
      - ENVIRONMENT=development
```

**Kubernetes:**
```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: app-config
data:
  REDIS_URL: "rediss://avnadmin:pass@valkey.aivencloud.com:6379"
  ENVIRONMENT: "production"
```

## Monitoring & Alerts

### Self-Hosted Redis

**Key Metrics:**
- Memory usage: `redis-cli INFO memory`
- Connections: `redis-cli INFO clients`
- Commands/sec: `redis-cli INFO stats`

**Monitoring:**
```bash
# Check connection
redis-cli ping

# Monitor commands
redis-cli MONITOR

# Check memory
redis-cli INFO memory | grep used_memory_human
```

### Upstash

**Use Upstash Dashboard:**
- Real-time metrics
- Command analysis
- Error rates
- Latency graphs

**URL:** https://app.upstash.com

### Aiven Valkey

**Use Aiven Dashboard:**
- Service metrics
- CPU/memory usage
- Connection counts
- Replication lag

**Alerts to Set Up:**
- High CPU usage (>80%)
- High memory usage (>80%)
- Connection failures
- Replication lag

## Troubleshooting

### Connection Timeouts

**Problem:** `Redis connection error: timed out`

**Solution:**
```rust
// Increase timeouts for slow connections
RedisConfig::new(url)
    .with_connection_timeout(60)  // 60s instead of 30s
    .with_response_timeout(45);   // 45s instead of 20s
```

### TLS Errors

**Problem:** `TLS handshake failed`

**Solution:**
```rust
// Ensure using rediss:// for TLS
let url = "rediss://...";  // Not redis://

// For Aiven, ensure using correct format
let url = "rediss://avnadmin:password@host:port";
```

### Connection Pool Exhaustion

**Problem:** Too many connections

**Solution:**
```rust
// Increase pool size
registry.register("orders", handler)
    .with_pool_size(30)  // Increase from default 20
    .build();
```

### High Memory Usage

**Problem:** Redis using too much memory

**Solution:**
```bash
# Enable eviction policy
redis-cli CONFIG SET maxmemory-policy allkeys-lru

# Or set maxmemory
redis-cli CONFIG SET maxmemory 256mb
```

## Best Practices

### 1. Always Use Environment Variables
```rust
let redis_url = std::env::var("REDIS_URL")?;
```

### 2. Use TLS in Production
```rust
// Production: rediss://
// Development: redis://
```

### 3. Configure Timeouts for Your Environment
```rust
// Cloud: 30s/20s
// Local: 5s/3s
```

### 4. Monitor Connection Health
```rust
let state = pool.supervisor().state().await;
println!("Redis state: {:?}", state);
```

### 5. Set Up Alerts
- Redis down
- High memory/CPU
- Connection failures
- Replication lag

### 6. Test Reconnection
```bash
# Stop Redis
redis-cli shutdown

# Watch logs for reconnect
# Should see: "Reconnection successful after X failures"

# Start Redis
redis-server
```

## Support

For provider-specific support:

- **Upstash:** https://docs.upstash.com
- **Aiven:** https://docs.aiven.io
- **Redis:** https://redis.io/documentation

For liteq issues:
- GitHub: https://github.com/your-repo/liteq
- Documentation: [README.md](README.md)
