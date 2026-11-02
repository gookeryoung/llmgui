//! Request scheduling, slot accounting and cancellation.

use std::sync::Arc;

use llmgui_backend_api::{Backend, GenerateRequest, TokenEvent};
use tokio::sync::mpsc::UnboundedReceiver;

pub struct Scheduler {
    backend: Arc<dyn Backend>,
    slots: Arc<tokio::sync::Semaphore>,
}

impl Scheduler {
    pub fn new(backend: Arc<dyn Backend>, slots: usize) -> Self {
        Self {
            backend,
            slots: Arc::new(tokio::sync::Semaphore::new(slots.max(1))),
        }
    }

    pub fn backend(&self) -> Arc<dyn Backend> {
        self.backend.clone()
    }

    /// Queue a generation. Returns a receiver immediately; the blocking backend
    /// runs on a dedicated thread.
    pub fn enqueue(
        &self,
        request: GenerateRequest,
        cancel: Arc<std::sync::atomic::AtomicBool>,
    ) -> UnboundedReceiver<TokenEvent> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let backend = self.backend.clone();
        let slots = self.slots.clone();
        tokio::spawn(async move {
            let _permit = match slots.acquire_owned().await {
                Ok(permit) => permit,
                Err(_) => {
                    let _ = tx.send(TokenEvent::Error {
                        message: "scheduler closed".to_string(),
                        code: "scheduler_closed".to_string(),
                    });
                    return;
                }
            };
            let tx_for_error = tx.clone();
            let result =
                tokio::task::spawn_blocking(move || backend.generate(request, cancel, tx.clone()))
                    .await;
            if let Err(error) = result {
                let _ = tx_for_error.send(TokenEvent::Error {
                    message: format!("backend task failed: {error}"),
                    code: "backend_join_error".to_string(),
                });
            }
        });
        rx
    }

    pub fn available_slots(&self) -> usize {
        self.slots.available_permits()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llmgui_backend_mock::MockBackend;
    use std::sync::atomic::AtomicBool;

    fn request(id: &str) -> GenerateRequest {
        GenerateRequest {
            request_id: id.to_string(),
            prompt: "hello".to_string(),
            max_tokens: 8,
            temperature: 0.0,
            top_p: 1.0,
            stop: vec![],
            seed: Some(1),
            cache_key: id.to_string(),
        }
    }

    #[tokio::test]
    async fn schedules_mock_backend_to_completion() {
        let backend = Arc::new(MockBackend::new(0));
        let scheduler = Scheduler::new(backend, 2);
        let mut rx = scheduler.enqueue(request("a"), Arc::new(AtomicBool::new(false)));
        let mut done = false;
        while let Some(event) = rx.recv().await {
            if let TokenEvent::Done { stats } = event {
                assert!(stats.completion_tokens > 0);
                done = true;
                break;
            }
        }
        assert!(done);
    }

    #[tokio::test]
    async fn runs_multiple_requests_concurrently() {
        let backend = Arc::new(MockBackend::new(1));
        let scheduler = Arc::new(Scheduler::new(backend, 4));
        let mut handles = Vec::new();
        for i in 0..4 {
            let scheduler = scheduler.clone();
            handles.push(tokio::spawn(async move {
                let mut rx =
                    scheduler.enqueue(request(&format!("r{i}")), Arc::new(AtomicBool::new(false)));
                let mut tokens = 0;
                while let Some(event) = rx.recv().await {
                    match event {
                        TokenEvent::Token { .. } => tokens += 1,
                        TokenEvent::Done { .. } => break,
                        TokenEvent::Error { .. } => break,
                    }
                }
                tokens
            }));
        }
        for handle in handles {
            assert!(handle.await.unwrap() > 0);
        }
    }

    #[tokio::test]
    async fn cancellation_finishes_with_cancelled_reason() {
        let backend = Arc::new(MockBackend::new(5));
        let scheduler = Scheduler::new(backend, 1);
        let cancel = Arc::new(AtomicBool::new(false));
        let mut rx = scheduler.enqueue(request("cancel"), cancel.clone());
        let first = rx.recv().await;
        assert!(matches!(first, Some(TokenEvent::Token { .. })));
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        let mut reason = None;
        while let Some(event) = rx.recv().await {
            if let TokenEvent::Done { stats } = event {
                reason = Some(stats.finish_reason);
                break;
            }
        }
        assert_eq!(reason.as_deref(), Some("cancelled"));
    }
}
