# ADR 0002：模型管理（模型库、发现下载、运行时生命周期、训练产物）

- 状态：Proposed（待评审；评审通过后改 Accepted 才进入实现）
- 日期：2026-11-03
- 关联需求卡片：`docs/requirements/0003-model-management.md`
- 关联调研：`docs/research/model-management.md`、`docs/research/download.md`、`docs/research/runtime.md`
- 决策摘要：在现有「下载器 + manifest + GGUF metadata + 单模型运行时」之上，增加**注册表驱动的模型库**、**任务化下载与发现**、**单活跃模型的生命周期管理（load/unload/switch/JIT/TTL）**、**训练产物派生注册**；服务端是唯一权威，CLI 与零构建 GUI 只是视图。

## 1. 背景与问题

现状是一条一次性链路：`llmgui pull <repo>` 前台下载 → `llmgui serve --model <gguf>` 固定加载一个模型 → `/v1/models` 固定返回该模型；GUI 的模型页是 `models/v0.toml` 的手写镜像，只能看不能操作。训练侧已有 adapter/merge/GGUF 导出能力，但产物不会进入任何库，用户仍需手工拼命令。

对照 LM Studio 与 Unsloth（见调研）后，用户对「模型管理」的预期是：

- 打开应用先看到**本机真实可用的模型**，而不是静态推荐清单；
- 发现模型时能按**量化 + 显存 + 上下文**判断「这台机器跑不跑得动」再下载；
- 下载是**可暂停/续传/取消的任务**，不是只能等的终端输出；
- 模型能**加载/卸载/切换**，有每模型默认参数、别名与预设；
- 训练/导出的产物**自动回到同一个库**，能直接加载对话；
- 磁盘可管理（占用、删除、GC），并且删除/加载不会把环境弄坏。

核心问题不是「再写一个模型下载器」，而是补齐**模型状态的所有权与生命周期**：谁记录安装了哪些模型、谁决定加载哪个、谁保证并发与磁盘安全。当前代码里 `registry.json` 路径已预留却未使用（`crates/llmgui-core/src/paths.rs:41`），`Backend::unload` 已存在却无人调用（`crates/llmgui-backend-api/src/lib.rs:110`），`pull` 与 GUI 之间没有可查询的任务状态（`crates/llmgui-download/src/lib.rs:63`）——这些正是本 ADR 要收口的部分。

## 2. 现状核对

| # | 能力 | 现状 / 候选位置 | 判定 | 备注 |
|---|---|---|---|---|
| 1 | 已安装模型枚举 | `crates/llmgui-cli/src/main.rs:273` | 确认 | 只遍历 `models/*/manifest.json` 打印文本，无结构化输出、无 API |
| 2 | 服务端模型列表 | `crates/llmgui-server/src/lib.rs:139` | 确认 | `list_models` 固定返回单个 `state.model_id`；无「已安装」概念 |
| 3 | 单模型启动 | `crates/llmgui-cli/src/main.rs:419` | 确认 | `serve` 启动时 `backend.load`，换模型必须重启 |
| 4 | 卸载原语 | `crates/llmgui-backend-api/src/lib.rs:110`；`llmgui-cli` 无调用 | 新增 | 原语存在，缺 manager、端点与 drain 协作 |
| 5 | 下载执行 | `crates/llmgui-download/src/lib.rs:63` | 确认 | 前台单次 `pull`，无队列/暂停/取消/跨进程状态 |
| 6 | 源适配 | `crates/llmgui-download/src/source.rs:100` | 新增 | 只有 `list/fetch`，无 `search/resolve` 能力 |
| 7 | CAS 缓存 | `crates/llmgui-download/src/cache.rs:71` | 确认 | 内容寻址与硬链接可用；无删除、引用计数、GC |
| 8 | 产物 manifest | `crates/llmgui-core/src/manifest.rs:9` | 排除 | schema v1 描述下载产物，**不扩成**用户偏好/库索引载体 |
| 9 | 注册表预留 | `crates/llmgui-core/src/paths.rs:41` | 新增 | `registry_path()` 已存在但全仓无调用 |
| 10 | GGUF 元数据 | `crates/llmgui-models/src/lib.rs:36` | 确认 | 可读 header/metadata/上下文，导入与详情直接复用 |
| 11 | 显存预估 | `crates/llmgui-core/src/estimate.rs:58`；`crates/llmgui-models/src/lib.rs:108` | 确认 | 公式与 24/32GB 判定已有，仅 CLI `estimate` 使用，未做加载闸门 |
| 12 | 调度/取消 | `crates/llmgui-scheduler/src/lib.rs:8` | 确认 | 有 slot 池与取消，可作为 drain 与切换的协作点 |
| 13 | GUI 模型页 | `gui/app.js:15`、`gui/index.html:62`、`gui/app.js:540` | 新增 | 静态 `DEFAULT_MODELS` + 只读 `/v1/models`；无库/下载/加载操作 |
| 14 | 训练导出 | `python/llmgui_train/src/llmgui_train/export.py:203/433/507` | 确认 | 有 adapter/merge/GGUF 导出，无注册与 lineage |
| 15 | 预设/别名 | 无 | 新增 | 无数据结构、无解析入口 |
| 16 | 离线模式 | `docs/research/download.md` §4.4 已有设计，代码未实现 CLI 开关 | 新增 | 库/加载/聊天离线可用，Discover 显式报错 |
| 17 | 多模型常驻 | ADR 0001 §7.4 明确「默认单大模型常驻」 | 排除 | 本 ADR 不做单卡多模型并发；只做切换与 JIT |
| 18 | 托管 Hub / 发布 | 无 | 排除 | 非目标；只做仓库/URL 解析与描述符导入导出 |

## 3. 目标与非目标

### 3.1 目标

