# ADR 0005：发现目录在线更新与能力建议标签

- 状态：Accepted（2026-11-09 用户确认，可进入实现）
- 日期：2026-11-09
- 关联需求卡片：`docs/requirements/0005-catalog-online-tags.md`
- 关联调研：`docs/research/model-management.md`、`docs/research/download.md`
- 决策摘要：在现有静态种子清单之上，引入**版本化远程目录 + 本地缓存 + 24h TTL 自动刷新**（失败回退缓存/内置），并为目录与已安装模型提供**带来源与置信度的能力建议标签**；不做 Hub 关键词搜索、不触发自动下载、不宣称多模态可运行。

## 1. 背景与问题

发现页当前的数据源是编译进二进制的 `models/v0.toml`（`crates/llmgui-server/src/admin.rs:650`），GUI 另有一份手写镜像（`gui/app.js:15`）。这带来两个直接问题：

- **列表不会更新**：新发布的模型、文件体积或许可变化，必须重新发版才能进入发现页；用户也没有「刷新目录」的动作。
- **只有粗粒度 role**：`role` 只能表达 default/balanced/lite/coder/reasoning/moe/large/smoke/train-base，无法回答「哪些模型适合编程」「哪些是多模态」「哪些是嵌入模型」，也不能按能力筛选。

核心问题不是下载器能力，而是**目录的时效性与能力语义**：谁提供目录、多久刷新一次、失败怎么办、标签从哪来、标签是否可信。现有下载、manifest、registry、GUI 视图可以直接复用，不需要重写。

## 2. 现状核对

| # | 能力 | 现状 / 候选位置 | 判定（确认 / 排除 / 新增） | 备注 |
|---|---|---|---|---|
| 1 | 发现目录接口 | `crates/llmgui-server/src/admin.rs:656` | 确认 | `GET /admin/catalog` 只解析编译期清单 |
| 2 | 目录搜索 | `crates/llmgui-server/src/admin.rs:691` | 确认 | 只对内置条目的 id/repo/quant/role 做子串过滤 |
| 3 | 内置目录 | `models/v0.toml` | 确认 | 无 `updated_at`、无 tags |
| 4 | GUI 目录 | `gui/app.js:15`、`gui/app.js:1037` | 确认 | 第二份镜像；只渲染 role chip |
| 5 | 源能力 | `crates/llmgui-download/src/source.rs:99` | 排除 | 只有 `list/fetch`；Hub 关键词搜索不在本 ADR 范围 |
| 6 | 配置 | `crates/llmgui-core/src/config.rs:10` | 新增 | 增加 `[catalog]` 配置节 |
| 7 | 数据目录 | `crates/llmgui-core/src/paths.rs:29` | 新增 | 增加 `<home>/catalog/` 缓存目录 |
| 8 | 用户偏好 | `crates/llmgui-models/src/library.rs:114` | 确认 | `prefs.tags` 已存在但未参与展示/筛选 |
| 9 | 已安装模型输出 | `crates/llmgui-server/src/admin.rs:132` | 新增 | `entry_value` 只回传原始 `prefs.tags` |
| 10 | 多模态运行时 | 无 | 排除 | 无 mmproj 加载、无图片/音频输入；本 ADR 只做标签标注 |
| 11 | 离线开关 | 无 | 新增 | 本 ADR 先提供 `catalog.auto_update = false`；全局 `--offline` 仍按 ADR 0002 后续落地 |
| 12 | 目录变更通知 | `crates/llmgui-server/src/admin.rs:866` | 新增 | 现有 SSE 只覆盖下载任务，无全局 `/admin/events`；P1 先靠轮询，不阻塞本 ADR |

## 3. 目标与非目标

### 3.1 目标

- G1：发现页目录可在不发版的情况下更新；24h 内至少检查一次，手动刷新可用，失败不影响页面。
- G2：目录来源可解释：API 与 GUI 明确显示「远程 / 缓存 / 内置」、更新时间、是否过期、是否正在刷新。
- G3：目录与已安装模型都带能力标签；标签有稳定 ID、来源与置信度，可按标签筛选。
- G4：推断标签是**建议**：用户可确认或隐藏；任何标签都不改变模型的可见性与可加载性。
- G5：多模态口径诚实：能识别 mmproj/视觉/音频特征，但当前运行时未支持时明确标注「运行暂不支持」。
- G6：刷新链路安全、可回退：https、超时、大小/条目上限、schema 校验、原子缓存、单飞刷新；坏数据不覆盖好缓存。
- G7：不改变 manifest 与既有 `/admin/*`、`/v1/*` 字段语义；旧目录与旧 registry 可直接使用。

### 3.2 非目标

- N1：不做 Hub 关键词在线搜索、不做全网模型爬取；搜索仍只过滤当前目录。
- N2：不做模型自动下载或自动升级；刷新只获取目录元数据。
- N3：不做能力评测/试跑（不通过推理验证「编程」「多模态」能力）。
- N4：不实现 mmproj 加载、图片/音频输入与多模态推理；标签只作信息标注。
- N5：不引入遥测、账号或云端索引；不上传本机模型列表。
- N6：不推断许可、质量、安全或 benchmark 分数；许可仍以目录/仓库展示为准。
- N7：不新增 Hub 登录、私有仓库目录同步；私有模型仍走仓库 ID/URL 解析。

