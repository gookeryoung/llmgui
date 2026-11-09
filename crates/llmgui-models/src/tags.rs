//! Capability tag taxonomy plus deterministic suggestion and merge rules.
//!
//! Tags are *advisory*: they never hide a model, never change the manifest and
//! never claim runtime support. The taxonomy IDs are stable; new capabilities
//! only add IDs and existing meanings never change.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct TagDef {
    pub id: &'static str,
    pub label: &'static str,
    pub description: &'static str,
}

/// Stable tag IDs shown by the API (`tag_taxonomy`) and persisted in prefs.
pub const TAXONOMY: &[TagDef] = &[
    TagDef {
        id: "chat",
        label: "通用对话",
        description: "指令/对话模型",
    },
    TagDef {
        id: "coding",
        label: "编程",
        description: "代码补全、重构与代码问答",
    },
    TagDef {
        id: "reasoning",
        label: "推理",
        description: "思维链或推理强化模型",
    },
    TagDef {
        id: "math",
        label: "数学",
        description: "数学专用或显著强化的模型",
    },
    TagDef {
        id: "multimodal",
        label: "多模态",
        description: "支持一种以上输入模态",
    },
    TagDef {
        id: "vision",
        label: "视觉",
        description: "图片/视频理解",
    },
    TagDef {
        id: "audio",
        label: "音频",
        description: "语音/音频理解或生成",
    },
    TagDef {
        id: "embedding",
        label: "向量",
        description: "嵌入模型，非对话用途",
    },
    TagDef {
        id: "rerank",
        label: "重排",
        description: "检索重排模型",
    },
    TagDef {
        id: "long-context",
        label: "长上下文",
        description: "训练上下文 ≥64k",
    },
    TagDef {
        id: "moe",
        label: "MoE",
        description: "混合专家架构",
    },
    TagDef {
        id: "tool-use",
        label: "工具调用",
        description: "明确支持函数/工具调用",
    },
    TagDef {
        id: "train-base",
        label: "训练底座",
        description: "safetensors 训练底座",
    },
];

