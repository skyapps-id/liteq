use crate::config::{QueueConfig, RedisConfig, ConsumerInfo};
use crate::connection_supervisor::ConnectionState;
use crate::consumer_registry::ConsumerRegistry;
use crate::error::JobResult;
use crate::metrics::MetricsRegistry;
use crate::pool::RedisPool;
use crate::queue::JobQueue;
use crate::retry::RetryConfig;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use futures_util::future::join_all;

type Handler = Arc<dyn Fn(Vec<u8>) -> JobResult<()> + Send + Sync>;

type HandlerWithData = Arc<dyn Fn(Vec<u8>, Arc<dyn std::any::Any + Send + Sync>) -> JobResult<()> + Send + Sync>;

struct WorkerConfig {
    queue: String,
    handler: Handler,
    concurrency: usize,
    pool_size: Option<usize>,
    min_idle: Option<usize>,
}

struct WorkerConfigWithData {
    queue: String,
    handler: HandlerWithData,
    data: Arc<dyn std::any::Any + Send + Sync>,
    concurrency: usize,
    pool_size: Option<usize>,
    min_idle: Option<usize>,
}

struct QueueGroup {
    pool: Arc<RedisPool>,
    queue: Arc<JobQueue>,
    workers: Vec<(Handler, usize)>,
    consumer_info: Option<ConsumerInfo>,
}

struct QueueGroupWithData {
    pool: Arc<RedisPool>,
    queue: Arc<JobQueue>,
    workers: Vec<(HandlerWithData, Arc<dyn std::any::Any + Send + Sync>, usize)>,
    consumer_info: Option<ConsumerInfo>,
}

pub struct SubscriberRegistry {
    redis_config: RedisConfig,
    retry_config: RetryConfig,
    metrics: Arc<MetricsRegistry>,
    workers: Vec<WorkerConfig>,
    workers_with_data: Vec<WorkerConfigWithData>,
}

pub struct QueueBuilder<'a, F = Handler> {
    registry: &'a mut SubscriberRegistry,
    queue: String,
    handler: Option<F>,
    concurrency: usize,
    pool_size: Option<usize>,
    min_idle: Option<usize>,
}

impl SubscriberRegistry {
    /// Creates new registry
    pub fn new() -> Self {
        Self {
            redis_config: RedisConfig::new("redis://127.0.0.1:6379"),
            retry_config: RetryConfig::new()
                .with_max_attempts(20)
                .with_initial_delay(500)
                .with_max_delay(30000),
            metrics: Arc::new(MetricsRegistry::new()),
            workers: Vec::new(),
            workers_with_data: Vec::new(),
        }
    }

    /// Sets Redis connection URL
    pub fn with_redis(mut self, url: impl Into<String>) -> Self {
        self.redis_config = RedisConfig::new(url);
        self
    }