## 4. 口径映射（C1..Cn）

| 约束 | 设计落点 |
|---|---|
| C1 | §7.1 目录层次、§7.2 刷新策略、§8.1 配置、§8.4 `/admin/catalog` |
| C2 | §7.3 目录 schema 2、§9 兼容与迁移、§11 测试 |
| C3 | §7.4 标签体系、§7.5 推断规则、§8.3 标签契约、§8.4 API |
| C4 | §7.6 合并优先级与用户覆盖、§5 不变量 I3/I4 |
| C5 | §7.4 `runtime_supported`、§7.8 GUI 文案、§10 合规 |
| C6 | §7.6 已安装模型、§8.5 registry 增量、§8.4 API |
| C7 | §10 安全、§7.2 失败/退避、§11 负例测试 |
| C8 | §12 分期、§11 门禁、§18/§19 落地与交接 |

## 5. 不变量

- I1：目录刷新失败永远不阻塞库、发现、下载与聊天；选择顺序固定为「新鲜缓存 → 过期缓存 → 内置清单」。
- I2：远程目录只提供元数据；不得触发下载、加载、执行或 shell 调用。
- I3：标签是建议，不是事实；用户标签优先于目录与推断，且任何标签都不能让模型从列表消失。
- I4：每个标签必须有来源；同一输入下推断结果确定，便于测试与排障。
- I5：`catalog.auto_update = false`（以及未来全局离线模式）下，自动刷新不发起任何外网请求；手动刷新返回明确的 skipped 原因。
- I6：远程响应必须通过 schema、字节数、条目数、repo/file 格式校验；失败保留旧缓存，不写入半成品。
- I7：`multimodal/vision/audio` 标签不得暗示运行支持；`runtime_supported=false` 必须随标签返回并在 UI 展示。
- I8：既有 API 字段只增不改；`role` 保留，`tags` 为新增字段；manifest 不被标签写入。
- I9：同一时刻只有一个刷新任务；缓存写入走临时文件 + rename，读到旧缓存也必须是完整文件。
- I10：目录与缓存不包含本机路径、用户标识或模型文件内容。

## 6. 总体架构

```text
┌──────────────────────────── 本机 127.0.0.1:11435 ────────────────────────────┐
│  GUI (gui/)                     CLI (llmgui models catalog/tags)             │
│   发现/模型库/筛选                    │                                      │
│        └──────────── HTTP ──────────┴──────────┐                            │
│                                                ▼                            │
│                                  llmgui-server（axum）                       │
│                    /admin/catalog  /admin/catalog/refresh                   │
│                    /admin/models   /admin/models/{id}/tags                  │
│                                                │                            │
│                         ┌──────────────────────┴───────────────┐             │
│                         ▼                                      ▼             │
│                  CatalogStore                          TagEngine             │
│                  - remote/cache/bundled                 - taxonomy           │
│                  - TTL/single-flight                    - curated/heuristic  │
│                  - atomic cache                         - merge/user override │
│                         │                                      │             │
│                         ▼                                      ▼             │
│             <home>/catalog/cache.toml                 registry.prefs        │
│             <home>/catalog/state.json                 (tags/hidden_tags)     │
│                         │                                                    │
│                         ▼                                                    │
│              https 远程目录（schema 2）                                       │
│              models/v0.toml（编译期内置回退）                                 │
└──────────────────────────────────────────────────────────────────────────────┘
```

数据流：

1. **启动**：加载内置目录 → 读本地缓存（若存在且 schema 合法）→ 后台检查 TTL；过期则单飞刷新远程目录。
2. **发现**：`/admin/catalog` 立即返回当前最佳目录（远程/缓存/内置）与刷新元状态，不等待网络。
3. **刷新**：拉取 → 校验 → 写临时文件 → rename → 更新 `state.json` → 广播 `catalog_changed`；失败只更新 `last_checked_at` 与退避时间。
4. **标签**：目录条目带 curated tags；已安装/解析仓库由 `TagEngine` 按「用户 > 目录 > metadata > 文件 > 名称」合并，输出带来源与置信度的建议标签。
5. **操作**：用户确认/隐藏标签写入 `registry.prefs`，`generation +1`；GUI 通过事件或轮询刷新。

## 7. 关键设计

### 7.1 目录层次与来源

三个层次，优先级从高到低：

1. **远程目录**：`catalog.url` 指向的版本化 TOML（默认 GitHub raw 的 `models/v0.toml`），成功校验后同时作为缓存来源。
2. **本地缓存**：`<LLMGUI_HOME>/catalog/cache.toml` + `<LLMGUI_HOME>/catalog/state.json`，保存最后一次校验通过的远程内容与刷新状态；可随时删除，删除后回到内置。
3. **内置目录**：`models/v0.toml` 经 `include_str!` 编译进二进制，永远只读，作为最终回退。

选择规则：缓存未过期 → 直接用缓存；缓存过期 → 先返回缓存并在后台刷新；无缓存或缓存损坏 → 返回内置并后台刷新。API 返回 `catalog.source ∈ {remote, cache, bundled}`、`updated_at`、`fetched_at`、`stale`、`refreshing`、`error`，让 GUI 能解释当前看到的是哪一份。