/// Tags that the current runtime cannot execute yet (no mmproj or media input).
const RUNTIME_UNSUPPORTED: &[&str] = &["multimodal", "vision", "audio"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TagSource {
    User,
    Curated,
    Metadata,
    File,
    Name,
}

impl TagSource {
    /// Merge priority: user > curated > metadata > file > name.
    fn priority(self) -> u8 {
        match self {
            Self::User => 4,
            Self::Curated => 3,
            Self::Metadata => 2,
            Self::File => 1,
            Self::Name => 0,
        }
    }

    fn confidence(self) -> f32 {
        match self {
            Self::User | Self::Curated => 1.0,
            Self::Metadata => 0.95,
            Self::File => 0.9,
            Self::Name => 0.7,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tag {
    pub id: String,
    pub label: String,
    pub source: TagSource,
    pub confidence: f32,
    pub suggested: bool,
    pub runtime_supported: bool,
}

pub fn taxonomy() -> &'static [TagDef] {
    TAXONOMY
}

pub fn taxonomy_entry(id: &str) -> Option<&'static TagDef> {
    TAXONOMY.iter().find(|def| def.id == id)
}

pub fn is_known(id: &str) -> bool {
    taxonomy_entry(id).is_some()
}

/// Unknown IDs are ignored for forward compatibility instead of failing a whole
/// catalog or registry.
pub fn unknown_tags(ids: &[String]) -> Vec<String> {
    ids.iter()
        .filter(|id| !is_known(id))
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn make_tag(id: &str, source: TagSource) -> Option<Tag> {
    let def = taxonomy_entry(id)?;
    Some(Tag {
        id: def.id.to_string(),
        label: def.label.to_string(),
        source,
        confidence: source.confidence(),
        suggested: matches!(
            source,
            TagSource::Metadata | TagSource::File | TagSource::Name
        ),
        runtime_supported: !RUNTIME_UNSUPPORTED.contains(&def.id),
    })
}

fn order_of(id: &str) -> usize {
    TAXONOMY
        .iter()
        .position(|def| def.id == id)
        .unwrap_or(usize::MAX)
}

fn ordered(tags: BTreeMap<String, Tag>) -> Vec<Tag> {
    let mut values: Vec<Tag> = tags.into_values().collect();
    values.sort_by(|a, b| {
        order_of(&a.id)
            .cmp(&order_of(&b.id))
            .then_with(|| a.id.cmp(&b.id))
    });
    values
}

/// Keep the strongest evidence for each tag ID.
fn absorb(into: &mut BTreeMap<String, Tag>, tag: Tag) {
    match into.get(&tag.id) {
        Some(existing)
            if existing.source.priority() > tag.source.priority()
                || (existing.source.priority() == tag.source.priority()
                    && existing.confidence >= tag.confidence) => {}
        _ => {
            into.insert(tag.id.clone(), tag);
        }
    }
}

/// Suggest tags from repository name, id, files, GGUF metadata and role.
///
/// Pure and deterministic: identical inputs always produce identical output.
pub fn suggest(
    repo: Option<&str>,
    id: Option<&str>,
    files: &[String],
    architecture: Option<&str>,
    n_ctx_train: Option<u32>,
    role: Option<&str>,
    metadata: Option<&BTreeMap<String, String>>,
) -> Vec<Tag> {
    let mut candidates: Vec<Tag> = Vec::new();
    let mut push = |id: &str, source: TagSource| {
        if let Some(tag) = make_tag(id, source) {
            candidates.push(tag);
        }
    };

    let has_mmproj = files.iter().any(|path| {
        let name = path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(path)
            .to_ascii_lowercase();
        name.starts_with("mmproj") && name.ends_with(".gguf")
    });
    if has_mmproj {
        push("multimodal", TagSource::File);
        push("vision", TagSource::File);
    }

    if let Some(metadata) = metadata {
        let flag = |key: &str| {
            metadata
                .get(key)
                .is_some_and(|value| value.eq_ignore_ascii_case("true"))
        };
        let has_vision = flag("clip.has_vision_encoder")
            || metadata.keys().any(|key| key.starts_with("clip.vision"));
        if has_vision {
            push("multimodal", TagSource::Metadata);
            push("vision", TagSource::Metadata);
        }
        if flag("clip.has_audio_encoder") {
            push("multimodal", TagSource::Metadata);
            push("audio", TagSource::Metadata);
        }
    }

    if n_ctx_train.is_some_and(|ctx| ctx >= 65_536) {
        push("long-context", TagSource::Metadata);
    }

    if architecture.is_some_and(|arch| arch.to_ascii_lowercase().contains("moe")) {
        push("moe", TagSource::Metadata);
    }

    if role == Some("train-base") {
        push("train-base", TagSource::Curated);
    }

    let mut raw = String::new();
    for part in [repo, id].into_iter().flatten() {
        raw.push(' ');
        raw.push_str(part);
    }
    for file in files {
        raw.push(' ');
        raw.push_str(file.rsplit(['/', '\\']).next().unwrap_or(file));
    }
    let lower = raw.to_ascii_lowercase();
    let tokens: BTreeSet<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect();
    let hit = |words: &[&str]| words.iter().any(|word| tokens.contains(word));

    if hit(&["coder", "code", "coding"]) {
        push("coding", TagSource::Name);
    }
    if hit(&["vl", "vision", "llava", "internvl"]) || lower.contains("minicpm-v") {
        push("multimodal", TagSource::Name);
        push("vision", TagSource::Name);
    }
    if hit(&["audio", "whisper", "omni"]) {
        push("multimodal", TagSource::Name);
        push("audio", TagSource::Name);
    }
    if hit(&["embed", "embedding", "bge", "gte", "e5"]) {
        push("embedding", TagSource::Name);
    }
    if hit(&["rerank"]) {
        push("rerank", TagSource::Name);
    }
    if hit(&["math"]) {
        push("math", TagSource::Name);
    }
    if hit(&["r1", "qwq", "reason", "reasoning", "thinking"]) {
        push("reasoning", TagSource::Name);
    }
    if hit(&["moe", "a3b"]) {
        push("moe", TagSource::Name);
    }
    if hit(&["instruct", "chat"]) {
        push("chat", TagSource::Name);
    }

    let mut merged = BTreeMap::new();
    for candidate in candidates {
        absorb(&mut merged, candidate);
    }
    ordered(merged)
}

/// Merge curated directory tags, suggested tags and user overrides.
///
/// Priority is `user > curated > metadata > file > name`; `hidden` suppresses a
/// tag unless the user explicitly confirmed it.
pub fn merge(
    curated: &[String],
    suggested: &[Tag],
    user: &[String],
    hidden: &[String],
) -> Vec<Tag> {
    let mut merged = BTreeMap::new();
    for id in curated {
        if let Some(tag) = make_tag(id, TagSource::Curated) {
            absorb(&mut merged, tag);
        }
    }
    for tag in suggested {
        absorb(&mut merged, tag.clone());
    }
    for id in user {
        if let Some(tag) = make_tag(id, TagSource::User) {
            absorb(&mut merged, tag);
        }
    }
    let user_ids: BTreeSet<&str> = user.iter().map(String::as_str).collect();
    for id in hidden {
        if !user_ids.contains(id.as_str()) {
            merged.remove(id);
        }
    }
    ordered(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(tags: &[Tag]) -> Vec<&str> {
        tags.iter().map(|tag| tag.id.as_str()).collect()
    }

    #[test]
    fn taxonomy_ids_are_stable_and_unique() {
        let ids: BTreeSet<&str> = TAXONOMY.iter().map(|def| def.id).collect();
        assert_eq!(ids.len(), TAXONOMY.len());
        assert!(ids.contains("coding"));
        assert!(ids.contains("multimodal"));
        assert!(is_known("long-context"));
        assert!(!is_known("unknown"));
    }

    #[test]
    fn multimodal_tags_are_not_runtime_supported() {
        let files = vec!["mmproj-model-f16.gguf".to_string()];
        let tags = suggest(Some("org/vl-model"), None, &files, None, None, None, None);
        let ids = ids(&tags);
        assert!(ids.contains(&"multimodal"));
        assert!(ids.contains(&"vision"));
        assert!(
            tags.iter()
                .filter(|tag| tag.id == "multimodal")
                .all(|tag| !tag.runtime_supported)
        );
    }

    #[test]
    fn name_tokens_do_not_match_substrings() {
        let files = vec!["encoder-only.gguf".to_string()];
        let tags = suggest(
            Some("org/encoder"),
            Some("encoder"),
            &files,
            None,
            None,
            None,
            None,
        );
        assert!(!ids(&tags).contains(&"coding"));

        let tags = suggest(
            Some("org/qwen-coder"),
            Some("qwen-coder"),
            &[],
            None,
            None,
            None,
            None,
        );
        assert!(ids(&tags).contains(&"coding"));
    }

    #[test]
    fn metadata_rules_cover_context_moe_and_vision() {
        let mut metadata = BTreeMap::new();
        metadata.insert("clip.has_vision_encoder".to_string(), "true".to_string());
        let tags = suggest(
            Some("org/model"),
            Some("model"),
            &[],
            Some("qwen3_moe"),
            Some(131_072),
            None,
            Some(&metadata),
        );
        let ids = ids(&tags);
        assert!(ids.contains(&"vision"));
        assert!(ids.contains(&"long-context"));
        assert!(ids.contains(&"moe"));
    }

    #[test]
    fn merge_prefers_user_then_curated_then_suggestions() {
        let curated = vec![
            "coding".to_string(),
            "chat".to_string(),
            "bogus".to_string(),
        ];
        let suggested = vec![
            make_tag("coding", TagSource::Name).unwrap(),
            make_tag("reasoning", TagSource::Name).unwrap(),
        ];
        let user = vec!["reasoning".to_string()];
        let tags = merge(&curated, &suggested, &user, &[]);
        let coding = tags.iter().find(|tag| tag.id == "coding").unwrap();
        assert_eq!(coding.source, TagSource::Curated);
        let reasoning = tags.iter().find(|tag| tag.id == "reasoning").unwrap();
        assert_eq!(reasoning.source, TagSource::User);
        assert!(!reasoning.suggested);
        assert!(!ids(&tags).contains(&"bogus"));
    }

    #[test]
    fn hidden_suppresses_suggestions_but_not_user_tags() {
        let suggested = vec![
            make_tag("moe", TagSource::Name).unwrap(),
            make_tag("chat", TagSource::Name).unwrap(),
        ];
        let tags = merge(&[], &suggested, &[], &["moe".to_string()]);
        assert_eq!(ids(&tags), vec!["chat"]);

        let tags = merge(&[], &suggested, &["moe".to_string()], &["moe".to_string()]);
        assert!(ids(&tags).contains(&"moe"));
    }

    #[test]
    fn suggestion_order_follows_taxonomy() {
        let tags = suggest(
            Some("org/Qwen3-Coder-30B-A3B-Instruct-GGUF"),
            Some("qwen3-coder-30b-a3b"),
            &[],
            None,
            None,
            None,
            None,
        );
        assert_eq!(ids(&tags), vec!["chat", "coding", "moe"]);
    }

    #[test]
    fn train_base_role_is_curated() {
        let tags = suggest(None, None, &[], None, None, Some("train-base"), None);
        assert_eq!(ids(&tags), vec!["train-base"]);
        assert_eq!(tags[0].source, TagSource::Curated);
    }
}