    /// Registers queue with handler (returns builder)
    pub fn register<'a, F>(&'a mut self, queue: impl Into<String>, handler: F) -> QueueBuilder<'a, F>
    where
        F: Send + Sync + 'static,
    {
        QueueBuilder {
            registry: self,
            queue: queue.into(),
            handler: Some(handler),
            concurrency: 1,
            pool_size: None,
            min_idle: None,
        }
    }

    pub fn with_concurrency(mut self, concurrency: usize) -> Self {
        if let Some(worker) = self.workers.last_mut() {
            worker.concurrency = concurrency;
        }
        self
    }

    pub fn with_pool_size(mut self, pool_size: usize) -> Self {
        if let Some(worker) = self.workers.last_mut() {
            worker.pool_size = Some(pool_size);
        }
        self
    }

    pub fn with_min_idle(mut self, min_idle: usize) -> Self {
        if let Some(worker) = self.workers.last_mut() {
            worker.min_idle = Some(min_idle);
        }
        self
    }

    /// Starts all workers (runs until Ctrl+C)
    pub async fn run(self) -> JobResult<()> {
        tracing::info!("SubscriberRegistry starting...");
        tracing::info!("Metrics enabled");
        tracing::info!("Connection supervisor (RabbitMQ-style)");
        tracing::info!("Press Ctrl+C to stop");

        let mut queue_groups: HashMap<String, QueueGroup> = HashMap::new();
        let mut queue_groups_with_data: HashMap<String, QueueGroupWithData> = HashMap::new();

        // Get unique queues and register consumers
        let mut unique_queues: std::collections::HashSet<String> = std::collections::HashSet::new();
        for worker_config in &self.workers {
            unique_queues.insert(worker_config.queue.clone());
        }
        for worker_config in &self.workers_with_data {
            unique_queues.insert(worker_config.queue.clone());
        }

        // Register consumer for each unique queue (in parallel)
        let consumer_registry = Arc::new(ConsumerRegistry::new(self.redis_config.clone()));
        let mut consumer_infos: HashMap<String, ConsumerInfo> = HashMap::new();

        let registration_tasks: Vec<_> = unique_queues
            .iter()
            .map(|queue_name| {
                let queue_name = queue_name.clone();
                let consumer_registry = consumer_registry.clone();
                tokio::spawn(async move {
                    let result = consumer_registry.register_and_start_heartbeat(&queue_name).await;
                    (queue_name, result)
                })
            })
            .collect();

        let results = join_all(registration_tasks).await;

        for result in results {
            if let Ok((queue_name, registration_result)) = result {
                match registration_result {
                    Ok(info) => {
                        tracing::info!(
                            queue = %queue_name,
                            consumer_id = info.id,
                            total_consumers = info.total,
                            "Auto-registered consumer for queue"
                        );
                        consumer_infos.insert(queue_name.clone(), info);
                    }
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            queue = %queue_name,
                            "Failed to register consumer, will use single-consumer mode"
                        );
                    }
                }
            }
        }

        for worker_config in self.workers {
            if !queue_groups.contains_key(&worker_config.queue) {
                let pool = Arc::new(
                    RedisPool::with_config(
                        self.redis_config.clone(),
                        &worker_config.queue,
                        self.metrics.clone(),
                        worker_config.pool_size,
                        worker_config.min_idle,
                    )
                    .await?,
                );

                let queue = Arc::new(
                    JobQueue::new(
                        QueueConfig::new(&worker_config.queue),
                        self.redis_config.clone(),
                    )
                    .await?
                    .with_retry_config(self.retry_config.clone()),
                );

                let consumer_info = consumer_infos.get(&worker_config.queue).cloned();

                tracing::info!(
                    queue = %worker_config.queue,
                    max_size = pool.status().await.max_size,
                    consumer_id = consumer_info.as_ref().map(|c| c.id).unwrap_or(0),
                    total_consumers = consumer_info.as_ref().map(|c| c.total).unwrap_or(1),
                    "Queue '{}' initialized with supervisor (max: {})",
                    worker_config.queue,
                    pool.status().await.max_size
                );

                queue_groups.insert(
                    worker_config.queue.clone(),
                    QueueGroup {
                        pool,
                        queue,
                        workers: Vec::new(),
                        consumer_info,
                    },
                );
            }

            if let Some(group) = queue_groups.get_mut(&worker_config.queue) {
                group.workers.push((worker_config.handler, worker_config.concurrency));
            }
        }

        for worker_config in self.workers_with_data {
            if !queue_groups_with_data.contains_key(&worker_config.queue) {
                let pool = Arc::new(
                    RedisPool::with_config(
                        self.redis_config.clone(),
                        &worker_config.queue,
                        self.metrics.clone(),
                        worker_config.pool_size,
                        worker_config.min_idle,
                    )
                    .await?,
                );

                let queue = Arc::new(
                    JobQueue::new(
                        QueueConfig::new(&worker_config.queue),
                        self.redis_config.clone(),
                    )
                    .await?
                    .with_retry_config(self.retry_config.clone()),
                );

                let consumer_info = consumer_infos.get(&worker_config.queue).cloned();

                tracing::info!(
                    queue = %worker_config.queue,
                    max_size = pool.status().await.max_size,
                    consumer_id = consumer_info.as_ref().map(|c| c.id).unwrap_or(0),
                    total_consumers = consumer_info.as_ref().map(|c| c.total).unwrap_or(1),
                    "Queue '{}' initialized with supervisor and DI (max: {})",
                    worker_config.queue,
                    pool.status().await.max_size
                );

                queue_groups_with_data.insert(
                    worker_config.queue.clone(),
                    QueueGroupWithData {
                        pool,
                        queue,
                        workers: Vec::new(),
                        consumer_info,
                    },
                );
            }

            if let Some(group) = queue_groups_with_data.get_mut(&worker_config.queue) {
                group.workers.push((worker_config.handler, worker_config.data, worker_config.concurrency));
            }
        }

        let mut handles = Vec::new();

        for (queue_name, group) in queue_groups {
            let consumer_info = group.consumer_info.clone();
            for (worker_idx, (handler, concurrency)) in group.workers.into_iter().enumerate() {
                for i in 0..concurrency {
                    let queue_clone = group.queue.clone();
                    let handler = handler.clone();
                    let pool = group.pool.clone();
                    let queue_name = queue_name.clone();
                    let metrics = self.metrics.clone();
                    let consumer_info_clone = consumer_info.clone();

                    let handle = tokio::spawn(async move {
                        tracing::info!(
                            worker_id = format!("{}-{}", worker_idx, i),
                            queue = %queue_name,
                            consumer_id = consumer_info_clone.as_ref().map(|c| c.id).unwrap_or(0),
                            total_consumers = consumer_info_clone.as_ref().map(|c| c.total).unwrap_or(1),
                            "Worker #{}-{} started for queue: {}",
                            worker_idx, i, queue_name
                        );

                        loop {
                            let job_result = if let Some(ref ci) = consumer_info_clone {
                                queue_clone.dequeue_with_consumer::<Value>(ci).await
                            } else {
                                queue_clone.dequeue::<Value>().await
                            };

                            if let Ok(Some(job)) = job_result {
                                tracing::info!(
                                    job_id = %job.id,
                                    queue = %job.queue,
                                    "JOB RECEIVED - Job ID: {}, Queue: {}",
                                    job.id, job.queue
                                );

                                let data = serde_json::to_vec(&job.payload).unwrap_or_default();

                                let start = std::time::Instant::now();
                                let result = handler(data);
                                let latency = start.elapsed().as_millis() as f64;

                                if let Err(e) = result {
                                    tracing::error!(
                                        error = %e,
                                        latency_ms = latency,
                                        "Error processing job: {}",
                                        e
                                    );
                                    metrics
                                        .record_operation(&queue_name, latency, false)
                                        .await;
                                } else {
                                    tracing::debug!(
                                        latency_ms = latency,
                                        "Job processed successfully in {}ms",
                                        latency
                                    );
                                    metrics
                                        .record_operation(&queue_name, latency, true)
                                        .await;
                                }

                                let conn_state = pool.supervisor().state().await;
                                if conn_state != ConnectionState::Connected {
                                    tracing::warn!(
                                        state = ?conn_state,
                                        "Connection state: {:?}",
                                        conn_state
                                    );
                                }
                            }

                            sleep(Duration::from_millis(500)).await;
                        }
                    });

                    handles.push(handle);
                }
            }
        }

        for (queue_name, group) in queue_groups_with_data {
            let consumer_info = group.consumer_info.clone();
            for (worker_idx, (handler, data, concurrency)) in group.workers.into_iter().enumerate() {
                for i in 0..concurrency {
                    let queue_clone = group.queue.clone();
                    let handler = handler.clone();
                    let pool = group.pool.clone();
                    let queue_name = queue_name.clone();
                    let metrics = self.metrics.clone();
                    let data_clone = data.clone();
                    let consumer_info_clone = consumer_info.clone();

                    let handle = tokio::spawn(async move {
                        tracing::info!(
                            worker_id = format!("{}-{}", worker_idx, i),
                            queue = %queue_name,
                            has_di = true,
                            consumer_id = consumer_info_clone.as_ref().map(|c| c.id).unwrap_or(0),
                            total_consumers = consumer_info_clone.as_ref().map(|c| c.total).unwrap_or(1),
                            "Worker #{}-{} started for queue: {} (with DI)",
                            worker_idx, i, queue_name
                        );

                        loop {
                            let job_result = if let Some(ref ci) = consumer_info_clone {
                                queue_clone.dequeue_with_consumer::<Value>(ci).await
                            } else {
                                queue_clone.dequeue::<Value>().await
                            };

                            if let Ok(Some(job)) = job_result {
                                tracing::info!(
                                    job_id = %job.id,
                                    queue = %job.queue,
                                    "JOB RECEIVED - Job ID: {}, Queue: {}",
                                    job.id, job.queue
                                );

                                let job_data = serde_json::to_vec(&job.payload).unwrap_or_default();

                                let start = std::time::Instant::now();
                                let result = handler(job_data, data_clone.clone());
                                let latency = start.elapsed().as_millis() as f64;

                                if let Err(e) = result {
                                    tracing::error!(
                                        error = %e,
                                        latency_ms = latency,
                                        "Error processing job: {}",
                                        e
                                    );
                                    metrics
                                        .record_operation(&queue_name, latency, false)
                                        .await;
                                } else {
                                    tracing::debug!(
                                        latency_ms = latency,
                                        "Job processed successfully in {}ms",
                                        latency
                                    );
                                    metrics
                                        .record_operation(&queue_name, latency, true)
                                        .await;
                                }

                                let conn_state = pool.supervisor().state().await;
                                if conn_state != ConnectionState::Connected {
                                    tracing::warn!(
                                        state = ?conn_state,
                                        "Connection state: {:?}",
                                        conn_state
                                    );
                                }
                            }

                            sleep(Duration::from_millis(500)).await;
                        }
                    });

                    handles.push(handle);
                }
            }
        }

        for handle in handles {
            handle.await?;
        }

        Ok(())
    }

    /// Returns pool status for queue
    pub async fn get_pool_status(&self, queue_name: &str) -> Option<crate::metrics::PoolStatus> {
        self.metrics.get_pool_status(queue_name).await
    }

    /// Returns metrics for queue
    pub async fn get_metrics(&self, queue_name: &str) -> Option<PerformanceMetrics> {
        self.metrics.get_metrics(queue_name).await
    }

    /// Returns metrics for all queues
    pub async fn get_all_metrics(&self) -> HashMap<String, PerformanceMetrics> {
        self.metrics.get_all_metrics().await
    }

    /// Returns health status for queue
    pub async fn health_check(&self, queue_name: &str) -> QueueHealth {
        self.metrics.health_check(queue_name).await
    }

    /// Returns health status for all queues
    pub async fn health_check_all(&self) -> HashMap<String, QueueHealth> {
        self.metrics.health_check_all().await
    }

    /// Returns (regular_count, scheduled_count)
    pub async fn get_job_counts(&self, queue_name: &str) -> Option<(usize, usize)> {
        let health = self.metrics.health_check(queue_name).await;
        Some((health.regular_jobs_count, health.scheduled_jobs_count))
    }
}