`state.json` 记录：`url`、`etag`、`last_modified`、`checked_at`、`fetched_at`、`schema`、`sha256`、`entries`、`last_error`（脱敏，不含凭据）、`next_retry_at`。缓存与状态均原子写；刷新用 `<home>/catalog/catalog.lock` 文件锁做跨进程互斥，内存 `Mutex` 做单飞。

### 7.2 刷新策略

- 触发：服务启动后异步检查一次；之后每 6h 检查一次，仅在 `now >= checked_at + ttl_hours` 时真正发请求；手动刷新可 `force=true` 跳过 TTL。
- TTL：默认 24h；`catalog.ttl_hours` 可配，范围 1–168。
- 请求：https、10s 超时、最多 3 次重定向、响应 ≤2MB、条目 ≤1000；发送 `If-None-Match`/`If-Modified-Since`，304 只更新 `checked_at`。
- 失败：保留旧缓存；错误写入 `last_error`；指数退避（30min → 2h → 6h），不阻塞任何请求；`/admin/catalog` 仍返回旧数据。
- 并发：同一时刻只有一个刷新；重复请求合并到同一个 future 或直接返回 `refreshing=true`。
- 关闭：`catalog.auto_update = false` 时不做自动检查；手动刷新仍可用，除非全局离线模式生效。当前未实现全局 `--offline`，本 ADR 不虚构该能力；未来落地时按 I5 联动。
- 事件/轮询：P1 由 GUI 在发现页可见时轮询 `/admin/catalog`（沿用现有 2s 轮询兜底）；若后续按 ADR 0002 §7.10 落地全局 `/admin/events`，再追加 `catalog_changed`，不作为本 ADR 的交付前置。

### 7.3 目录 schema 2

```toml
schema = 2
updated_at = "2026-11-09T00:00:00Z"

[[models]]
id = "qwen3-coder-30b-a3b-q4km"
repo = "unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF"
file = "Qwen3-Coder-30B-A3B-Instruct-Q4_K_M.gguf"
quant = "Q4_K_M"
license = "Apache-2.0"
size_bytes = 18556689568
role = "coder"
tags = ["coding", "chat", "moe"]
```

约束：

- `schema` 必须存在且 ≤2；`schema = 1` 视为旧格式，`tags`/`updated_at` 缺省为空。
- `id/repo/file/quant/license/size_bytes/role` 沿用 v0 字段；`file` 为空表示整仓库（训练底座）。
- `tags` 只能使用 §7.4 的稳定 ID；未知 ID 忽略并记录一次告警（前向兼容），不因单个未知标签拒绝整份目录。
- 条目 `id` 唯一；`repo` 必须匹配 `owner/name`；`file` 必须是相对路径且不含 `..`/绝对路径/盘符/UNC。
- `updated_at` 是目录内容的发布时间，不是拉取时间；GUI 分别展示两者。
- 目录不包含下载 URL、哈希或镜像地址；下载仍走既有 `resolve` + manifest。

### 7.4 标签体系

稳定 ID 与展示名（标签表随 API 返回，GUI 不硬编码文案）：

| ID | 展示名 | 定义 | 主要证据 |
|---|---|---|---|
| `chat` | 通用对话 | 指令/对话模型 | curated；名称含 instruct/chat |
| `coding` | 编程 | 代码补全、重构、代码问答 | curated；名称 token 含 coder/code/coding |
| `reasoning` | 推理 | 思维链/推理强化模型 | curated；名称含 r1/qwq/reason/thinking |
| `math` | 数学 | 数学专用或显著强化 | curated；名称含 math |
| `multimodal` | 多模态 | 支持一种以上输入模态 | curated；存在 mmproj；名称含 vl/vision/omni/audio |
| `vision` | 视觉 | 图片/视频理解 | curated；mmproj；GGUF 视觉元数据；名称含 vl/llava/internvl/minicpm-v |
| `audio` | 音频 | 语音/音频理解或生成 | curated；GGUF 音频元数据；名称含 audio/whisper/omni |
| `embedding` | 向量 | 嵌入模型，非对话 | curated；名称含 embed/bge/gte/e5 |
| `rerank` | 重排 | 检索重排模型 | curated；名称含 rerank |
| `long-context` | 长上下文 | 训练上下文 ≥64k | GGUF `n_ctx_train ≥ 65536`；curated |
| `moe` | MoE | 混合专家架构 | GGUF expert 元数据/架构；名称含 moe/a3b |
| `tool-use` | 工具调用 | 明确支持函数/工具调用 | 仅 curated，避免名称误判 |
| `train-base` | 训练底座 | safetensors 训练底座 | `role = "train-base"` |

标签对象契约：

```json
{
  "id": "multimodal",
  "label": "多模态",
  "source": "file",
  "confidence": 0.9,
  "suggested": true,
  "runtime_supported": false
}
```

- `source ∈ {user, curated, metadata, file, name}`；`suggested = source ∈ {metadata, file, name}`。
- `confidence`：user/curated 1.0，metadata 0.95，file 0.9，name 0.7；同一标签取最高优先级来源。
- `runtime_supported`：当前仅 `multimodal/vision/audio` 为 `false`（无 mmproj 加载与媒体输入），其余为 `true`；由服务端计算，不写进目录。
- `embedding`/`rerank` 只是标签，不改变下载与加载入口；是否在聊天中提示「非对话模型」由 GUI 文案负责，不在本 ADR 改运行时。

