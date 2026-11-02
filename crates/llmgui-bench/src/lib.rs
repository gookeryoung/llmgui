//! Reproducible local benchmark harness.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use llmgui_backend_api::{GenerateRequest, TokenEvent};
use llmgui_scheduler::Scheduler;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchReport {
    pub backend: String,
    pub model: String,
    pub concurrency: usize,
    pub iterations: usize,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub wall_ms: u64,
    pub aggregate_tokens_per_second: f64,
    pub per_request: Vec<RequestResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestResult {
    pub request_id: String,
    pub ttft_ms: u64,
    pub total_ms: u64,
    pub completion_tokens: u32,
    pub finish_reason: String,
}

pub async fn run(
    scheduler: Arc<Scheduler>,
    model: impl Into<String>,
    prompt: impl Into<String>,
    max_tokens: u32,
    concurrency: usize,
    iterations: usize,
) -> BenchReport {
    let model = model.into();
    let prompt = prompt.into();
    let started = Instant::now();
    let mut handles = Vec::new();
    for index in 0..iterations {
        let scheduler = scheduler.clone();
        let prompt = prompt.clone();
        handles.push(tokio::spawn(async move {
            let request = GenerateRequest {
                request_id: format!("bench-{index}"),
                prompt,
                max_tokens,
                temperature: 0.0,
                top_p: 1.0,
                stop: vec![],
                seed: Some(index as u64),
                cache_key: format!("bench-{index}"),
            };
            let mut rx = scheduler.enqueue(request, Arc::new(AtomicBool::new(false)));
            let mut text = String::new();
            let mut result = RequestResult {
                request_id: format!("bench-{index}"),
                ttft_ms: 0,
                total_ms: 0,
                completion_tokens: 0,
                finish_reason: "error".to_string(),
            };
            while let Some(event) = rx.recv().await {
                match event {
                    TokenEvent::Token { text: token, .. } => text.push_str(&token),
                    TokenEvent::Done { stats } => {
                        result.ttft_ms = stats.ttft_ms;
                        result.total_ms = stats.total_ms;
                        result.completion_tokens = stats.completion_tokens;
                        result.finish_reason = stats.finish_reason;
                    }
                    TokenEvent::Error { .. } => {}
                }
            }
            result
        }));
    }
    let mut per_request = Vec::new();
    let mut completion_tokens = 0u32;
    for handle in handles {
        if let Ok(result) = handle.await {
            completion_tokens += result.completion_tokens;
            per_request.push(result);
        }
    }
    let wall = started.elapsed();
    let wall_ms = wall.as_millis() as u64;
    BenchReport {
        backend: "scheduler".to_string(),
        model,
        concurrency,
        iterations,
        prompt_tokens: 0,
        completion_tokens,
        wall_ms,
        aggregate_tokens_per_second: if wall_ms == 0 {
            0.0
        } else {
            f64::from(completion_tokens) * 1000.0 / wall_ms as f64
        },
        per_request,
    }
}
