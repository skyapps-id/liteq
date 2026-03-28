use crate::config::{QueueConfig, RedisConfig};
use crate::connection_supervisor::ConnectionState;
use crate::error::JobResult;
use crate::metrics::MetricsRegistry;
use crate::pool::RedisPool;
use crate::queue::JobQueue;
use crate::retry::RetryConfig;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::time::{sleep, Duration};

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
}

struct QueueGroupWithData {
    pool: Arc<RedisPool>,
    queue: Arc<JobQueue>,
    workers: Vec<(HandlerWithData, Arc<dyn std::any::Any + Send + Sync>, usize)>,
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

    pub fn with_redis(mut self, url: impl Into<String>) -> Self {
        self.redis_config = RedisConfig::new(url);
        self
    }

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

    pub async fn run(self) -> JobResult<()> {
        tracing::info!("SubscriberRegistry starting...");
        tracing::info!("Metrics enabled");
        tracing::info!("Connection supervisor (RabbitMQ-style)");
        tracing::info!("Press Ctrl+C to stop");

        let mut queue_groups: HashMap<String, QueueGroup> = HashMap::new();
        let mut queue_groups_with_data: HashMap<String, QueueGroupWithData> = HashMap::new();

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

                tracing::info!(
                    queue = %worker_config.queue,
                    max_size = pool.status().await.max_size,
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

                tracing::info!(
                    queue = %worker_config.queue,
                    max_size = pool.status().await.max_size,
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
                    },
                );
            }

            if let Some(group) = queue_groups_with_data.get_mut(&worker_config.queue) {
                group.workers.push((worker_config.handler, worker_config.data, worker_config.concurrency));
            }
        }

        let mut handles = Vec::new();

        for (queue_name, group) in queue_groups {
            for (worker_idx, (handler, concurrency)) in group.workers.into_iter().enumerate() {
                for i in 0..concurrency {
                    let queue_clone = group.queue.clone();
                    let handler = handler.clone();
                    let pool = group.pool.clone();
                    let queue_name = queue_name.clone();
                    let metrics = self.metrics.clone();

                    let handle = tokio::spawn(async move {
                        tracing::info!(
                            worker_id = format!("{}-{}", worker_idx, i),
                            queue = %queue_name,
                            "Worker #{}-{} started for queue: {}",
                            worker_idx, i, queue_name
                        );

                        loop {
                            if let Ok(Some(job)) = queue_clone.dequeue::<Value>().await {
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
            for (worker_idx, (handler, data, concurrency)) in group.workers.into_iter().enumerate() {
                for i in 0..concurrency {
                    let queue_clone = group.queue.clone();
                    let handler = handler.clone();
                    let pool = group.pool.clone();
                    let queue_name = queue_name.clone();
                    let metrics = self.metrics.clone();
                    let data_clone = data.clone();

                    let handle = tokio::spawn(async move {
                        tracing::info!(
                            worker_id = format!("{}-{}", worker_idx, i),
                            queue = %queue_name,
                            has_di = true,
                            "Worker #{}-{} started for queue: {} (with DI)",
                            worker_idx, i, queue_name
                        );

                        loop {
                            if let Ok(Some(job)) = queue_clone.dequeue::<Value>().await {
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

    pub async fn get_pool_status(&self, queue_name: &str) -> Option<crate::metrics::PoolStatus> {
        self.metrics.get_pool_status(queue_name).await
    }

    pub async fn get_metrics(&self, queue_name: &str) -> Option<PerformanceMetrics> {
        self.metrics.get_metrics(queue_name).await
    }

    pub async fn get_all_metrics(&self) -> HashMap<String, PerformanceMetrics> {
        self.metrics.get_all_metrics().await
    }

    pub async fn health_check(&self, queue_name: &str) -> QueueHealth {
        self.metrics.health_check(queue_name).await
    }

    pub async fn health_check_all(&self) -> HashMap<String, QueueHealth> {
        self.metrics.health_check_all().await
    }

    /// Get job counts for a specific queue from metrics
    pub async fn get_job_counts(&self, queue_name: &str) -> Option<(usize, usize)> {
        let health = self.metrics.health_check(queue_name).await;
        Some((health.regular_jobs_count, health.scheduled_jobs_count))
    }
}

impl<'a, F> QueueBuilder<'a, F>
where
    F: Send + Sync + 'static,
{
    pub fn with_concurrency(mut self, concurrency: usize) -> Self {
        self.concurrency = concurrency;
        self
    }

    pub fn with_pool_size(mut self, pool_size: usize) -> Self {
        self.pool_size = Some(pool_size);
        self
    }

    pub fn with_min_idle(mut self, min_idle: usize) -> Self {
        self.min_idle = Some(min_idle);
        self
    }
}

impl<'a, F> QueueBuilder<'a, F>
where
    F: Fn(Vec<u8>) -> JobResult<()> + Send + Sync + 'static,
{
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
    pub fn with_concurrency(mut self, concurrency: usize) -> Self {
        self.concurrency = concurrency;
        self
    }

    pub fn with_pool_size(mut self, pool_size: usize) -> Self {
        self.pool_size = Some(pool_size);
        self
    }

    pub fn with_min_idle(mut self, min_idle: usize) -> Self {
        self.min_idle = Some(min_idle);
        self
    }

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