- G1：本机模型库可重建、可查询：安装态/加载态分离，列表与磁盘一致；500 条目下 API p95 ≤50ms。
- G2：发现→量化/显存/许可确认→下载→入库是一条可恢复的任务链；50% 中断可续传并通过 SHA256。
- G3：无 `--model` 也能启动服务；GUI/CLI 可加载、卸载、切换；切换前 drain，失败回滚，显存不足在加载前拒绝。
- G4：每个模型可保存加载默认值、别名与命名预设，CLI/GUI/OpenAI 客户端共用同一解析结果。
- G5：训练/导出产物自动注册为派生模型，记录 base/adapter/量化/评测 lineage，可直接加载对话。
- G6：磁盘可治理：删除使用中模型被拒绝，共享 blob 不被误删，显式 GC 可回收。
- G7：零构建 GUI 完成「库 / 发现 / 下载」三视图，全部动作有 CLI/HTTP 对等入口。
- G8：所有写路径单写者、原子化、可恢复；registry 损坏可由 manifest 重建。

### 3.2 非目标

- N1：不实现单卡多模型同时常驻或跨模型批处理；并发承诺仍按 ADR 0001（1–8 槽位）。
- N2：不支持 MLX/NVFP4/safetensors 直接推理；safetensors 仅作训练底座（ADR 0001 §7.8）。
- N3：不做托管 Hub、账号、社交发布、模型评分；`model.yaml` 只做导入/导出子集。
- N4：不改训练算法、量化算法、推理性能目标；这些仍以 ADR 0001 为准。
- N5：不做多用户/RBAC/远程多租户；admin API 默认 loopback。
- N6：不引入遥测或云端索引；离线模式下所有本地能力可用。

## 4. 口径映射（C1..Cn）

| 约束 | 设计落点 |
|---|---|
| C1 | §7.1 注册表、§7.4 扫描/导入、§8.1 `GET /admin/models` |
| C2 | §7.2 目录与发现、§7.3 量化/适配展示、§8.2 catalog/search |
| C3 | §7.3 下载任务状态机、§8.3 downloads API/SSE |
| C4 | §7.1 详情字段、§7.4 元数据解析、§8.1 `GET /admin/models/{id}` |
| C5 | §7.5 ModelManager、§8.4 load/unload/loaded、§8.5 OpenAI 语义 |
| C6 | §7.6 defaults/alias/preset、§8.6 preset API |
| C7 | §7.7 训练产物接入、§8.7 train/export 契约 |
| C8 | §7.9 GUI 信息架构 |
| C9 | §7.8 CLI/API 对等矩阵、§8 全节 |
| C10 | §10 安全与合规、§5 不变量、§11 负例测试 |
| C11 | §9 兼容迁移、§11 测试、§12 分期、§13 预算、§14 风险、§15 决策、§18/§19 落地与交接 |

## 5. 不变量

- I1：`manifest.json` 是下载产物的权威元数据；registry 是索引与用户偏好，**不写回** manifest。registry 损坏/删除后，仅凭 `models/*/manifest.json` 必须能重建出模型列表（用户偏好可能丢失，需告警）。
- I2：任何时刻只有一个写者修改 registry；写入走临时文件 + `rename`，并递增 `generation`。
- I3：一个模型 id 唯一；别名在所有模型间唯一。解析顺序为「精确 id → 别名 → 唯一前缀」，多义直接 409，不猜测。
- I4：删除模型前必须确认未被加载、未被训练任务占用、未被 adapter/派生模型引用；外部目录只注销不删文件。
- I5：共享 blob 只能在没有引用时回收；GC 支持 dry-run，默认保留 24h 宽限。
- I6：加载前必须通过显存闸门（估算 + 可选实机可用显存）；不足时拒绝并给出降 ctx/KV/量化建议，不依赖 OOM 崩溃。
- I7：所有模型路径必须在锚定根目录内（`ensure_within` 语义），只接受白名单扩展名，拒绝 pickle/可执行/符号链接逃逸。
- I8：GUI 不直接读写磁盘或 registry；所有动作经本地 API，服务端状态是唯一权威。
- I9：离线模式不发起任何外网请求；库、加载、聊天、预设、删除本地模型全部可用。
- I10：OpenAI 兼容契约只增不改：`/v1/models` 仍返回标准字段，`llmgui` 扩展字段放在独立对象内。

## 6. 总体架构

```text
┌──────────────────────────── 本机 127.0.0.1:11435 ────────────────────────────┐
│                                                                              │
│  GUI (gui/, 零构建)          CLI (llmgui ...)          OpenAI 客户端           │
│   模型库/发现/下载             models/get/load/...      /v1/chat/completions    │
│        │                          │                          │                │
│        └────────── HTTP/SSE ──────┴──────────────────────────┘                │
│                                    ▼                                         │
│                       llmgui-server（axum，Bearer，loopback）                  │
│        /admin/models  /admin/downloads  /admin/catalog  /v1/*                │
│                                    │                                         │
│                    ┌───────────────┼────────────────┐                        │
│                    ▼               ▼                ▼                        │
│              ModelManager      DownloadManager   Catalog/Source              │
│              - registry         - job 状态机       - 种子目录                 │
│              - prefs/presets     - 队列/暂停        - 仓库/URL 解析           │
│              - load/unload       - SSE 进度         - 量化/适配               │
│              - JIT/TTL           - 空间预检                                   │
│                    │               │                                         │
│         ┌──────────┴──────┐        ▼                                         │
│         ▼                 ▼    llmgui-download（CAS/分片/续传/SHA256）        │
│  Scheduler/Backend     llmgui-models（GGUF metadata/量化/显存估算）           │
│  load/unload/prefix     │                                                    │
│         │               └──> models/<id>/manifest.json（权威产物元数据）       │
│         ▼                                                                    │
│  GPU lease（与训练互斥） <──── python/llmgui_train（train → merge → GGUF）     │
│                                    │                                         │
│                              adapter/derived 注册 ──> registry               │
└──────────────────────────────────────────────────────────────────────────────┘
```

数据流：

