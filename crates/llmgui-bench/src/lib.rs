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
    pub warmup: usize,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub wall_ms: u64,
    pub aggregate_tokens_per_second: f64,
    /// Median of per-request decode tok/s (first token excluded).
    pub median_decode_tps: Option<f64>,
    pub target_min: Option<f64>,
    pub target_max: Option<f64>,
    pub target_pass: Option<bool>,
    pub decode_samples: Vec<f64>,
    pub per_request: Vec<RequestResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestResult {
    pub request_id: String,
    pub ttft_ms: u64,
    pub total_ms: u64,
    pub completion_tokens: u32,
    pub decode_tokens_per_second: f64,
    pub finish_reason: String,
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    scheduler: Arc<Scheduler>,
    model: impl Into<String>,
    prompt: impl Into<String>,
    max_tokens: u32,
    concurrency: usize,
    iterations: usize,
    warmup: usize,
    target: Option<(f64, f64)>,
) -> BenchReport {
    let model = model.into();
    let prompt = prompt.into();
    let concurrency = concurrency.max(1);

    for index in 0..warmup {
        run_request(scheduler.clone(), prompt.clone(), max_tokens, index, true).await;
    }

    let started = Instant::now();
    let mut per_request = Vec::new();
    let mut completion_tokens = 0u32;
    for batch_start in (0..iterations).step_by(concurrency) {
        let batch_end = (batch_start + concurrency).min(iterations);
        let mut handles = Vec::with_capacity(batch_end - batch_start);
        for index in batch_start..batch_end {
            let scheduler = scheduler.clone();
            let prompt = prompt.clone();
            handles.push(tokio::spawn(run_request(
                scheduler, prompt, max_tokens, index, false,
            )));
        }
        for handle in handles {
            if let Ok(result) = handle.await {
                completion_tokens += result.completion_tokens;
                per_request.push(result);
            }
        }
    }
    let wall = started.elapsed();
    let wall_ms = wall.as_millis() as u64;

    let mut decode_samples: Vec<f64> = per_request
        .iter()
        .filter(|result| result.decode_tokens_per_second > 0.0)
        .map(|result| result.decode_tokens_per_second)
        .collect();
    decode_samples.sort_by(f64::total_cmp);
    let median_decode_tps = median(&decode_samples);
    let target_pass = target_met(target, median_decode_tps);

    BenchReport {
        backend: "scheduler".to_string(),
        model,
        concurrency,
        iterations,
        warmup,
        prompt_tokens: 0,
        completion_tokens,
        wall_ms,
        aggregate_tokens_per_second: if wall_ms == 0 {
            0.0
        } else {
            f64::from(completion_tokens) * 1000.0 / wall_ms as f64
        },
        median_decode_tps,
        target_min: target.map(|(min, _)| min),
        target_max: target.map(|(_, max)| max),
        target_pass,
        decode_samples,
        per_request,
    }
}

async fn run_request(
    scheduler: Arc<Scheduler>,
    prompt: String,
    max_tokens: u32,
    index: usize,
    warmup: bool,
) -> RequestResult {
    let request_id = if warmup {
        format!("bench-warmup-{index}")
    } else {
        format!("bench-{index}")
    };
    let request = GenerateRequest {
        request_id: request_id.clone(),
        prompt,
        max_tokens,
        temperature: 0.0,
        top_p: 1.0,
        stop: vec![],
        seed: Some(index as u64),
        cache_key: request_id.clone(),
    };
    let mut rx = scheduler.enqueue(request, Arc::new(AtomicBool::new(false)));
    let mut result = RequestResult {
        request_id,
        ttft_ms: 0,
        total_ms: 0,
        completion_tokens: 0,
        decode_tokens_per_second: 0.0,
        finish_reason: "error".to_string(),
    };
    while let Some(event) = rx.recv().await {
        match event {
            TokenEvent::Token { .. } => {}
            TokenEvent::Done { stats } => {
                result.ttft_ms = stats.ttft_ms;
                result.total_ms = stats.total_ms;
                result.completion_tokens = stats.completion_tokens;
                result.decode_tokens_per_second = stats.decode_tokens_per_second;
                result.finish_reason = stats.finish_reason;
            }
            TokenEvent::Error { .. } => {}
        }
    }
    result
}

fn median(sorted: &[f64]) -> Option<f64> {
    match sorted.len() {
        0 => None,
        len if len % 2 == 1 => Some(sorted[len / 2]),
        len => Some((sorted[len / 2 - 1] + sorted[len / 2]) / 2.0),
    }
}

/// Throughput is a floor goal: reaching `target_min` passes. `target_max` is a
/// reference upper bound (headroom), so exceeding it is not a failure.
fn target_met(target: Option<(f64, f64)>, median: Option<f64>) -> Option<bool> {
    match (target, median) {
        (Some((min, _)), Some(value)) => Some(value >= min),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_handles_even_and_odd_samples() {
        assert_eq!(median(&[]), None);
        assert_eq!(median(&[42.0]), Some(42.0));
        assert_eq!(median(&[1.0, 2.0, 3.0]), Some(2.0));
        assert_eq!(median(&[1.0, 2.0, 3.0, 4.0]), Some(2.5));
    }

    #[test]
    fn target_met_only_checks_lower_bound() {
        assert_eq!(target_met(Some((90.0, 120.0)), Some(240.6)), Some(true));
        assert_eq!(target_met(Some((90.0, 120.0)), Some(90.0)), Some(true));
        assert_eq!(target_met(Some((90.0, 120.0)), Some(89.9)), Some(false));
        assert_eq!(target_met(None, Some(240.6)), None);
        assert_eq!(target_met(Some((90.0, 120.0)), None), None);
    }
}
