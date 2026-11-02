use serde::{Deserialize, Serialize};

/// KV cache element type. Byte accounting includes GGUF block overhead:
/// q8_0 = 34/32 bytes/element, q4_0 = 18/32 bytes/element.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KvCacheType {
    F16,
    Q8_0,
    Q4_0,
}

impl KvCacheType {
    pub fn bytes_per_element(self) -> f64 {
        match self {
            Self::F16 => 2.0,
            Self::Q8_0 => 34.0 / 32.0,
            Self::Q4_0 => 18.0 / 32.0,
        }
    }

    pub fn parse(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "q4_0" | "q4" => Self::Q4_0,
            "q8_0" | "q8" => Self::Q8_0,
            _ => Self::F16,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryParams {
    pub n_layer: u32,
    pub n_head: u32,
    pub n_head_kv: u32,
    pub n_embd: u32,
    pub n_ctx: u32,
    pub kv_type: KvCacheType,
    /// Total size of weight files in bytes.
    pub weight_bytes: u64,
    /// Number of concurrent sequences whose KV is resident.
    pub slots: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEstimate {
    pub weights_bytes: u64,
    pub kv_bytes: u64,
    pub compute_bytes: u64,
    pub runtime_bytes: u64,
    pub total_bytes: u64,
    pub head_dim: u32,
    pub kv_bytes_per_token: u64,
    pub fits_24gb: bool,
    pub fits_32gb: bool,
}

pub fn estimate_memory(p: &MemoryParams) -> MemoryEstimate {
    let head_dim = if p.n_head == 0 {
        0
    } else {
        p.n_embd.checked_div(p.n_head).unwrap_or(0)
    };
    let elements_per_token =
        2.0 * f64::from(p.n_layer) * f64::from(p.n_head_kv) * f64::from(head_dim);
    let kv_bytes_per_token = (elements_per_token * p.kv_type.bytes_per_element()).ceil() as u64;
    let kv_bytes = kv_bytes_per_token
        .saturating_mul(u64::from(p.n_ctx))
        .saturating_mul(u64::from(p.slots.max(1)));
    // Empirically: compute graph buffers scale with ubatch but are bounded;
    // 1 GiB is a conservative default for the supported 0.5B-32B range.
    let compute_bytes = 1_073_741_824;
    let runtime_bytes = 644_245_504;
    let total_bytes = p
        .weight_bytes
        .saturating_add(kv_bytes)
        .saturating_add(compute_bytes)
        .saturating_add(runtime_bytes);
    MemoryEstimate {
        weights_bytes: p.weight_bytes,
        kv_bytes,
        compute_bytes,
        runtime_bytes,
        total_bytes,
        head_dim,
        kv_bytes_per_token,
        fits_24gb: total_bytes <= 24 * 1_000_000_000,
        fits_32gb: total_bytes <= 32 * 1_000_000_000,
    }
}

pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.2}{}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kv_bytes_match_formula() {
        let p = MemoryParams {
            n_layer: 36,
            n_head: 32,
            n_head_kv: 8,
            n_embd: 4096,
            n_ctx: 32768,
            kv_type: KvCacheType::Q8_0,
            weight_bytes: 5_000_000_000,
            slots: 1,
        };
        let e = estimate_memory(&p);
        assert_eq!(e.head_dim, 128);
        // 2 * 36 * 8 * 128 * 32768 * 1.0625 = 2.566 GB
        assert!((e.kv_bytes as f64 - 2_566_324_224.0).abs() < 1_000_000.0);
        assert!(e.total_bytes > 8_000_000_000);
    }

    #[test]
    fn q4_is_smaller_than_f16() {
        let mut p = MemoryParams {
            n_layer: 36,
            n_head: 32,
            n_head_kv: 8,
            n_embd: 4096,
            n_ctx: 8192,
            kv_type: KvCacheType::F16,
            weight_bytes: 1,
            slots: 1,
        };
        let f16 = estimate_memory(&p).kv_bytes;
        p.kv_type = KvCacheType::Q4_0;
        let q4 = estimate_memory(&p).kv_bytes;
        assert!(q4 < f16 / 2);
    }
}