1. **入库**：下载完成 / 目录扫描 / 手工导入 → 读 GGUF metadata → 校验（大小、SHA256）→ 写/更新 manifest → 合并进 registry（`generation +1`）。
2. **服务**：加载请求 → id/alias 解析 → 显存闸门 → drain → `backend.load` → 更新 loaded 状态 → `/v1/models` 与 GUI 读取。
3. **作业**：下载/导入校验/导出注册都是持久化 job，SSE 推送，可取消；崩溃后重启进入 paused，可恢复。
4. **训练**：训练前申请 GPU lease → 服务 drain/unload → 训练/导出 → 注册派生模型 → 释放 lease → 可选自动回载上一个模型。

## 7. 关键设计

### 7.1 注册表与模型身份

`<LLMGUI_HOME>/registry.json`，schema 1，单文件、原子写、文件锁（跨进程）。内容分两部分：

- `models[]`：**可由 manifest 重建的派生索引**（id、路径、量化、架构、上下文、许可、文件、大小、fit、来源、时间）。
- `prefs{id: {...}}`：用户偏好（display_name、alias、favorite、tags、load_defaults、last_used），重建时按 id 保留。

模型身份：安装 id 必须是**路径安全**的（Windows 目录名约束）。下载器的仓库 id 已做 `owner--repo` 规范化（`crates/llmgui-download/src/cache.rs:25`）；P1 起同一仓库多量化用 `<repo-id>--<quant>` 区分（如 `Qwen--Qwen3-14B-GGUF--q4-k-m`），分片文件共用同一 id；已存在的旧 `<repo-id>` 目录按 legacy id 导入，不强制改名，再装其他量化时使用新 id 与新目录。导入外部文件时生成 `local--<name>--<quant>`。OpenAI 的 `model` 字段按 I3 解析。

详情字段（C4）：architecture、n_layer/n_head/n_head_kv/n_embd、n_ctx_train、quant + bits/weight、license、file 表（path/size/sha256）、source/repo/revision、installed_at、last_used、fit 矩阵（24/32GB × ctx 8k/16k/32k × KV f16/q8_0/q4_0）。fit 由 `estimate_memory` 计算并缓存，加载参数变化时重算。

### 7.2 目录、发现与仓库解析

三级来源：

1. **种子目录**（随包发布，`models/v0.toml` 升级为 `catalog/v1.toml` 的兼容子集）：策展模型 + 角色/用途/许可/推荐量化。
2. **仓库/URL 解析**：用户粘贴 `owner/repo`、ModelScope/HF URL；调用 `Source::resolve` 列出文件、按量化分组、展示体积与 fit。
3. **Hub 关键词搜索**（可选能力）：源适配器声明 `search` 能力；不可用/超时自动降级到种子目录 + 仓库解析，不阻塞主流程。

量化分组规则：按文件名识别 `Q4_K_M/IQ4_XS/Q8_0/...`（复用 `infer_quant`，`crates/llmgui-download/src/manifest.rs:60`），同组多分片合并为一项；每项给出体积、显存估算、24/32GB 适配、是否需 imatrix。许可未知时不隐藏条目，但下载前必须确认。

### 7.3 下载任务与磁盘治理

任务状态机（持久化在 `<home>/downloads/<job_id>.json`）：

```text
queued → resolving → downloading → verifying → installed
   ↑         │            │            │
   └─ retry ─┴─ failed ←──┴─ paused ←──┘（取消 → cancelled）
```

- 队列默认并发 1、上限可配；同一文件/仓库去重，重复请求返回现有 job。
- 复用现有 `.part` + `state.json` 续传；重启后 running 任务标记 paused，由用户/配置恢复。
- 进度经 SSE 推送（`progress`/`state`/`error` 三类事件，≤2Hz/任务，支持断线重连补发）。
- 空间预检 ×1.1；低于阈值拒绝入队；磁盘占用页显示「模型/缓存/可回收」三块。
- 删除：注销 registry + 移除模型目录；`purge` 才删 blob，且先做引用检查（I4/I5）。外部模型只注销。
- GC：`llmgui cache gc [--dry-run] [--grace-hours 24]`；GUI 提供「清理未引用缓存」。

### 7.4 扫描、导入与元数据

- 扫描根：`<home>/models`（托管）+ 配置 `models.dirs[]`（外部）；识别 `publisher/model/file.gguf` 与扁平布局。
- 增量：按 `path + size + mtime` 跳过未变化文件；只对新增/变化文件读 GGUF header（`GgufInfo::open`）。
- 导入：单文件/目录；目标可选 `copy | link | external`。`external` 不搬移文件，只登记锚定路径。
- 校验：下载产物沿用 manifest SHA256；外部导入创建**后台校验 job**（大文件不阻塞 UI），状态 `verifying → ready`，失败拒绝注册。
- 拒绝：非 GGUF/非训练用 safetensors、pickle/`.bin`、绝对路径/`..`/盘符/UNC、符号链接逃逸。
- 训练底座（safetensors）单独 `kind=train`，不进入推理可加载列表。

### 7.5 运行时生命周期（ModelManager）

单活跃模型（N1），`ModelManager` 持有：

```text
ModelManager
├── registry: RegistryStore（RwLock + 文件锁 + generation）
├── backend: Arc<dyn Backend>（现有 mock/llamacpp/upstream）
├── scheduler: Arc<Scheduler>（drain、可用 slot）
├── loaded: RwLock<Option<LoadedModel>>（id、alias、path、LoadConfig、loaded_at、ttl）
└── lease: GPU lease 协调（与训练互斥）
```

- **load**：解析 id/alias → 校验文件/许可 → `estimate`（ctx/KV/slots）→ 可选 NVML 可用显存 → 不足则 409 `insufficient_memory` → drain（停止接收新请求，等待在飞请求，超时可配）→ `backend.load` → 原子替换 loaded → 失败恢复旧模型。
- **unload**：drain → `backend.unload`（已有原语）→ 清 loaded；`all` 变体逐个卸载。
- **switch**：同一把锁内 unload→load；失败回滚到旧模型，返回明确错误。
- **JIT**：chat 请求的 `model` 未加载但已安装时，single-flight 加载（并发首个请求合并），随后按正常路径生成；未安装 → 404 `model_not_found`。
- **TTL**：`idle_ttl_secs`（默认 0 = 不自动卸载）；无在飞请求且空闲超时后 unload，事件通知 GUI。
- **GPU lease**：训练申请 lease → 服务 drain + unload → 写锁 → 训练结束释放 → 可选 `auto_reload` 上一个模型；与 ADR 0001 §7.4 的 `gpu.lock` 口径一致。

