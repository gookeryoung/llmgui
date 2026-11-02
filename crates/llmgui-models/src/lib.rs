//! GGUF metadata reader and the local model library (registry). It only reads the header and
//! metadata/tensor table; tensor data stays untouched, so opening a 70B file is
//! cheap. Malformed files fail with bounds-checked errors.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use llmgui_core::estimate::{
    KvCacheType, MemoryEstimate, MemoryParams, estimate_memory, human_bytes,
};
use llmgui_core::{LlmguiError, Result};
use serde::{Deserialize, Serialize};

pub mod library;

pub const GGUF_MAGIC: &[u8; 4] = b"GGUF";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GgufInfo {
    pub version: u32,
    pub tensor_count: u64,
    pub metadata_count: u64,
    pub file_size: u64,
    pub architecture: Option<String>,
    pub name: Option<String>,
    pub n_layer: u32,
    pub n_head: u32,
    pub n_head_kv: u32,
    pub n_embd: u32,
    pub n_ctx_train: u32,
    pub chat_template: Option<String>,
    pub metadata: BTreeMap<String, String>,
}

impl GgufInfo {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let file_size = file.metadata()?.len();
        let mut reader = BufReader::new(file);
        let mut magic = [0u8; 4];
        reader.read_exact(&mut magic)?;
        if &magic != GGUF_MAGIC {
            return Err(LlmguiError::Model(
                "not a GGUF file (bad magic)".to_string(),
            ));
        }
        let version = read_u32(&mut reader)?;
        if !(2..=3).contains(&version) {
            return Err(LlmguiError::Model(format!(
                "unsupported GGUF version {version}"
            )));
        }
        let tensor_count = read_u64(&mut reader)?;
        let metadata_count = read_u64(&mut reader)?;
        let mut metadata = BTreeMap::new();
        for _ in 0..metadata_count {
            let key = read_string(&mut reader)?;
            let value_type = read_u32(&mut reader)?;
            let value = read_meta_value(&mut reader, value_type)?;
            metadata.insert(key, value);
        }
        // Skip tensor infos, but validate the advertised count is readable.
        for _ in 0..tensor_count {
            let _name = read_string(&mut reader)?;
            let n_dims = read_u32(&mut reader)?;
            if n_dims > 8 {
                return Err(LlmguiError::Model(format!(
                    "invalid tensor dimension count {n_dims}"
                )));
            }
            for _ in 0..n_dims {
                let _dim = read_u64(&mut reader)?;
            }
            let _tensor_type = read_u32(&mut reader)?;
            let offset = read_u64(&mut reader)?;
            if offset > file_size {
                return Err(LlmguiError::Model(
                    "tensor offset beyond end of file".to_string(),
                ));
            }
        }
        let architecture = metadata.get("general.architecture").cloned();
        let arch = architecture.clone().unwrap_or_default();
        let n_layer = lookup_u32(&metadata, &format!("{arch}.block_count")).unwrap_or(0);
        let n_head = lookup_u32(&metadata, &format!("{arch}.attention.head_count")).unwrap_or(0);
        let n_head_kv =
            lookup_u32(&metadata, &format!("{arch}.attention.head_count_kv")).unwrap_or(n_head);
        let n_embd = lookup_u32(&metadata, &format!("{arch}.embedding_length")).unwrap_or(0);
        let n_ctx_train = lookup_u32(&metadata, &format!("{arch}.context_length")).unwrap_or(0);
        let chat_template = metadata.get("tokenizer.chat_template").cloned();
        Ok(Self {
            version,
            tensor_count,
            metadata_count,
            file_size,
            architecture,
            name: metadata.get("general.name").cloned(),
            n_layer,
            n_head,
            n_head_kv,
            n_embd,
            n_ctx_train,
            chat_template,
            metadata,
        })
    }

    pub fn estimate(&self, n_ctx: u32, kv_type: KvCacheType, slots: u32) -> MemoryEstimate {
        estimate_memory(&MemoryParams {
            n_layer: self.n_layer,
            n_head: self.n_head,
            n_head_kv: self.n_head_kv,
            n_embd: self.n_embd,
            n_ctx,
            kv_type,
            weight_bytes: self.file_size,
            slots,
        })
    }

    pub fn summary(&self) -> String {
        format!(
            "{} ({}) v{} · {} layers · {} heads ({} kv) · {} · {} · ctx {}",
            self.name.as_deref().unwrap_or("unnamed"),
            self.architecture.as_deref().unwrap_or("unknown"),
            self.version,
            self.n_layer,
            self.n_head,
            self.n_head_kv,
            self.n_embd,
            human_bytes(self.file_size),
            self.n_ctx_train
        )
    }
}