impl<'a, F> QueueBuilder<'a, F>
where
    F: Send + Sync + 'static,
{
    /// Sets the number of concurrent workers for this queue
    ///
    /// # Arguments
    /// * `concurrency` - Number of parallel worker tasks to spawn
    ///
    /// # Returns
    /// * `Self` - Returns self for method chaining
    ///
    /// # Default
    /// 1 (single worker)
    ///
    /// # Example
    /// ```ignore
    /// registry.register("orders", handler)
    ///     .with_concurrency(10)  // 10 parallel workers
    ///     .build();
    /// ```
    pub fn with_concurrency(mut self, concurrency: usize) -> Self {
        self.concurrency = concurrency;
        self
    }

    /// Sets the maximum size of the connection pool for this queue
    ///
    /// # Arguments
    /// * `pool_size` - Maximum number of Redis connections in the pool
    ///
    /// # Returns
    /// * `Self` - Returns self for method chaining
    ///
    /// # Default
    /// None (uses pool default)
    ///
    /// # Example
    /// ```ignore
    /// registry.register("orders", handler)
    ///     .with_pool_size(20)  // Max 20 connections
    ///     .build();
    /// ```
    pub fn with_pool_size(mut self, pool_size: usize) -> Self {
        self.pool_size = Some(pool_size);
        self
    }

    /// Sets the minimum number of idle connections to maintain
    ///
    /// # Arguments
    /// * `min_idle` - Minimum idle connections to keep open
    ///
    /// # Returns
    /// * `Self` - Returns self for method chaining
    ///
    /// # Default
    /// None (uses pool default)
    ///
    /// # Use Case
    /// Ensures connections are ready for bursts of activity
    ///
    /// # Example
    /// ```ignore
    /// registry.register("orders", handler)
    ///     .with_min_idle(5)  // Keep 5 connections ready
    ///     .build();
    /// ```
    pub fn with_min_idle(mut self, min_idle: usize) -> Self {
        self.min_idle = Some(min_idle);
        self
    }
}