### 7.6 默认值、别名与预设

- `load_defaults`：ctx、kv、gpu_layers、slots、ttl_secs、flash_attn（后端支持时）；保存后 `llmgui load <id>` 与 GUI 点击加载都使用同一份。
- `alias`：全局唯一；`--identifier` 语义对齐 LM Studio；别名解析结果在 `/v1/models` 以 `id` 返回，避免客户端拿到不稳定名字。
- 预设：`<home>/presets/<model_id>.json`，含 `system_prompt`、采样参数、可选 template 覆盖；可命名多个；导入/导出为 JSON。加载参数仍归 `load_defaults`（对齐 LM Studio 把 load 参数移出 Preset 的做法）。
- `model.yaml`：P3 提供**导入/导出子集**（model、metadataOverrides、config.load/operation）；内部不使用该文件。

### 7.7 训练与导出产物接入

- 训练底座选择器只列 `kind=train` 且 safetensors 完整的模型；训练配置记录 base id/revision。
- adapter 注册表：`<home>/adapters/<adapter_id>/manifest.json`（base、r/alpha、target_modules、数据指纹、指标、工具版本）。
- 导出注册：merge → GGUF → 量化（Q4_K_M 默认，IQ4_XS 可选，imatrix 记录）→ 生成 manifest + registry 条目，`derived_from` 指向 base，`adapter_id` 关联 adapter，量化与评测（ppl/KLD）写入 manifest。
- 不匹配（架构/形状/base revision 不同）拒绝加载；导出失败保留旧产物（ADR 0001 §7.7 已有约束）。
- GUI Train 页从库中选底座、展示 adapter/导出状态，并提供「在聊天中打开」。

### 7.8 CLI / API 对等

| 动作 | CLI | HTTP | GUI |
|---|---|---|---|
| 列库/详情 | `models list/show --json` | `GET /admin/models[/{id}]` | 模型库 |
| 扫描/导入 | `models scan/import` | `POST /admin/models/scan\|import` | 扫描/导入按钮 |
| 删除/GC | `models rm`、`cache gc` | `DELETE /admin/models/{id}`、`POST /admin/cache/gc` | 删除/清理 |
| 发现/量化 | `models search <q>` | `GET /admin/catalog/search` | 发现 |
| 下载 | `models get` | `POST /admin/downloads` + SSE | 下载 |
| 加载/卸载 | `load/unload/ps` | `POST /admin/models/{id}/load` 等 | 加载控件 |
| 预估 | `models estimate --ctx/--kv` | `GET /admin/models/{id}/estimate` | 详情矩阵 |
| 预设/别名 | `models preset/alias` | `PUT /admin/models/{id}/preset` | 详情编辑 |

所有命令提供 `--json`；GUI 使用同一 API，不存在「GUI 私有路径」。ADR 0001 §7.8 的 `POST /admin/models/load` 保留为 `/admin/models/{id}/load` 的兼容别名。

### 7.9 GUI 信息架构

模型页改为三视图（同一个 `#view-models` 内切换，保持零构建）：

- **模型库**：库卡片/表格（体积、量化、ctx、许可、fit、来源）、搜索与筛选（适配/用途/量化/许可/来源）、排序（最近使用/体积/名称）、批量选择（删除/校验/导出预设）、磁盘占用摘要。
- **发现**：种子目录 + 仓库/URL 解析；量化对比表（体积/显存/适配/质量）；许可与来源确认后入队下载。
- **下载**：任务列表（状态、进度、速度、剩余时间）、暂停/恢复/取消/重试、完成后「去加载」。
- **详情抽屉**：metadata、文件与哈希、fit 矩阵、加载默认值、别名、预设、删除；加载中/被引用时按钮禁用并说明原因。
- 状态口径：加载态来自 `/admin/models/loaded`，安装态来自 `/admin/models`，任务态来自 `/admin/downloads`；`generation` 变化触发重新拉取，不在前端拼权威状态。
- 可访问性：表格/卡片键盘可达、`aria-pressed`/`aria-live`、空态/错误态可操作；继续使用 `textContent`，无 `innerHTML`。

### 7.10 可观测性

- 指标：`model_scan_seconds`、`model_list_latency`、`model_load_seconds`、`model_load_failures`、`model_unload_total`、`download_bytes_total`、`download_jobs{state}`、`registry_generation`、`cache_bytes{kind}`、`gpu_lease_wait_seconds`。
- 日志：job id、model id、duration、结果；不打印 token、本地绝对路径可按配置脱敏。
- 事件：`/admin/events` SSE（model_changed/download_changed/loaded_changed）供 GUI 失效缓存；保留轮询兜底。

## 8. 数据与接口契约

### 8.1 registry.json（schema 1）

```json
{
  "schema": 1,
  "generation": 42,
  "updated_at": "2026-11-03T12:00:00Z",
  "models": [
    {
      "id": "qwen3-14b-q4km",
      "display_name": "Qwen3-14B Q4_K_M",
      "kind": "inference",
      "path": "models/qwen3-14b-q4km",
      "manifest_path": "models/qwen3-14b-q4km/manifest.json",
      "location": { "kind": "managed" },
      "source": { "repo": "Qwen/Qwen3-14B-GGUF", "type": "modelscope", "revision": "master" },
      "quant": { "format": "gguf", "kind": "Q4_K_M", "bits_per_weight": 4.89 },
      "architecture": "qwen3",
      "context": { "train": 40960, "max": 40960 },
      "license": "Apache-2.0",
      "size_bytes": 9300000000,
      "files": [{ "path": "Qwen3-14B-Q4_K_M.gguf", "size": 9300000000, "sha256": "…" }],
      "fit": {
        "32gb": { "32768": { "q8_0": true, "f16": true }, "16384": { "q8_0": true, "f16": true } },
        "24gb": { "32768": { "q8_0": false, "f16": false } }
      },
      "installed_at": "2026-11-03T11:00:00Z",
      "verified": true
    }
  ],
  "prefs": {
    "qwen3-14b-q4km": {
      "alias": "qwen3-14b",
      "favorite": true,
      "tags": ["chat"],
      "last_used": "2026-11-03T12:30:00Z",
      "load_defaults": { "ctx": 32768, "kv": "q8_0", "gpu_layers": -1, "slots": 2, "ttl_secs": 0 }
    }
  }
}
```

