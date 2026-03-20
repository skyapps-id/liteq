use crate::error::JobResult;
use crate::job::Job;
use crate::queue::JobQueue;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub type Handler<T> = Arc<dyn Fn(Job<T>) -> JobResult<()> + Send + Sync>;

pub struct Worker {
    queue: Arc<JobQueue>,
    handlers: Arc<RwLock<HashMap<String, Box<dyn std::any::Any + Send + Sync>>>>,
}

impl Worker {
    pub fn new(queue: Arc<JobQueue>) -> Self {
        Self {
            queue,
            handlers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn register_handler<T>(&self, handler: Handler<T>)
    where
        T: for<'de> serde::Deserialize<'de> + Send + 'static,
    {
        let type_name = std::any::type_name::<T>().to_string();
        let mut handlers = self.handlers.write().await;
        handlers.insert(
            type_name,
            Box::new(handler),
        );
    }

    pub async fn start(&self) -> JobResult<()> {
        loop {
            if let Some(job) = self.queue.dequeue::<serde_json::Value>().await? {
                println!("Processing job: {}", job.id);
            }
            
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
    }
}

pub struct WorkerV2 {
    queues: Vec<Arc<JobQueue>>,
    running: Arc<std::sync::atomic::AtomicBool>,
}

impl WorkerV2 {
    pub fn new(queues: Vec<Arc<JobQueue>>) -> Self {
        Self {
            queues,
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    pub async fn start(&self) -> JobResult<()> {
        self.running.store(true, std::sync::atomic::Ordering::SeqCst);
        
        while self.running.load(std::sync::atomic::Ordering::SeqCst) {
            for queue in &self.queues {
                if let Some(job) = queue.dequeue::<serde_json::Value>().await? {
                    println!("Worker processing job: {}", job.id);
                }
            }
            
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }
        
        Ok(())
    }

    pub fn stop(&self) {
        self.running.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}
