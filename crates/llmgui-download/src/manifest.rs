//! Model manifest construction: id/quant inference from the file table.

use llmgui_core::manifest::{
    ContextInfo, MANIFEST_SCHEMA, ModelFile, ModelManifest, ModelSource, QuantInfo,
};

use crate::source::RemoteFile;

/// Extensions that must never be downloaded: arbitrary code / pickle payloads
/// (requirement C10).
const FORBIDDEN_EXTENSIONS: &[&str] = &["bin", "pkl", "pickle", "pt", "pth", "ckpt"];

/// `true` when `path` is a file we refuse to fetch (pickle-style weights).
pub fn is_forbidden_file(path: &str) -> bool {
    path.rsplit_once('.')
        .map(|(_, ext)| {
            FORBIDDEN_EXTENSIONS
                .iter()
                .any(|bad| ext.eq_ignore_ascii_case(bad))
        })
        .unwrap_or(false)
}

/// Known GGUF quantisation names and their approximate bits-per-weight.
const QUANT_BITS: &[(&str, f32)] = &[
    ("IQ1_S", 1.56),
    ("IQ1_M", 1.75),
    ("IQ2_XXS", 2.06),
    ("IQ2_XS", 2.31),
    ("IQ2_S", 2.5),
    ("IQ3_XXS", 3.06),
    ("IQ3_XS", 3.3),
    ("IQ3_S", 3.44),
    ("IQ3_M", 3.66),
    ("IQ4_XS", 4.25),
    ("IQ4_NL", 4.5),
    ("Q2_K", 2.63),
    ("Q3_K_S", 3.44),
    ("Q3_K_M", 3.91),
    ("Q3_K_L", 4.27),
    ("Q4_0", 4.55),
    ("Q4_1", 5.0),
    ("Q4_K_S", 4.58),
    ("Q4_K_M", 4.89),
    ("Q5_0", 5.5),
    ("Q5_1", 6.0),
    ("Q5_K_S", 5.52),
    ("Q5_K_M", 5.69),
    ("Q6_K", 6.56),
    ("Q8_0", 8.5),
    ("F16", 16.0),
    ("BF16", 16.0),
    ("F32", 32.0),
];

/// Infer quantisation info from the selected file names. Files are scanned for
/// a known GGUF quant token (longest token wins to prefer `Q4_K_M` over `Q4`).
pub fn infer_quant(files: &[RemoteFile]) -> QuantInfo {
    let format = infer_format(files);
    if format != "gguf" {
        return QuantInfo {
            format,
            kind: "unknown".to_string(),
            bits_per_weight: None,
        };
    }
    let names: Vec<String> = files.iter().map(|file| file.path.to_uppercase()).collect();
    let mut best: Option<(&str, f32)> = None;
    for (kind, bits) in QUANT_BITS {
        if names.iter().any(|name| contains_token(name, kind))
            && best.is_none_or(|(best_kind, _)| kind.len() > best_kind.len())
        {
            best = Some((kind, *bits));
        }
    }
    match best {
        Some((kind, bits)) => QuantInfo {
            format,
            kind: kind.to_string(),
            bits_per_weight: Some(bits),
        },
        None => QuantInfo {
            format,
            kind: "unknown".to_string(),
            bits_per_weight: None,
        },
    }
}

fn contains_token(haystack: &str, token: &str) -> bool {
    let mut start = 0;
    while let Some(index) = haystack[start..].find(token) {
        let begin = start + index;
        let end = begin + token.len();
        let before_ok = begin == 0
            || !haystack.as_bytes()[begin - 1].is_ascii_alphanumeric()
                && haystack.as_bytes()[begin - 1] != b'_';
        let after_ok = end == haystack.len()
            || !haystack.as_bytes()[end].is_ascii_alphanumeric()
                && haystack.as_bytes()[end] != b'_';
        if before_ok && after_ok {
            return true;
        }
        start = begin + 1;
    }
    false
}