约束：`id` 是 registry 安装 id（路径安全、唯一），可与 `manifest.id`（仓库身份）不同；`prefs` 键必须存在于 `models` 或显式标记 `orphan`（扫描后清理）；写入用 `<file>.tmp` + rename；锁文件 `<home>/registry.lock`。

### 8.2 库与发现

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/admin/models?query=&role=&fit=&quant=&license=&sort=&limit=&offset=` | 分页返回 `{data, total, generation}` |
| GET | `/admin/models/{id}` | 详情（含 fit 矩阵、文件、来源、许可） |
| POST | `/admin/models/scan` | `{dirs?, dry_run}` → `{job_id}` 或同步 dry-run 结果 |
| POST | `/admin/models/import` | `{path, mode: copy\|link\|external, id?}` → `{job_id}` |
| DELETE | `/admin/models/{id}?purge=true\|false` | 加载中/被引用 → 409；外部只注销 |
| GET | `/admin/models/{id}/estimate?ctx=&kv=&slots=` | 返回 `MemoryEstimate` + 建议 |
| GET | `/admin/catalog/search?q=&source=&limit=` | 来源能力不可用时返回 501 + 降级说明 |
| GET | `/admin/catalog/resolve?repo=&revision=` | 文件/量化分组 + 体积 + fit + 许可 |

### 8.3 下载

| 方法 | 路径 | 说明 |
|---|---|---|
| POST | `/admin/downloads` | `{repo, revision?, files[], source?, quant_hint?}` → `{job_id}`；重复返回现有 job |
| GET | `/admin/downloads?state=` | 任务列表 |
| GET | `/admin/downloads/{job_id}` | 单任务快照 |
| GET | `/admin/downloads/{job_id}/events?since=` | SSE：`state`/`progress`/`error`，断线可补发 |
| POST | `/admin/downloads/{job_id}/pause\|resume\|cancel\|retry` | 状态机合法迁移；非法迁移 409 |
| GET | `/admin/storage` | `{models_bytes, cache_bytes, reclaimable_bytes, disk_free_bytes}` |
| POST | `/admin/cache/gc` | `{dry_run, grace_hours}` → `{reclaimed_bytes, kept[]}` |

### 8.4 运行时

| 方法 | 路径 | 说明 |
|---|---|---|
| POST | `/admin/models/{id}/load` | `{ctx?, kv?, gpu_layers?, slots?, ttl_secs?, alias?, force?}`；`force` 仅覆盖软检查，不绕过显存闸门 |
| POST | `/admin/models/{id}/unload` / `POST /admin/models/unload` | `{all: true}` 变体 |
| GET | `/admin/models/loaded` | `{data: [{id, alias, path, ctx, kv, slots, loaded_at, ttl_secs}], generation}` |
| POST | `/admin/models/{id}/verify` | 后台重算 SHA256，更新 `verified` |
| GET/PUT/DELETE | `/admin/models/{id}/preset[/{name}]` | 预设与 defaults |
| PUT | `/admin/models/{id}/alias` | `{alias}`；冲突 409 |

兼容别名：`POST /admin/models/load`（body 含 `id`）保留。

### 8.5 OpenAI 语义

- `/v1/models` 只返回**当前已加载**的模型（含 JIT 可立即加载的已安装模型？否——只返回 loaded，避免客户端误判）；`id` 为 canonical id，`owned_by` 为 `llmgui`。扩展字段放 `llmgui: {alias, quant, size_bytes, context, fit}`，不污染标准字段。
- `POST /v1/chat/completions` 的 `model`：空/`default` → 当前 loaded；匹配 id/alias → 已加载则直接用，未加载且安装 → JIT；未安装 → 404 `model_not_found`；加载中 → 409 `model_busy` 或按配置排队。
- 错误沿用 OpenAI 风格 `{"error":{"message","type","code"}}`，新增 code：`model_not_found`、`model_busy`、`insufficient_memory`、`registry_locked`、`unsafe_path`、`download_conflict`、`license_required`。

### 8.6 预设与 CLI 输出

预设 JSON：

```json
{
  "schema": 1,
  "model_id": "qwen3-14b-q4km",
  "name": "code-review",
  "system_prompt": "You are a strict code reviewer.",
  "sampling": { "temperature": 0.3, "top_p": 0.9, "max_tokens": 1024 },
  "template": { "override": null }
}
```

CLI 统一 `--json` 输出 `{ok, data|error: {code, message}, generation}`；人类输出只用于终端展示，脚本以 JSON 为准。

## 9. 兼容与迁移

- **manifest schema v1 不变**：新增库/偏好/lineage 全在 registry 与独立 JSON，旧模型目录无需迁移。registry 的 install `id` 与 `manifest.id` 解耦：manifest 仍记录仓库身份，registry 可用量化后缀区分同仓库多个安装。
- **多量化目录**：P0 只扫描不改变布局；P1 起下载写入 `<repo-id>--<quant>` 新目录，旧 `<repo-id>` 目录保留（legacy id）；同一 id 已存在且文件表不同时返回 409，不覆盖旧 manifest。
- **首次启动**：若 registry 不存在或 schema 未知，扫描 `models/` 重建；保留同名 `prefs`，失败回退到「仅 manifest」只读模式并记录告警。
- **旧 GUI 静态清单**：`models/v0.toml` 保留为种子目录来源；GUI 在 API 不可用时回退到静态清单（只读），行为与当前一致。
- **CLI 兼容**：`pull/serve/run/estimate/list` 保留；`serve --model` 仍可用（等价于启动后 load），但不再必需。
- **API 兼容**：`/v1/models` 字段只增；`/admin/models/load` 保留别名。
- **回滚**：删除 registry 即回到「扫描视图」；下载任务与模型目录不回滚；新增字段使用 `#[serde(default)]`。
- **Windows 特例**：硬链接失败退化为复制（已有行为）；删除被占用文件进入 `pending_delete`，重启后清理。

