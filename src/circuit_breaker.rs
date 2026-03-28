use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

#[derive(Debug, Clone, PartialEq)]
pub enum CircuitState {
    Closed,
    Open,
    HalfOpen,
}

pub struct CircuitBreaker {
    failure_threshold: u32,
    success_threshold: u32,
    timeout: Duration,
    state: Arc<RwLock<CircuitState>>,
    failure_count: Arc<AtomicU32>,
    success_count: Arc<AtomicU32>,
    last_failure_time: Arc<RwLock<Option<Instant>>>,
}

impl CircuitBreaker {
    pub fn new(failure_threshold: u32, timeout: Duration) -> Self {
        Self {
            failure_threshold,
            success_threshold: 2,
            timeout,
            state: Arc::new(RwLock::new(CircuitState::Closed)),
            failure_count: Arc::new(AtomicU32::new(0)),
            success_count: Arc::new(AtomicU32::new(0)),
            last_failure_time: Arc::new(RwLock::new(None)),
        }
    }

    pub async fn is_request_allowed(&self) -> bool {
        let state = self.state.read().await;
        
        match *state {
            CircuitState::Closed => true,
            CircuitState::Open => {
                let last_failure = self.last_failure_time.read().await;
                if let Some(failure_time) = *last_failure {
                    if failure_time.elapsed() >= self.timeout {
                        drop(state);
                        // Transition to HalfOpen
                        *self.state.write().await = CircuitState::HalfOpen;
                        self.success_count.store(0, Ordering::Relaxed);
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            CircuitState::HalfOpen => true,
        }
    }

    pub async fn record_success(&self) {
        let state = self.state.read().await;
        
        if *state == CircuitState::HalfOpen {
            let current = self.success_count.fetch_add(1, Ordering::Relaxed);
            
            if current + 1 >= self.success_threshold {
                drop(state);
                *self.state.write().await = CircuitState::Closed;
                self.failure_count.store(0, Ordering::Relaxed);
                self.success_count.store(0, Ordering::Relaxed);
            }
        } else {
            self.failure_count.store(0, Ordering::Relaxed);
        }
    }

    pub async fn record_failure(&self) {
        let current = self.failure_count.fetch_add(1, Ordering::Relaxed);

        if current + 1 >= self.failure_threshold {
            *self.state.write().await = CircuitState::Open;
            *self.last_failure_time.write().await = Some(Instant::now());
            tracing::warn!("Circuit breaker OPEN after {} failures", current + 1);
        }
    }

    pub async fn state(&self) -> CircuitState {
        self.state.read().await.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_circuit_breaker_opens_after_threshold() {
        let breaker = CircuitBreaker::new(3, Duration::from_secs(5));
        
        assert!(breaker.is_request_allowed().await);
        
        breaker.record_failure().await;
        breaker.record_failure().await;
        assert!(breaker.is_request_allowed().await);
        
        breaker.record_failure().await;
        assert!(!breaker.is_request_allowed().await);
        assert_eq!(breaker.state().await, CircuitState::Open);
    }

    #[tokio::test]
    async fn test_circuit_breaker_transitions_to_half_open() {
        let breaker = CircuitBreaker::new(2, Duration::from_millis(100));
        
        breaker.record_failure().await;
        breaker.record_failure().await;
        
        assert_eq!(breaker.state().await, CircuitState::Open);
        assert!(!breaker.is_request_allowed().await);
        
        tokio::time::sleep(Duration::from_millis(150)).await;
        
        assert!(breaker.is_request_allowed().await);
        assert_eq!(breaker.state().await, CircuitState::HalfOpen);
    }

    #[tokio::test]
    async fn test_circuit_breaker_closes_after_successes() {
        let breaker = CircuitBreaker::new(2, Duration::from_millis(100));
        
        breaker.record_failure().await;
        breaker.record_failure().await;
        
        tokio::time::sleep(Duration::from_millis(150)).await;
        
        breaker.is_request_allowed().await;
        breaker.record_success().await;
        breaker.record_success().await;
        
        assert_eq!(breaker.state().await, CircuitState::Closed);
    }
}
