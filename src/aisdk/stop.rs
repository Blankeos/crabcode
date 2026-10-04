use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
pub enum StopReason {
    Finish,
    MaxTokens,
    Refusal,
    Hook,
    Error(String),
    Other(String),
}

pub fn step_count_is(max_steps: usize) -> StopWhenFn {
    let counter = std::sync::atomic::AtomicUsize::new(0);
    Arc::new(move |step_count: usize| {
        counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        step_count > max_steps
    })
}

pub type StopWhenFn = Arc<dyn Fn(usize) -> bool + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_count_is_allows_exact_configured_steps() {
        let stop = step_count_is(2);

        assert!(!stop(1));
        assert!(!stop(2));
        assert!(stop(3));
    }
}