## 10. 安全与合规

| 威胁 | 控制 |
|---|---|
| 路径穿越/符号链接逃逸 | 所有路径经锚定根校验；外部根显式配置并 canonicalize；拒绝绝对路径/`..`/盘符/UNC |
| 恶意文件（pickle/可执行） | 扩展名白名单 + 内容为 GGUF/ safetensors；配置文件只读解析，不执行 |
| 下载内容被替换 | manifest SHA256 + 流式校验；导入后台校验；`verify` 命令可复跑 |
| 误删共享数据/使用中模型 | 加载/训练/adapter 引用检查；`purge` 需显式参数；GC dry-run + 宽限期 |
| 删除/加载不存在的模型或别名冲突 | 404/409 明确错误；解析不猜测 |
| 本地 API 被远程访问 | 默认 loopback；远程需 `--allow-remote` + Bearer token（沿用现有中间件） |
| 许可/合规 | 下载前展示并记录 license；未知许可需显式确认；不重分发权重 |
| 凭据泄漏 | 源 token 只对 https 发送、日志脱敏；沿用 `docs/research/download.md` §4.4 |
| 离线环境误联网 | `--offline`/配置开关；Discover 返回 409/501，不发起请求 |
| 磁盘打满/拒绝服务 | 预检 ×1.1、并发上限、任务取消、下载限速可配 |
| 并发写坏 registry | 单写者 + 文件锁 + 原子 rename + generation；损坏可重建 |

## 11. 测试与质量门禁

分层：

- **单元（Rust）**：registry serde/原子写/重建/锁；id 与 alias 解析；fit 计算；job 状态机合法迁移；GC 引用计数；路径与格式校验；preset 合并。
- **集成（Rust + axum）**：临时 `LLMGUI_HOME` + fixture 源；scan/import/list/detail；下载中断续传；SSE 事件顺序与断线补发；load/unload/switch（mock 后端）；加载中删除 409；alias 冲突；JIT 并发合并；TTL 卸载；GPU lease 与 drain。
- **前端（jsdom，沿用现有 `gui` 测试形态）**：库/发现/下载三视图渲染、筛选/排序/搜索、空态/错误态、按钮禁用原因、`aria` 状态、无 `innerHTML`。
- **桌面 e2e**：扩展 `--selftest`：fixture 下载 → 入库 → 加载 → 对话；切模型后再对话；不依赖公网。
- **负例/安全**：路径穿越语料（`..`、绝对路径、UNC、符号链接、Unicode 归一化）、哈希不符、disk full、取消后残留、删除共享 blob、并发 load 竞争。
- **性能**：500 条 registry 的列表 p95、冷/热扫描、SSE 频率、前端首屏；加载耗时基线（P2a 记录）。
- **门禁**：L2 改动走 `make flow`（check-fast → test）；Python 覆盖率维持 `pyproject.toml` 的 95%；GUI 每次 `node --check`；Linux + Windows CI 均需绿；e2e 抖动由重试兜底，不跳过用例。
- **不做**：不为覆盖率写无断言用例；不把第三方 Hub 搜索作为合入门槛（用 fixture）。

## 12. 分期计划

总工作量估算（1–2 名熟悉代码的工程师）：P0 2–3 周、P1 4–5 周、P2a 3–4 周、P2b 3–4 周、P3 3–4 周，合计约 16–20 周。P1 完成即可日常使用；P2a 才解决「换模型要重启」。

| 期 | 工作 | 交付 / 退出标准 | 估算 |
|---|---|---|---|
| P0 | 注册表（schema/锁/原子写/重建）；扫描与导入（GGUF metadata、外部根、后台校验）；`GET /admin/models[/{id}]`；CLI `models list/show/scan/import`；GUI 模型库只读并替换静态清单为 API 数据（失败回退） | registry 可删可重建；GUI 列出真实模型；导入 32B 文件不阻塞 UI；`--json` 稳定；现有 `/v1/models` 行为不变 | 2–3 周 |
| P1 | 种子目录/仓库解析/量化分组；可选 Hub 搜索；下载 job 状态机 + SSE + 暂停/恢复/取消/重试；空间预检、磁盘统计；删除 + CAS 引用计数 + GC；GUI 发现/下载视图 | 下载 50% 中断后续传 SHA256 一致；取消无残留；重复下载去重；删除使用中模型 409；GC dry-run 与回收可验证；fixture 全绿 | 4–5 周 |
| P2a | ModelManager（load/unload/switch/drain/rollback）；显存闸门 + `estimate-only`；JIT single-flight；TTL；GPU lease 联动；alias/load default；`/admin/models/{id}/load` 等；CLI `load/unload/ps`；`serve` 无 `--model`；GUI 加载控件 | 不传 `--model` 可 serve；GUI 在 7B/14B 间切换并对话；流式中切换按 drain/409 口径；OOM 前拒绝并给建议；TTL 到期卸载；崩溃重启回读 loaded 状态 | 3–4 周 |
| P2b | adapter 注册表；底座选择器（safetensors）；导出→merge→GGUF→量化→自动注册；lineage 与 ppl/KLD 记录；GUI Train 联动与「在聊天中打开」 | 一条命令/一次点击完成「下载→训练→导出→注册→加载→对话」；base 不匹配拒绝；派生模型在库中可见 lineage；复现工具版本记录 | 3–4 周 |
| P3 | `model.yaml` 子集导入导出；预设导入导出；批量操作/URL 状态/键盘；旧目录（LM Studio/Unsloth 布局）一键导入；`verify`/崩溃恢复/GC 自动化；性能长稳与安全审计；文档与截图 | 导入导出往返一致；旧目录导入成功；`models verify --all` 全绿；7 天长稳；文档覆盖主流程；安全 checklist 通过 | 3–4 周 |