### 7.5 推断规则

`TagEngine::suggest(repo, files, metadata, role) -> Vec<Tag>` 是纯函数、确定性、可单测。规则按证据强度执行：

| 证据 | 规则 | 输出 | confidence |
|---|---|---|---|
| 目录 | 条目 `tags` 原样转为 curated | 对应标签 | 1.0 |
| 文件 | 文件列表存在 `mmproj*.gguf`（大小写不敏感，含子目录） | `multimodal`, `vision` | 0.9 |
| GGUF metadata | `clip.has_vision_encoder = true` 或视觉投影层存在 | `multimodal`, `vision` | 0.95 |
| GGUF metadata | `clip.has_audio_encoder = true` | `multimodal`, `audio` | 0.95 |
| GGUF metadata | `n_ctx_train ≥ 65536` | `long-context` | 0.95 |
| GGUF metadata | expert 数 >0 或架构名含 moe | `moe` | 0.95 |
| 名称 | repo/file/id 按非字母数字切 token 后命中 `coder/code/coding` | `coding` | 0.7 |
| 名称 | 命中 `vl/vision/llava/internvl/minicpm-v` | `multimodal`, `vision` | 0.7 |
| 名称 | 命中 `audio/whisper/omni` | `multimodal`, `audio` | 0.7 |
| 名称 | 命中 `embed/bge/gte/e5` | `embedding` | 0.7 |
| 名称 | 命中 `rerank` | `rerank` | 0.7 |
| 名称 | 命中 `math` | `math` | 0.7 |
| 名称 | 命中 `r1/qwq/reason/thinking` | `reasoning` | 0.7 |
| 名称 | 命中 `moe/a3b` | `moe` | 0.7 |
| role | `role = "train-base"` | `train-base` | 1.0 |

边界：

- 名称匹配必须按 token，避免 `encoder` 命中 `code`、`embedding` 误判为 `coding` 之类子串问题。
- 不推断 `tool-use`、`chat`（除非名称明确 instruct/chat）、许可、质量、安全与 benchmark。
- 同一标签多来源时保留最高优先级；用户隐藏优先于一切建议。
- 已安装模型若没有仓库名，则只用 manifest 文件表与 GGUF metadata；推断不到就返回空，不猜。

### 7.6 已安装模型、用户覆盖与合并优先级

- `/admin/models` 与 `/admin/models/{id}` 的 `tags` 改为合并结果：目录 curated（按 `entry.repo` 大小写不敏感匹配）+ 本地文件/metadata 推断 + `prefs.tags`（user）+ `prefs.hidden_tags` 过滤。
- `prefs.tags`：用户确认的标签，`source=user`、`confidence=1.0`，可覆盖同 ID 的目录/推断标签。
- `prefs.hidden_tags`：新增 additive 字段；隐藏的标签不再出现在合并结果中，也不参与筛选。用户确认某标签时自动从 hidden 移除。
- 目录标签不写回 manifest，也不持久化到 registry 的模型索引；目录更新后标签可变化，GUI 在详情里提示「标签来自目录，可能随更新变化」。
- 删除/卸载模型时 `prefs` 按既有规则清理，不影响本设计。
- 合并优先级固定为 `user > curated > metadata > file > name`；同优先级按 confidence 降序，再按标签 ID 字典序，保证输出稳定。

### 7.7 刷新与标签的执行位置

- `CatalogStore` 放在 `llmgui-core`（配置、路径、schema、缓存读写）或 `llmgui-server` 内的独立模块；网络请求复用 `llmgui-download` 的 `reqwest` 依赖与 TLS 配置，但不调用 `Source::list/fetch`（避免把目录刷新绑到模型源适配器）。
- `TagEngine` 放在 `llmgui-models`，输入为纯数据（repo、文件列表、GGUF metadata、role、用户标签），不依赖 HTTP。
- GUI 不直接读缓存文件，只走 API；保持 ADR 0002 I8「服务端是唯一权威」。
- CLI 增加 `llmgui models catalog [--refresh] [--json]` 与 `llmgui models tags <id> [--confirm ...] [--hide ...]`，与 HTTP 行为对等（ADR 0002 C9）。

### 7.8 GUI 信息架构

- **发现页**：
  - 顶部状态行：「目录：远程 / 缓存 / 内置 · 内容更新于 … · 拉取于 … · 刷新」；过期显示「已过期，后台刷新中」；失败显示「使用缓存/内置」。
  - 卡片 tag chips：curated 标签正常显示；推断标签加「建议」样式；`runtime_supported=false` 追加「运行暂不支持」。
  - 筛选：标签多选（AND，与现有 fit/quant/license 筛选叠加）、结果计数、清除筛选；筛选只影响展示，不隐藏数据源。
- **模型库**：安装模型卡片与详情展示合并标签；详情提供「确认标签」「不感兴趣」；隐藏后立即从列表与筛选中消失。
- **可访问性**：标签 chip 使用 `button` + `aria-pressed`；状态行 `aria-live="polite"`；沿用 `textContent`，禁止 `innerHTML`。
- **回退**：API 不可用时仍使用 `gui/app.js` 的静态镜像，但展示「内置镜像，无法刷新」；镜像在 P0 同步补 tags。

