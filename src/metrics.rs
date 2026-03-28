use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;

#[derive(Debug, Clone, PartialEq)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

#[derive(Debug, Clone)]
pub struct PoolStatus {
    pub max_size: usize,
    pub size: usize,
    pub available: usize,
    pub active: usize,
    pub waiting: usize,
    pub scheduled_jobs: usize,
    pub regular_jobs: usize,
}

#[derive(Debug, Clone)]
pub struct PerformanceMetrics {
    pub total_operations: u64,
    pub successful_operations: u64,
    pub failed_operations: u64,
    pub total_latency_ms: f64,
    pub operation_count: usize,
    pub last_operation_time: Option<Instant>,
}

impl PerformanceMetrics {
    pub fn new() -> Self {
        Self {
            total_operations: 0,
            successful_operations: 0,
            failed_operations: 0,
            total_latency_ms: 0.0,
            operation_count: 0,
            last_operation_time: None,
        }
    }

    pub fn record_success(&mut self, latency_ms: f64) {
        self.total_operations += 1;
        self.successful_operations += 1;
        self.total_latency_ms += latency_ms;
        self.operation_count += 1;
        self.last_operation_time = Some(Instant::now());
    }

    pub fn record_failure(&mut self) {
        self.total_operations += 1;
        self.failed_operations += 1;
        self.last_operation_time = Some(Instant::now());
    }
}

impl Default for PerformanceMetrics {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct QueueHealth {
    pub status: HealthStatus,
    pub pool_status: PoolStatus,
    pub metrics: PerformanceMetrics,
    pub error_rate: f64,
    pub last_check: Instant,
    pub scheduled_jobs_count: usize,
    pub regular_jobs_count: usize,
    pub total_pending_jobs: usize,
}

pub struct MetricsRegistry {
    queue_metrics: Arc<RwLock<HashMap<String, QueueMetrics>>>,
}

#[derive(Debug, Clone)]
struct QueueMetrics {
    pool_status: PoolStatus,
    performance: PerformanceMetrics,
    errors: Vec<(Instant, String)>,
}

impl MetricsRegistry {
    pub fn new() -> Self {
        Self {
            queue_metrics: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn register_queue(&self, queue_name: &str, pool_size: usize) {
        let mut metrics = self.queue_metrics.write().await;
        metrics.insert(
            queue_name.to_string(),
            QueueMetrics {
                pool_status: PoolStatus {
                    max_size: pool_size,
                    size: 0,
                    available: 0,
                    active: 0,
                    waiting: 0,
                    scheduled_jobs: 0,
                    regular_jobs: 0,
                },
                performance: PerformanceMetrics::new(),
                errors: Vec::new(),
            },
        );
    }

    pub async fn record_operation(&self, queue_name: &str, latency_ms: f64, success: bool) {
        let mut metrics = self.queue_metrics.write().await;
        if let Some(queue_metrics) = metrics.get_mut(queue_name) {
            if success {
                queue_metrics.performance.record_success(latency_ms);
            } else {
                queue_metrics.performance.record_failure();
            }
        }
    }

    pub async fn record_error(&self, queue_name: &str, error: String) {
        let mut metrics = self.queue_metrics.write().await;
        if let Some(queue_metrics) = metrics.get_mut(queue_name) {
            queue_metrics.errors.push((Instant::now(), error));
            // Keep only last 100 errors
            if queue_metrics.errors.len() > 100 {
                queue_metrics.errors.remove(0);
            }
        }
    }

    pub async fn update_job_counts(&self, queue_name: &str, scheduled: usize, regular: usize) {
        let mut metrics = self.queue_metrics.write().await;
        if let Some(queue_metrics) = metrics.get_mut(queue_name) {
            queue_metrics.pool_status.scheduled_jobs = scheduled;
            queue_metrics.pool_status.regular_jobs = regular;
        }
    }

    pub async fn get_pool_status(&self, queue_name: &str) -> Option<PoolStatus> {
        let metrics = self.queue_metrics.read().await;
        metrics.get(queue_name).map(|m| m.pool_status.clone())
    }

    pub async fn get_metrics(&self, queue_name: &str) -> Option<PerformanceMetrics> {
        let metrics = self.queue_metrics.read().await;
        metrics.get(queue_name).map(|m| m.performance.clone())
    }

    pub async fn get_all_metrics(&self) -> HashMap<String, PerformanceMetrics> {
        let metrics = self.queue_metrics.read().await;
        metrics
            .iter()
            .map(|(k, v)| (k.clone(), v.performance.clone()))
            .collect()
    }

    pub async fn health_check(&self, queue_name: &str) -> QueueHealth {
        let metrics = self.queue_metrics.read().await;
        if let Some(queue_metrics) = metrics.get(queue_name) {
            let error_rate = if queue_metrics.performance.total_operations > 0 {
                queue_metrics.performance.failed_operations as f64
                    / queue_metrics.performance.total_operations as f64
            } else {
                0.0
            };

            let total_jobs = queue_metrics.pool_status.scheduled_jobs + queue_metrics.pool_status.regular_jobs;

            // Health status considers both errors and job backlog
            let status = if error_rate > 0.5 || queue_metrics.pool_status.available == 0 {
                HealthStatus::Unhealthy
            } else if error_rate > 0.1 || queue_metrics.pool_status.available < 2
                || total_jobs > 10000 {
                // Degraded if: high error rate, low available connections, OR large backlog
                HealthStatus::Degraded
            } else {
                HealthStatus::Healthy
            };

            QueueHealth {
                status,
                pool_status: queue_metrics.pool_status.clone(),
                metrics: queue_metrics.performance.clone(),
                error_rate,
                last_check: Instant::now(),
                scheduled_jobs_count: queue_metrics.pool_status.scheduled_jobs,
                regular_jobs_count: queue_metrics.pool_status.regular_jobs,
                total_pending_jobs: total_jobs,
            }
        } else {
            QueueHealth {
                status: HealthStatus::Unhealthy,
                pool_status: PoolStatus {
                    max_size: 0,
                    size: 0,
                    available: 0,
                    active: 0,
                    waiting: 0,
                    scheduled_jobs: 0,
                    regular_jobs: 0,
                },
                metrics: PerformanceMetrics::new(),
                error_rate: 1.0,
                last_check: Instant::now(),
                scheduled_jobs_count: 0,
                regular_jobs_count: 0,
                total_pending_jobs: 0,
            }
        }
    }

    pub async fn health_check_all(&self) -> HashMap<String, QueueHealth> {
        let metrics = self.queue_metrics.read().await;
        let mut result = HashMap::new();

        for (queue_name, _) in metrics.iter() {
            result.insert(queue_name.clone(), self.health_check(queue_name).await);
        }

        result
    }
}

impl Default for MetricsRegistry {
    fn default() -> Self {
        Self::new()
    }
}
