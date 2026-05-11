use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
pub enum StopReason {
    Finish,
    Hook,
    Error(String),
    Other(String),
}

pub fn step_count_is(max_steps: usize) -> StopWhenFn {
    let counter = std::sync::atomic::AtomicUsize::new(0);
    Arc::new(move |step_count: usize| {
        counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        step_count >= max_steps
    })
}

pub type StopWhenFn = Arc<dyn Fn(usize) -> bool + Send + Sync>;