### 7.9 可观测性

- 指标：`catalog_refresh_total{result=success|not_modified|failed|skipped}`、`catalog_refresh_seconds`、`catalog_age_seconds`、`catalog_bytes`、`catalog_entries`、`catalog_parse_errors`、`tag_suggestions_total{source}`。
- 日志：只记录 host、状态码、字节数、耗时、条目数与脱敏错误；不打印响应体、完整 URL 凭据或用户路径。
- 事件/轮询：P1 由 GUI 轮询 `/admin/catalog` 感知刷新结果；全局 `/admin/events` 尚未实现，若后续落地再补 `catalog_changed`。
- 无遥测：刷新只请求配置的目录 URL，不发送本机模型列表、设备信息或用户标识。

## 8. 数据与接口契约

### 8.1 配置（`<LLMGUI_HOME>/config.toml`）

```toml
[catalog]
auto_update = true
url = "https://raw.githubusercontent.com/gookeryoung/llmgui/main/models/v0.toml"
ttl_hours = 24
timeout_secs = 10
max_bytes = 2097152
```

- 新增 `CatalogConfig`，全部字段 `#[serde(default)]`，旧配置无需迁移。
- `auto_update=false` 关闭自动检查；`url` 为空视为禁用远程目录，只使用缓存/内置。
- 测试通过 fixture URL 覆盖；不引入环境变量专用开关，避免配置分散。

### 8.2 缓存与状态文件

```json
{
  "schema": 1,
  "url": "https://…/models/v0.toml",
  "etag": "\"…\"",
  "last_modified": "…",
  "checked_at": "2026-11-09T08:00:00Z",
  "fetched_at": "2026-11-09T08:00:00Z",
  "catalog_schema": 2,
  "sha256": "…",
  "entries": 12,
  "last_error": null,
  "next_retry_at": null
}
```

`cache.toml` 与远程目录同 schema；两者都原子写。`state.json` 只由服务端写，CLI 只读。

### 8.3 标签契约

```json
{
  "id": "coding",
  "label": "编程",
  "source": "curated",
  "confidence": 1.0,
  "suggested": false,
  "runtime_supported": true
}
```

`tag_taxonomy` 由服务端返回，GUI 用它渲染筛选与文案；新增标签只加 ID，不改已有 ID 含义。

### 8.4 HTTP API

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/admin/catalog` | 返回当前最佳目录；新增 `catalog` 元状态、每条目 `tags`、`tag_taxonomy`；既有字段不变 |
| POST | `/admin/catalog/refresh` | body `{force?: bool}`；返回刷新后的 `catalog` 元状态；`skipped_reason ∈ {disabled, offline, fresh, refreshing}` |
| GET | `/admin/catalog/search?q=&tags=&limit=` | 只过滤当前目录，不联网；`tags` 为逗号分隔的 AND 条件 |
| GET | `/admin/models?tags=` | 按合并标签过滤已安装模型（AND），与既有 query/fit/kind/sort 叠加 |
| GET | `/admin/models/{id}` | `tags` 为合并结果；保留 `prefs.tags` 原始字段以免破坏兼容 |
| PUT | `/admin/models/{id}/tags` | body `{confirmed: [...], hidden: [...]}`；校验标签 ID；原子更新 prefs 并 `generation +1` |

`GET /admin/catalog` 响应示例（只列新增部分）：

```json
{
  "ok": true,
  "data": [
    {
      "id": "qwen3-coder-30b-a3b-q4km",
      "tags": [
        {"id": "coding", "label": "编程", "source": "curated", "confidence": 1.0, "suggested": false, "runtime_supported": true},
        {"id": "moe", "label": "MoE", "source": "name", "confidence": 0.7, "suggested": true, "runtime_supported": true}
      ]
    }
  ],
  "catalog": {
    "schema": 2,
    "source": "cache",
    "updated_at": "2026-11-09T00:00:00Z",
    "fetched_at": "2026-11-09T08:00:00Z",
    "stale": false,
    "refreshing": false,
    "auto_update": true,
    "error": null
  },
  "tag_taxonomy": [{"id": "coding", "label": "编程", "description": "代码补全、重构与代码问答"}]
}
```

错误口径：手动刷新遇到坏数据返回 200 + `catalog.error`（不丢旧数据）；参数非法（未知标签、非法 repo）返回既有 400/422 风格错误；不新增会破坏既有客户端的状态码。

### 8.5 registry 增量

```json
{
  "prefs": {
    "qwen3-coder-30b-a3b-q4km": {
      "tags": ["coding", "moe"],
      "hidden_tags": ["reasoning"]
    }
  }
}
```

- `hidden_tags` 新增，`#[serde(default)]`；旧 registry 读取后为空。
- 不新增模型索引字段；标签合并结果按需计算，不落盘。
- 写入沿用单写者、文件锁、原子 rename 与 `generation`（ADR 0002 I2）。

## 9. 兼容与迁移