fn lookup_u32(map: &BTreeMap<String, String>, key: &str) -> Option<u32> {
    map.get(key)?.parse().ok()
}

fn read_u32(reader: &mut impl Read) -> Result<u32> {
    let mut buf = [0u8; 4];
    reader.read_exact(&mut buf)?;
    Ok(u32::from_le_bytes(buf))
}

fn read_u64(reader: &mut impl Read) -> Result<u64> {
    let mut buf = [0u8; 8];
    reader.read_exact(&mut buf)?;
    Ok(u64::from_le_bytes(buf))
}

fn read_string(reader: &mut impl Read) -> Result<String> {
    let len = read_u64(reader)?;
    if len > 64 * 1024 * 1024 {
        return Err(LlmguiError::Model(format!(
            "GGUF string too large: {len} bytes"
        )));
    }
    let mut buf = vec![0u8; len as usize];
    reader.read_exact(&mut buf)?;
    String::from_utf8(buf)
        .map_err(|_| LlmguiError::Model("GGUF string is not valid UTF-8".to_string()))
}

fn read_meta_value(reader: &mut impl Read, value_type: u32) -> Result<String> {
    match value_type {
        0 => Ok(read_u8(reader)?.to_string()),
        1 => Ok((read_u8(reader)? as i8).to_string()),
        2 => Ok(read_u16(reader)?.to_string()),
        3 => Ok((read_u16(reader)? as i16).to_string()),
        4 => Ok(read_u32(reader)?.to_string()),
        5 => Ok((read_u32(reader)? as i32).to_string()),
        6 => Ok(f32::from_bits(read_u32(reader)?).to_string()),
        7 => Ok((read_u8(reader)? != 0).to_string()),
        8 => read_string(reader),
        9 => read_array_summary(reader),
        10 => Ok(read_u64(reader)?.to_string()),
        11 => Ok((read_u64(reader)? as i64).to_string()),
        12 => Ok(f64::from_bits(read_u64(reader)?).to_string()),
        other => Err(LlmguiError::Model(format!(
            "unknown GGUF metadata type {other}"
        ))),
    }
}

fn read_array_summary(reader: &mut impl Read) -> Result<String> {
    let element_type = read_u32(reader)?;
    let len = read_u64(reader)?;
    // Token arrays can be huge; skip them without allocating.
    if len > 64 {
        skip_array(reader, element_type, len)?;
        return Ok(format!("<array len={len}>"));
    }
    let mut values = Vec::new();
    for _ in 0..len {
        values.push(read_meta_value(reader, element_type)?);
    }
    Ok(format!("[{}]", values.join(",")))
}

fn skip_array(reader: &mut impl Read, element_type: u32, len: u64) -> Result<()> {
    for _ in 0..len {
        skip_value(reader, element_type)?;
    }
    Ok(())
}