## 13. 性能预算

基准：RTX 5090 D 32GB / Windows 11 / NVMe；500 条 registry 的合成库；下载基准沿用 ADR 0001 §13.4。

| 指标 | 目标 | 口径 |
|---|---|---|
| 模型列表 API | p95 ≤50ms | 500 条、`limit=50`、本机 |
| 冷扫描 500 目录 | ≤5s | 只读 header/metadata，不哈希 |
| 热扫描 500 目录 | ≤1s | 按 size+mtime 跳过 |
| 库首屏渲染 | ≤200ms | 收到 JSON 后到可交互 |
| 下载进度 SSE | ≤2 事件/s/任务，单个 ≤1KB | 断线重连补发不重放全量 |
| 下载吞吐 | 不低于 ADR 0001 C8（同源 ≥3× 单线程） | 任务化不引入额外拷贝 |
| 加载 7B Q4_K_M | ≤15s（NVMe，目标，P2a 记录基线） | 含 metadata/显存检查，不含首 token |
| JIT 首次请求 | 仅增加加载时间，无重复加载 | 并发首请求 single-flight |
| 管理器内存开销 | ≤128MiB / 500 条 | 不含模型权重 |
| 切换失败回滚 | ≤2s | mock 后端；真实后端以卸载/重载时间为准 |
| registry 写入 | ≤50ms | 原子 rename + fsync（按平台） |

## 14. 风险与缓解

| # | 风险 | 影响 | 缓解 |
|---|---|---|---|
| 1 | registry 与磁盘/manifest 不一致 | 列表错误、加载失败 | 单写者 + 原子写；启动重建；`verify` 命令；generation 失效缓存 |
| 2 | GC 误删共享 blob | 其他模型损坏 | 引用计数 + dry-run + 宽限期；`purge` 显式；测试覆盖硬链接/复制两种布局 |
| 3 | 切换/卸载打断在飞请求 | 用户请求失败、体验差 | drain + 超时 + 409 `model_busy`；不静默杀流；回滚旧模型 |
| 4 | Windows 文件占用/硬链接差异 | 删除失败、空间不释放 | `pending_delete` 重试；硬链接失败退化为复制（已有）；文档说明 |
| 5 | 第三方源搜索/列表 API 漂移 | 发现不可用 | 能力标记 + 种子目录降级 + 仓库 ID 解析 + fixture 测试；不把搜索作为合入门槛 |
| 6 | 显存预估不准 | 加载 OOM | 保守常数 + 实机可用显存 + 加载前拒绝 + 降档建议；`estimate` 误差目标沿用 ADR 0001 |
| 7 | 训练与推理抢 GPU | OOM/卡死 | GPU lease + drain/unload + 排队；auto_reload 可关 |
| 8 | 范围蔓延（Hub/多格式/多运行时） | 延期 | 非目标 N2–N5；分期退出标准；评审卡点 |
| 9 | GUI 复杂度上升（零构建） | 维护成本/回归 | 服务端驱动状态；视图模块化；jsdom 测试；静态文件预算检查 |
| 10 | 大文件导入校验耗时 | 用户以为卡死 | 后台 job + 进度 + 状态 `verifying`；可跳过校验但显式标记 |
| 11 | 别名/id 冲突 | 客户端路由错误 | 全局唯一约束 + 409 + 解析顺序明确；迁移时清理重复 |
| 12 | 许可信息缺失 | 合规风险 | 下载前展示；未知许可需确认；registry 记录 license；不重分发权重 |
| 13 | 磁盘打满 | 下载/导出失败 | 预检 ×1.1；配额告警；失败清理 `.part`；GC 建议 |
| 14 | 训练导出工具版本漂移 | 产物不可复现 | 锁定工具版本并写入 manifest（ADR 0001 §7.7 已有约束） |

## 15. 决策记录

| 编号 | 议题 | 结论 | 依据 | 被否决项 | 落点 |
|---|---|---|---|---|---|
| D1 | 模型管理分层 | 产物 / 库 / 运行时 / 工作流四层，库为核心 | 对标 LM Studio 与 Unsloth；现状缺口清晰 | 只做下载器；只做 GUI 静态展示 | §6、§7 |
| D2 | 库的权威载体 | `registry.json`（模型索引 + prefs），manifest 保持不可变 | 需要用户偏好但不想污染产物元数据；可重建 | 扩 manifest schema；SQLite；只靠目录扫描 | §7.1、§8.1、§9 |
| D3 | 模型身份 | 路径安全的 canonical id（`<repo-id>--<quant>` / `local--…`），全局唯一 alias | 多量化共存、Windows 目录名约束、客户端需要稳定名字 | 文件名/路径作 id；哈希 id；允许重复 | §7.1、I3 |
| D4 | 运行时并发 | 单活跃模型 + drain/rollback + JIT + TTL | 32GB 单卡显存约束；ADR 0001 §7.4 | 单卡多模型常驻；每请求新进程 | §7.5、N1 |
| D5 | OpenAI `/v1/models` | 只列已加载模型，扩展字段只增 | 避免客户端把未加载模型当可用 | 列全部已安装；修改标准字段含义 | §8.5、I10 |
| D6 | 发现策略 | 种子目录 + 仓库/URL 解析为主，Hub 关键词搜索为可选能力 | 第三方搜索无 SLA；仓库解析足够覆盖主流用法 | 只依赖 Hub 搜索；纯离线目录 | §7.2、§14 风险 5 |
| D7 | 下载执行 | Rust 内建 job 队列 + SSE，复用现有分片/续传 | GUI 不直连源；状态可回读；与 CAS 同栈 | GUI 直接下载；仅前台 CLI | §7.3、§8.3 |
| D8 | 删除与回收 | 引用检查 + 显式 purge + GC dry-run/宽限期 | 共享 blob 与使用中模型是主要事故源 | 删除模型即删 blob；永不回收 | §7.3、I4/I5 |
| D9 | 预设与描述符 | per-model defaults + JSON 预设；`model.yaml` 仅导入导出 | LM Studio 把 load 参数移出 Preset；避免绑定草案规范 | 内部用 model.yaml；YAML 预设 | §7.6 |
| D10 | 训练产物 | 派生模型进同一库，记录 lineage | Unsloth 的「训练→导出→运行」闭环；避免第二套列表 | 单独产物区；不注册只打印路径 | §7.7 |
| D11 | 写一致性 | 单写者 + 文件锁 + 原子 rename + generation | 防并发写坏；GUI 可失效缓存 | GUI 直接写 registry；无锁并发写 | §7.1、I2 |
| D12 | GUI 技术路线 | 继续零构建原生 JS，服务端过滤/分页 | ADR 0001 定调；避免 React 重写阻塞交付 | 立即 React/TS 重写；引入构建链 | §7.9 |
| D13 | admin API 形态 | 资源化 `/admin/models/{id}/...`，保留旧 `/admin/models/load` 别名 | 与 REST 习惯一致；兼容 ADR 0001 §7.8 | 只保留扁平命令式端点 | §7.8、§8.4 |
| D14 | 安全基线 | 路径锚定、白名单格式、SHA256、删除确认、loopback 默认 | 本地模型文件与删除操作是主要风险面 | 信任用户输入路径；远程默认开放 | §10、I7 |

