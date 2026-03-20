#[macro_export]
macro_rules! job_handler {
    ($handler_name:ident, $job_type:ty, $handler_expr:expr) => {
        pub fn $handler_name(job: $crate::Job<$job_type>) -> $crate::JobResult<()> {
            $handler_expr(job);
            Ok(())
        }
    };
}

#[macro_export]
macro_rules! enqueue_job {
    ($queue:expr, $payload:expr, $queue_name:expr) => {{
        let job = $crate::Job::new($payload, $queue_name);
        $queue.enqueue(job).await
    }};
}