- **目录 schema**：schema 1 可读（无 tags/updated_at）；schema >2 拒绝并保留旧缓存；未知字段忽略。
- **manifest**：零改动；标签不进入 manifest，I1 不受影响。
- **registry**：只新增 `prefs.hidden_tags`，旧文件可直接读；删除该字段不影响功能。
- **GUI**：新增字段只增；静态镜像在 P0 补 tags，API 不可用时仍可用；上线后若远程目录失败，行为与当前静态目录一致。
- **CLI/API**：`/admin/catalog` 与 `/admin/models` 既有字段保持不变；新增字段有默认值；`role` 继续返回。
- **回退**：关闭 `auto_update`、删除 `<home>/catalog/` 即回到内置目录；无需迁移模型目录或数据库。
- **默认联网**：首次启动即可能访问默认目录 URL；文档与 GUI 状态行需明确披露，用户可通过配置关闭。默认 URL 指向本仓库 GitHub raw，国内网络不可达时自动回退缓存/内置，风险见 §14。

## 10. 安全与合规

| 威胁 | 控制 |
|---|---|
| 目录被篡改/投毒 | 仅 https；schema/字节/条目上限；repo/file 格式校验；缓存原子写；可选 `sha256` 固定校验；不信任目录中的下载地址 |
| SSRF/内网探测 | 只允许 https 公网 URL；测试 fixture 需显式 localhost 例外；不跟随无限重定向（≤3） |
| 恶意大文件/解析炸弹 | 响应 ≤2MB、条目 ≤1000、字符串长度上限；超限拒绝并保留旧缓存 |
| 刷新阻塞服务 | 后台任务、10s 超时、单飞、退避；HTTP 请求永不等待刷新完成 |
| 缓存半写/并发损坏 | 临时文件 + rename；文件锁；读到坏缓存回退内置并告警 |
| 标签误导（多模态可运行） | `runtime_supported=false` + 「运行暂不支持」文案；不新增 mmproj 加载 |
| 隐私泄漏 | 刷新只发目录 GET，不带本机模型列表/设备信息；`auto_update=false` 可关闭；文档披露默认联网 |
| 用户偏好被覆盖 | 用户标签优先级最高；hidden 抑制建议；写入走 registry 锁与原子写 |
| 未知/恶意标签 ID | 只接受标签表内 ID；未知 ID 忽略并记录，不进入筛选与持久化 |

## 11. 测试与质量门禁

- **单元（Rust）**：catalog schema 1/2 解析、未知标签、非法 repo/file、字节/条目上限；TTL/stale/退避；ETag/304；缓存原子写与损坏回退；标签表快照；推断规则逐条；合并优先级与隐藏逻辑；用户标签校验。
- **集成（Rust + axum + fixture HTTP）**：启动刷新成功/失败/超时/304；`/admin/catalog` 四种来源状态；手动刷新 `force/skipped`；`/admin/catalog/search?tags=`；`/admin/models?tags=`；`PUT /admin/models/{id}/tags` 与 `generation` 变化；坏响应不覆盖旧缓存。
- **前端（jsdom）**：标签 chip、来源/建议/「运行暂不支持」样式；标签筛选与结果计数；状态行四种状态；空态/错误态；`aria-pressed`/`aria-live`；无 `innerHTML`。
- **负例/安全**：http URL、超大响应、坏 TOML、schema 3、路径穿越 file、并发刷新、缓存半写、未知标签注入。
- **不做**：不访问真实公网作为合入门禁；不把 Hub 搜索纳入测试（本 ADR 不做）。
- **门禁**：代码阶段 `make flow`；文档阶段 `make check-docs`；GUI 每次 `node --check`；覆盖率沿用仓库既有口径，不为凑数写无断言用例。

## 12. 分期计划

| 期 | 工作 | 交付 / 退出标准 |
|---|---|---|
| P0 | 目录 schema 2（tags/updated_at）；标签表与合并优先级；`models/v0.toml` 与 GUI 镜像补 curated tags；`/admin/catalog` 与 `/admin/models` 返回 tags；GUI 标签 chip、筛选与「内置目录」状态行 | schema 1/2 可解析；标签可筛选；无网络请求；现有发现/下载流程不回归 |
| P1 | `[catalog]` 配置；`CatalogStore` 远程拉取/缓存/状态/TTL/ETag/退避/单飞；`POST /admin/catalog/refresh`；来源与更新时间展示；http/大小/schema 校验 | fixture 下成功/304/超时/坏数据四类路径全绿；失败回退缓存/内置；`auto_update=false` 无请求；缓存可删可重建 |
| P2a | `TagEngine` 推断（mmproj、GGUF metadata、名称 token）；已安装/解析仓库合并标签；`/admin/models?tags=` | 含 mmproj fixture 返回 multimodal/vision 且 `runtime_supported=false`；名称误判用例覆盖；推断结果确定 |
| P2b | `prefs.hidden_tags`；`PUT /admin/models/{id}/tags`；GUI 确认/隐藏；CLI `models tags`；事件刷新 | 用户确认/隐藏后 API、GUI、筛选一致；registry 原子写与并发测试通过 |
| P3 | 标签表扩展（长上下文/工具调用等 curated 规则）；筛选状态持久化；刷新与标签性能回归；文档与截图 | 500 条目下列表与筛选性能达标；文档覆盖远程目录/标签/关闭联网；无遗留「待定」 |

## 13. 性能预算

基线：500 条目录、本机 NVMe、无公网依赖的 fixture。以下为估算，P1/P3 实测回填。