fn skip_value(reader: &mut impl Read, value_type: u32) -> Result<()> {
    match value_type {
        0 | 1 | 7 => skip_bytes(reader, 1),
        2 | 3 => skip_bytes(reader, 2),
        4..=6 => skip_bytes(reader, 4),
        10..=12 => skip_bytes(reader, 8),
        8 => {
            let len = read_u64(reader)?;
            skip_bytes(reader, len)
        }
        9 => {
            let nested_type = read_u32(reader)?;
            let len = read_u64(reader)?;
            skip_array(reader, nested_type, len)
        }
        other => Err(LlmguiError::Model(format!(
            "unknown GGUF metadata type {other}"
        ))),
    }
}

fn skip_bytes(reader: &mut impl Read, count: u64) -> Result<()> {
    let mut remaining = count;
    let mut buf = [0u8; 8192];
    while remaining > 0 {
        let take = remaining.min(buf.len() as u64) as usize;
        reader.read_exact(&mut buf[..take])?;
        remaining -= take as u64;
    }
    Ok(())
}

fn read_u8(reader: &mut impl Read) -> Result<u8> {
    let mut buf = [0u8; 1];
    reader.read_exact(&mut buf)?;
    Ok(buf[0])
}

fn read_u16(reader: &mut impl Read) -> Result<u16> {
    let mut buf = [0u8; 2];
    reader.read_exact(&mut buf)?;
    Ok(u16::from_le_bytes(buf))
}

/// Write a tiny valid GGUF header for tests. Only metadata is populated.
#[doc(hidden)]
pub fn write_test_gguf(
    path: &Path,
    arch: &str,
    layers: u32,
    heads: u32,
    kv_heads: u32,
    embd: u32,
) -> Result<()> {
    use std::io::Write;
    let mut file = File::create(path)?;
    file.write_all(b"GGUF")?;
    file.write_all(&3u32.to_le_bytes())?;
    file.write_all(&0u64.to_le_bytes())?;
    let meta = [
        ("general.architecture".to_string(), arch.to_string()),
        (format!("{arch}.block_count"), layers.to_string()),
        (format!("{arch}.attention.head_count"), heads.to_string()),
        (
            format!("{arch}.attention.head_count_kv"),
            kv_heads.to_string(),
        ),
        (format!("{arch}.embedding_length"), embd.to_string()),
        (format!("{arch}.context_length"), "32768".to_string()),
    ];
    file.write_all(&(meta.len() as u64).to_le_bytes())?;
    for (key, value) in &meta {
        write_string(&mut file, key)?;
        file.write_all(&8u32.to_le_bytes())?;
        write_string(&mut file, value)?;
    }
    Ok(())
}

fn write_string(writer: &mut impl std::io::Write, value: &str) -> Result<()> {
    writer.write_all(&(value.len() as u64).to_le_bytes())?;
    writer.write_all(value.as_bytes())?;
    Ok(())
}

/// Read only the GGUF magic to quickly reject non-GGUF files.
pub fn is_gguf(path: &Path) -> Result<bool> {
    let mut file = File::open(path)?;
    let mut magic = [0u8; 4];
    let n = file.read(&mut magic)?;
    Ok(n == 4 && &magic == GGUF_MAGIC)
}

/// Seek helper kept public for future adapter/tensor inspection.
pub fn file_len(path: &Path) -> Result<u64> {
    let mut file = File::open(path)?;
    Ok(file.seek(SeekFrom::End(0))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_synthetic_gguf() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tiny.gguf");
        write_test_gguf(&path, "qwen2", 28, 28, 4, 3584).unwrap();
        let info = GgufInfo::open(&path).unwrap();
        assert_eq!(info.version, 3);
        assert_eq!(info.architecture.as_deref(), Some("qwen2"));
        assert_eq!(info.n_layer, 28);
        assert_eq!(info.n_head, 28);
        assert_eq!(info.n_head_kv, 4);
        assert_eq!(info.n_embd, 3584);
        assert!(info.summary().contains("qwen2"));
    }

    #[test]
    fn rejects_non_gguf() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.bin");
        std::fs::write(&path, b"NOPE").unwrap();
        assert!(GgufInfo::open(&path).is_err());
    }
}