impl<'a, F> QueueBuilder<'a, F>
where
    F: Fn(Vec<u8>) -> JobResult<()> + Send + Sync + 'static,
{
    /// Builds and registers the queue configuration
    ///
    /// # Behavior
    /// - Adds the queue to the registry
    /// - Spawns workers when `run()` is called
    /// - Workers will process jobs from the queue
    ///
    /// # Example
    /// ```ignore
    /// registry.register("orders", |data| {
    ///     let order: Order = serde_json::from_slice(&data)?;
    ///     process_order(order)?;
    ///     Ok(())
    /// })
    /// .with_concurrency(5)
    /// .build();  // Must call build() to register
    /// ```
    pub fn build(mut self) {
        if let Some(handler) = self.handler.take() {
            self.registry.workers.push(WorkerConfig {
                queue: self.queue.clone(),
                handler: Arc::new(handler),
                concurrency: self.concurrency,
                pool_size: self.pool_size,
                min_idle: self.min_idle,
            });
        }
    }
}

pub struct QueueBuilderWithData<'a, T>
where
    T: Send + Sync + 'static,
{
    registry: &'a mut SubscriberRegistry,
    queue: String,
    handler: HandlerWithData,
    data: Arc<T>,
    concurrency: usize,
    pool_size: Option<usize>,
    min_idle: Option<usize>,
}

