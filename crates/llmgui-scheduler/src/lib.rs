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