| 指标 | 目标 | 口径 |
|---|---|---|
| 内置目录解析 | ≤10ms | 启动一次，编译期字符串 |
| 缓存目录解析 | ≤20ms | 2MB TOML 上限内 |
| `GET /admin/catalog` | p95 ≤50ms | 500 条、含标签合并 |
| `GET /admin/models?tags=` | p95 ≤50ms | 500 条、AND 筛选 |
| 标签推断 | ≤50ms / 500 条 | 纯内存、无网络 |
| 远程刷新 | ≤10s、≤2MB | 非阻塞；超时即失败 |
| GUI 首屏 | ≤200ms | 收到 JSON 到可交互 |
| 缓存写入 | ≤50ms | 原子 rename + fsync（按平台） |

## 14. 风险与缓解

| # | 风险 | 影响 | 缓解 |
|---|---|---|---|
| 1 | 默认 GitHub raw 在部分网络不可达 | 目录长期使用缓存/内置 | URL 可配置；失败退避不阻塞；文档说明可换镜像/自建 URL |
| 2 | 远程目录维护不及时 | 新模型仍不出现 | `updated_at` + 过期提示 + 手动刷新；发布流程把更新目录纳入清单 |
| 3 | 名称启发式误判 | 标签不可信 | 只在名称 token 精确命中时给 0.7；标「建议」；用户可隐藏；不用于隐藏模型 |
| 4 | 多模态标签被误读为可运行 | 用户上传图片失败 | `runtime_supported=false` + 「运行暂不支持」；本 ADR 不实现媒体输入 |
| 5 | 缓存/目录被替换 | 错误元数据或诱导下载 | https、schema/大小校验、可选 sha256、原子写；下载仍走 manifest SHA256 |
| 6 | 标签表频繁变更 | UI/持久化漂移 | ID 稳定；只增不改；未知 ID 忽略；快照测试 |
| 7 | GUI 静态镜像与目录漂移 | 回退视图信息旧 | P0 同步补 tags；状态行标注「内置镜像」；后续可评估移除以减少重复 |
| 8 | 多实例并发刷新 | 重复请求/缓存竞争 | 文件锁 + 单飞 + 原子写；重复实例只读缓存 |
| 9 | 默认联网引发隐私顾虑 | 用户不信任 | 只发目录 GET、无标识、可关闭、文档披露；不引入遥测 |
| 10 | 目录体积增长 | 解析/首屏变慢 | 2MB/1000 条上限；性能预算与 P3 回归 |

## 15. 决策记录

| 编号 | 议题 | 结论 | 依据 | 被否决项 | 落点 |
|---|---|---|---|---|---|
| D1 | 总体选型 | 远程策展目录 + 本地缓存 + 建议标签；不做 Hub 关键词搜索 | 用户确认范围；第三方搜索无 SLA，目录可控可测 | 只做静态清单；只做 Hub 实时搜索；两者都做 | §7.1、N1 |
| D2 | 目录载体 | 复用 `models/v0.toml` 升级到 schema 2，远程与内置同 schema | 避免第三份格式；现有 `include_str!` 与 GUI 镜像改动最小 | 新增 `catalog/v1.toml`；JSON 目录 | §7.3、§9 |
| D3 | 刷新策略 | 24h TTL + 启动检查 + 6h 轮询 + 手动刷新 + 单飞/退避 | 时效性与资源开销平衡；失败不影响页面 | 每次打开页面实时拉取；WebSocket 推送目录 | §7.2 |
| D4 | 标签模型 | 稳定 ID + 来源 + 置信度 + `runtime_supported` | 可解释、可筛选、可测试；不把推测当事实 | 纯字符串数组；只存 role；用标签隐藏模型 | §7.4、§8.3 |
| D5 | 标签来源优先级 | `user > curated > metadata > file > name` | 用户意图最高；目录人工标注次之；推断最弱 | 名称优先；metadata 覆盖用户 | §7.5、§7.6 |
| D6 | 多模态口径 | 可识别但标注「运行暂不支持」，本 ADR 不实现 mmproj | 当前运行时无媒体输入，避免过度承诺 | 直接显示「多模态」；本 ADR 同时实现 mmproj | §7.4、N4 |
| D7 | 存储位置 | 目录 tags 在目录文件；用户 tags/hidden 在 registry prefs；推断不落盘 | 保持 manifest 不可变；registry 可重建 | 写 manifest；把推断结果持久化 | §7.6、§8.5 |
| D8 | API 形态 | 扩展 `/admin/catalog` 与 `/admin/models`，新增 refresh/tags 端点 | 与既有资源化 API 一致；字段只增 | 新开 `/admin/catalog/v2`；GUI 直读缓存文件 | §8.4 |
| D9 | 联网默认值 | 默认开启自动更新，可配置关闭，仅拉目录元数据 | 满足「自动更新」且成本低；不涉及模型与遥测 | 默认关闭；默认开启 Hub 搜索 | §7.2、§10 |
| D10 | 安全基线 | https、超时、2MB/1000 条、schema/路径校验、原子缓存、可选 sha256 | 目录是新的外部输入面，按不可信数据处理 | 信任远程 TOML；只靠 TLS | §10 |
| D11 | GUI 技术路线 | 延续零构建原生 JS，标签与状态由服务端驱动 | ADR 0001/0002 定调；避免构建链 | 引入前端框架；GUI 自行推断标签 | §7.8 |
| D12 | CLI 对等 | `models catalog` 与 `models tags` 提供与 HTTP 等价入口 | ADR 0002 C9 的动作矩阵要求 | 只做 GUI；只做 API | §7.7 |