/// Infer the tensor format from file extensions (`gguf`, `safetensors`, ...).
pub fn infer_format(files: &[RemoteFile]) -> String {
    let mut formats: Vec<&str> = files
        .iter()
        .filter_map(|file| file.path.rsplit_once('.').map(|(_, ext)| ext))
        .map(|ext| {
            if ext.eq_ignore_ascii_case("gguf") {
                "gguf"
            } else if ext.eq_ignore_ascii_case("safetensors") {
                "safetensors"
            } else {
                "other"
            }
        })
        .collect();
    // Prefer weight formats over config files.
    if formats.contains(&"gguf") {
        formats.clear();
        return "gguf".to_string();
    }
    if formats.contains(&"safetensors") {
        return "safetensors".to_string();
    }
    "unknown".to_string()
}

/// Build the manifest written to `models/<id>/manifest.json`.
pub fn build_manifest(
    source_kind: &str,
    repo: &str,
    revision: &str,
    model_id: &str,
    files: Vec<ModelFile>,
    remote_files: &[RemoteFile],
) -> ModelManifest {
    ModelManifest {
        schema: MANIFEST_SCHEMA,
        id: model_id.to_string(),
        source: ModelSource {
            kind: source_kind.to_string(),
            repo: repo.to_string(),
            revision: revision.to_string(),
            url: None,
        },
        architecture: None,
        quant: infer_quant(remote_files),
        context: ContextInfo::default(),
        files,
        license: None,
        chat_template: None,
        created_at: llmgui_core::now_rfc3339(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, size: u64) -> RemoteFile {
        RemoteFile {
            path: path.to_string(),
            size,
            sha256: Some("a".repeat(64)),
        }
    }

    #[test]
    fn infers_gguf_quant_from_file_name() {
        let quant = infer_quant(&[file("qwen2.5-7b-instruct-q4_k_m-00001-of-00002.gguf", 1)]);
        assert_eq!(quant.format, "gguf");
        assert_eq!(quant.kind, "Q4_K_M");
        assert_eq!(quant.bits_per_weight, Some(4.89));

        assert_eq!(infer_quant(&[file("x-iq4_xs.gguf", 1)]).kind, "IQ4_XS");
        assert_eq!(infer_quant(&[file("x-Q8_0.gguf", 1)]).kind, "Q8_0");
        assert_eq!(infer_quant(&[file("x-f16.gguf", 1)]).kind, "F16");
    }

    #[test]
    fn picks_longest_quant_token() {
        // "Q4_K_M" must win over the shorter "Q4" style tokens.
        let quant = infer_quant(&[file("model-Q4_K_M.gguf", 1)]);
        assert_eq!(quant.kind, "Q4_K_M");
    }

    #[test]
    fn safetensors_has_no_quant() {
        let quant = infer_quant(&[
            file("model-00001-of-00002.safetensors", 10),
            file("config.json", 1),
        ]);
        assert_eq!(quant.format, "safetensors");
        assert_eq!(quant.kind, "unknown");
        assert!(quant.bits_per_weight.is_none());
    }

    #[test]
    fn rejects_pickle_style_files() {
        assert!(is_forbidden_file("pytorch_model.bin"));
        assert!(is_forbidden_file("training_args.BIN"));
        assert!(is_forbidden_file("model.pkl"));
        assert!(!is_forbidden_file("model.gguf"));
        assert!(!is_forbidden_file("config.json"));
        assert!(!is_forbidden_file("README.md"));
    }

    #[test]
    fn build_manifest_defaults() {
        let remote = vec![file("a-q4_k_m.gguf", 7)];
        let manifest = build_manifest(
            "modelscope",
            "Qwen/X",
            "master",
            "Qwen--X",
            vec![ModelFile {
                path: "a-q4_k_m.gguf".to_string(),
                size: 7,
                sha256: "a".repeat(64),
            }],
            &remote,
        );
        assert_eq!(manifest.schema, MANIFEST_SCHEMA);
        assert_eq!(manifest.id, "Qwen--X");
        assert_eq!(manifest.source.kind, "modelscope");
        assert_eq!(manifest.quant.kind, "Q4_K_M");
        assert!(manifest.architecture.is_none());
        assert_eq!(manifest.total_size(), 7);
        manifest.validate().unwrap();
    }
}