impl<'a, F> QueueBuilder<'a, F>
where
    F: Send + Sync + 'static,
{
    /// Adds dependency injection support to the handler
    ///
    /// # Type Parameters
    /// * `T` - Type of shared data (must be Send + Sync + 'static)
    ///
    /// # Arguments
    /// * `data` - Shared data to pass to all worker instances
    ///
    /// # Returns
    /// * `QueueBuilderWithData` - Builder with DI support
    ///
    /// # Use Case
    /// Share database connections, HTTP clients, or other resources across workers
    ///
    /// # Example
    /// ```ignore
    /// struct AppContext {
    ///     db: Database,
    ///     http_client: HttpClient,
    /// }
    ///
    /// let ctx = AppContext { /* ... */ };
    ///
    /// registry.register("orders", |data, ctx| {
    ///     let order: Order = serde_json::from_slice(&data)?;
    ///     ctx.db.save_order(&order)?;
    ///     Ok(())
    /// })
    /// .with_data(ctx)
    /// .with_concurrency(10)
    /// .build();
    /// ```
    pub fn with_data<T>(mut self, data: T) -> QueueBuilderWithData<'a, T>
    where
        F: Fn(Vec<u8>, Arc<T>) -> JobResult<()> + 'static,
        T: Send + Sync + 'static,
    {
        let data_arc = Arc::new(data);
        let data_clone = data_arc.clone();

        let handler = self.handler.take().expect("Handler not set");
        let wrapper = Arc::new(move |job_data: Vec<u8>, _any_data: Arc<dyn std::any::Any + Send + Sync>| -> JobResult<()> {
            handler(job_data, data_clone.clone())
        });

        QueueBuilderWithData {
            registry: self.registry,
            queue: self.queue.clone(),
            handler: wrapper,
            data: data_arc,
            concurrency: self.concurrency,
            pool_size: self.pool_size,
            min_idle: self.min_idle,
        }
    }
}