## 16. 后果

**正面**：目录不必随版本发布即可更新；用户能按编程/多模态等能力筛选；标签来源可解释，避免「猜出来的事实」；远程失败有缓存与内置兜底，发现页不会因网络不可用而空白；不触碰 manifest、下载与运行时，风险面小。

**负面**：新增一条外部网络依赖与缓存生命周期；标签质量依赖目录维护与启发式规则；GUI 需要处理更多状态（远程/缓存/内置/刷新中/过期），测试面增加；静态镜像仍是第二份数据，需要同步。

**中性**：多模态标签在运行支持前只是信息标注；Hub 关键词搜索仍留待后续 ADR；默认联网只影响目录元数据，不改变「无遥测」口径。

## 17. 与既有 ADR 的关系

- **ADR 0002** 仍是模型库/发现/下载/生命周期的权威。本 ADR 只细化 §7.2 的「种子目录 + 仓库解析」：新增远程目录刷新与标签，不改变下载任务、registry 写一致性、运行时与 `/v1/*` 契约。
- **ADR 0001** 的推理与训练目标不受影响；本 ADR 不新增推理能力，也不改变量化/显存口径。
- **ADR 0004** 的 `role = "coder"` 条目继续有效；本 ADR 为其补 `coding` 等标签，不替换 role。
- Hub 关键词搜索仍按 ADR 0002 D6 作为可选能力，本 ADR 明确不做；若后续要做，另立 ADR。

## 18. 落地状态

| 期 | 状态 | 代码落点 | 测试 | 提交 |
|---|---|---|---|---|
| P0 | 完成 | `models/v0.toml`（schema 2 + tags）、`crates/llmgui-models/src/tags.rs`、`admin.rs` catalog/models 返回 tags、`gui/app.js` 标签 chip/筛选/状态行 | tags 单测、`catalog_and_storage` | 本次变更 |
| P1 | 完成 | `catalog.rs`（`CatalogStore`/TTL/ETag/退避/单飞/文件锁/原子写）、`config.rs` `[catalog]`、`paths.rs` catalog 目录、`POST /admin/catalog/refresh` | `catalog.rs` fixture 测试（成功/304/失败/禁用/http 拒绝/损坏缓存回退） | 本次变更 |
| P2a | 完成 | `tags::suggest`（mmproj/metadata/名称 token）、`merged_model_tags`、`GET /admin/models?tags=` | 推断规则/误判/合并优先级单测、`model_tags_confirm_hide_and_filter` | 本次变更 |
| P2b | 完成 | `ModelPrefs.hidden_tags`、`library.set_tags`、`PUT /admin/models/{id}/tags`、GUI 确认/隐藏、CLI `models tags` | registry 兼容与 API 测试 | 本次变更 |
| P3 | 部分 | 标签表扩展、筛选 localStorage 持久化、GUI/CLI/文档同步 | `make check-fast`、`make test`、Python 测试、`node --check` | 本次变更 |

## 19. 交接

- 当前阶段：P0–P2b 完成，P3 除性能回归外完成。
- 已完成：远程目录 + 缓存/TTL/ETag/退避/单飞/原子写；标签表与推断；API 与 GUI/CLI 标签筛选、确认/隐藏；文档与 CHANGELOG。
- 下一步：按 §13 用 500 条 fixture 实测 `GET /admin/catalog`、`GET /admin/models?tags=` 与标签推断，回填性能表；评估是否把 §7.9 的 `catalog_*` 指标接入 `/metrics`。
- 已知坑：
  1. 默认 GitHub raw 在部分网络不可达，必须验证「缓存/内置回退」而不是只在开发机测试；
  2. 多模态标签只做信息标注，不能宣称可上传图片；
  3. GUI 静态镜像与 `models/v0.toml` 是两份数据，改目录时需同步补 tags；
  4. 目录 schema 未来升级需保持「未知字段忽略、未知标签忽略、schema >2 拒绝」的前向兼容策略；
  5. `prefs.hidden_tags` 是 registry 新增字段，旧文件依赖 serde 默认值读取；
  6. 自动刷新只在 `llmgui serve` 中启动；`CatalogStore::refresh` 失败一律保留旧文档，新增逻辑不要先写缓存再校验。

## 20. 参考

- 需求卡片：`docs/requirements/0005-catalog-online-tags.md`
- 既有方案：`docs/adr/0002-model-management.md` §7.1/§7.2/§7.9/§7.10、`docs/adr/0001-local-llm-runtime-finetune.md`
- 调研：`docs/research/model-management.md`、`docs/research/download.md`
- ModelScope 文件列表 API（现状下载源）：<https://modelscope.cn/api/v1/models/Qwen/Qwen2.5-7B-Instruct-GGUF/repo/files?Revision=master&Recursive=true>
- GitHub raw（默认目录候选）：<https://raw.githubusercontent.com/gookeryoung/llmgui/main/models/v0.toml>