## 16. 后果

**正面**：用户从「记命令 + 手工下载 + 重启换模型」变为库内完成全流程；训练产物不再脱节；下载与磁盘可治理；CLI/API/GUI 同一套状态，易测试、易排障；复用现有 CAS/manifest/预估/后端，不重写底座。

**负面**：新增注册表与 job 状态需要维护一致性；单活跃模型的切换有 drain 成本；GUI 代码量与测试面增加；P0/P1 期间用户会同时看到「旧的静态清单」与「新的库」两套数据，需要明确回退语义。

**中性**：`/admin/*` 是本地扩展 API，不承诺跨版本稳定；`model.yaml` 只做子集，规范演进时只影响导入导出；多格式/多运行时候选仍留 post-v1.0。

## 17. 与既有 ADR 的关系

- **ADR 0001 仍是总架构与性能/训练算法的权威**：本 ADR 细化了 §7.4 的模型生命周期、§7.5 的缓存治理、§7.7 的产物接入、§7.8 的入口与 GUI，不改变推理性能目标、量化默认值与训练栈选型。
- **分期关系**：ADR 0001 的 P1-2 已列「unload/hot-swap」但未实现，P2b 只交付静态清单 GUI；本 ADR 把它们重新排为 P0/P1/P2a/P2b，并以本 ADR 的退出标准验收。
- **冲突口径**：模型管理的库/任务/生命周期契约以本 ADR 为准；推理性能、显存公式、量化与训练算法以 ADR 0001 为准。
- 无替代/废弃关系，ADR 0001 继续有效。

## 18. 落地状态

| 期 | 状态 | 代码落点 | 测试 | 提交 |
|---|---|---|---|---|
| P0 | 未开始 | — | — | — |
| P1 | 未开始 | — | — | — |
| P2a | 未开始 | — | — | — |
| P2b | 未开始 | — | — | — |
| P3 | 未开始 | — | — | — |

## 19. 交接

- 当前阶段：方案评审前（Proposed）；本轮只交付需求卡片、调研与 ADR，不进入实现。
- 已完成：需求卡片 0003；`docs/research/model-management.md`（LM Studio / Unsloth 对照）；本 ADR 的现状核对、契约、分期与风险。
- 下一步：人工评审（重点 D2 registry 载体、D4 单活跃模型、D6 Hub 搜索边界、D10 训练产物接入）；评审通过改 Accepted 后从 P0 开始，每期回填 §18/§19。
- 已知坑：
  1. `registry_path()` 已预留但全仓无调用，P0 不要在旧路径上叠第二份索引；
  2. `Backend::unload` 已存在，但 upstream 后端的「卸载」只是状态清理，真实显存由外部 llama-server 持有，P2a 需明确提示；
  3. CAS 目前只增不减（`crates/llmgui-download/src/cache.rs`），P1 的 GC 必须先做引用统计与 dry-run；
  4. GUI 现有模型清单是 `gui/app.js:15` 的静态镜像，P0 切换 API 时保留离线回退，避免服务不可达时页面空白；
  5. 训练导出链路在 Python 侧（`python/llmgui_train/src/llmgui_train/export.py`），P2b 只做注册与 lineage，不改导出算法；
  6. 性能预算中的加载耗时是目标值，需 P2a 用 5090D + NVMe 实测后回填。

## 20. 参考

- 对标调研：`docs/research/model-management.md`（LM Studio / Unsloth 能力拆解与取舍）
- 内部设计：`docs/adr/0001-local-llm-runtime-finetune.md` §7.4/§7.5/§7.7/§7.8；`docs/research/download.md`；`docs/research/runtime.md`
- LM Studio：<https://lmstudio.ai/docs/app/basics/download-model>、<https://lmstudio.ai/docs/app/advanced/import-model>、<https://lmstudio.ai/docs/app/advanced/per-model>、<https://lmstudio.ai/docs/app/presets>、<https://lmstudio.ai/docs/app/modelyaml>、<https://lmstudio.ai/docs/cli>、<https://lmstudio.ai/docs/cli/load>
- Unsloth：<https://unsloth.ai/docs/new/studio>、<https://unsloth.ai/docs/new/studio/start>、<https://unsloth.ai/docs/new/studio/export>、<https://unsloth.ai/docs/desktop>、<https://unsloth.ai/docs/get-started/unsloth-model-catalog>、<https://unsloth.ai/docs/basics/dynamic-3.0-ggufs>
- 需求卡片：`docs/requirements/0003-model-management.md`
