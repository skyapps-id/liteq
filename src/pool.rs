use crate::config::RedisConfig;
use crate::connection_supervisor::ConnectionSupervisor;
use crate::error::JobResult;
use crate::metrics::{MetricsRegistry, PoolStatus};
use redis::aio::ConnectionManager;
use std::sync::Arc;
use std::time::Instant;

#[derive(Clone)]
pub struct RedisPool {
    supervisor: Arc<ConnectionSupervisor>,
    metrics: Arc<MetricsRegistry>,
    queue_name: String,
    pool_size: usize,
}

impl RedisPool {
    /// Creates new pool with default config
    pub async fn new(config: RedisConfig, queue_name: &str, metrics: Arc<MetricsRegistry>) -> JobResult<Self> {
        Self::with_config(config, queue_name, metrics, None, None).await
    }

    /// Creates new pool with custom config
    pub async fn with_config(
        config: RedisConfig,
        queue_name: &str,
        metrics: Arc<MetricsRegistry>,
        pool_size: Option<usize>,
        _min_idle: Option<usize>,
    ) -> JobResult<Self> {
        let actual_pool_size = pool_size.unwrap_or(config.pool_size);

        let supervisor = Arc::new(ConnectionSupervisor::new(config));
        supervisor.start().await?;

        metrics.register_queue(queue_name, actual_pool_size).await;

        Ok(Self {
            supervisor,
            metrics,
            queue_name: queue_name.to_string(),
            pool_size: actual_pool_size,
        })
    }

    /// Gets connection with automatic metrics tracking
    pub async fn get_connection(&self) -> JobResult<ConnectionManager> {
        let start = Instant::now();
        let result = self.supervisor.get_connection().await;
        let latency = start.elapsed().as_millis() as f64;

        match result {
            Ok(conn) => {
                self.metrics
                    .record_operation(&self.queue_name, latency, true)
                    .await;
                Ok(conn)
            }
            Err(e) => {
                self.metrics
                    .record_operation(&self.queue_name, latency, false)
                    .await;
                self.metrics
                    .record_error(&self.queue_name, format!("Connection failed: {}", e))
                    .await;
                Err(e)
            }
        }
    }

    /// Returns pool status
    pub async fn status(&self) -> PoolStatus {
        self.metrics
            .get_pool_status(&self.queue_name)
            .await
            .unwrap_or(PoolStatus {
                max_size: self.pool_size,
                size: 0,
                available: self.pool_size,
                active: 0,
                waiting: 0,
                scheduled_jobs: 0,
                regular_jobs: 0,
            })
    }

    /// Returns reference to supervisor
    pub fn supervisor(&self) -> &Arc<ConnectionSupervisor> {
        &self.supervisor
    }
}