impl<'a, T> QueueBuilderWithData<'a, T>
where
    T: Send + Sync + 'static,
{
    /// Sets the number of concurrent workers for this queue (with DI)
    ///
    /// # Arguments
    /// * `concurrency` - Number of parallel worker tasks to spawn
    ///
    /// # Returns
    /// * `Self` - Returns self for method chaining
    ///
    /// # Default
    /// 1 (single worker)
    ///
    /// # Example
    /// ```ignore
    /// registry.register("orders", handler)
    ///     .with_data(ctx)
    ///     .with_concurrency(10)  // 10 parallel workers with shared ctx
    ///     .build();
    /// ```
    pub fn with_concurrency(mut self, concurrency: usize) -> Self {
        self.concurrency = concurrency;
        self
    }

    /// Sets the maximum size of the connection pool for this queue (with DI)
    ///
    /// # Arguments
    /// * `pool_size` - Maximum number of Redis connections in the pool
    ///
    /// # Returns
    /// * `Self` - Returns self for method chaining
    ///
    /// # Default
    /// None (uses pool default)
    pub fn with_pool_size(mut self, pool_size: usize) -> Self {
        self.pool_size = Some(pool_size);
        self
    }

    /// Sets the minimum number of idle connections to maintain (with DI)
    ///
    /// # Arguments
    /// * `min_idle` - Minimum idle connections to keep open
    ///
    /// # Returns
    /// * `Self` - Returns self for method chaining
    ///
    /// # Default
    /// None (uses pool default)
    pub fn with_min_idle(mut self, min_idle: usize) -> Self {
        self.min_idle = Some(min_idle);
        self
    }

    /// Builds and registers the queue configuration with dependency injection
    ///
    /// # Behavior
    /// - Adds the queue to the registry with shared data
    /// - Spawns workers with access to shared data when `run()` is called
    /// - All workers share the same data instance via Arc
    ///
    /// # Example
    /// ```ignore
    /// let ctx = AppContext::new();
    ///
    /// registry.register("orders", |data, ctx| {
    ///     let order: Order = serde_json::from_slice(&data)?;
    ///     ctx.db.save_order(&order)?;
    ///     Ok(())
    /// })
    /// .with_data(ctx)
    /// .with_concurrency(10)
    /// .build();  // Must call build() to register
    /// ```
    pub fn build(self) {
        self.registry.workers_with_data.push(WorkerConfigWithData {
            queue: self.queue.clone(),
            handler: self.handler,
            data: self.data as Arc<dyn std::any::Any + Send + Sync>,
            concurrency: self.concurrency,
            pool_size: self.pool_size,
            min_idle: self.min_idle,
        });
    }
}

impl Default for SubscriberRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub use crate::metrics::{QueueHealth, PerformanceMetrics};
