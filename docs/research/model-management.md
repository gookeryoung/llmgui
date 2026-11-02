# 调研：LM Studio / Unsloth 的模型管理与对 llmgui 的借鉴

- 日期：2026-11-03
- 关联需求：C1–C11（`docs/requirements/0003-model-management.md`）
- 方法：阅读官方文档（见 §8，抓取于 2026-11-03），未安装客户端做交互实测；功能描述以文档原文为准。
- 结论适用栈：单机 Rust 运行时 + Python 训练层、GGUF 推理、国内下载源、零构建 Web GUI。

## 1. 结论摘要

1. 两个产品的模型管理都围绕**「一个模型 = 文件集 + 元数据 + 运行配置 + 来源」**展开，并把量化选择、显存适配、加载参数当成模型的一等属性，而不是散落的命令行参数。
2. LM Studio 的重心是**库 + 发现/下载 + 运行时生命周期**：`lms ls/ps/load/unload/get/import` 与 GUI 的 My Models / Discover 一一对应；`--ttl` 空闲卸载、`--estimate-only` 预演显存、`--identifier` 别名、per-model defaults、Presets、`model.yaml` 描述符是可直接借鉴的交互骨架。
3. Unsloth Studio 的重心是**模型库与训练闭环的同一入口**：模型 Hub / HF 实时搜索、按 RAM/VRAM 选量化、下载后直接加载、自动识别已有 GGUF/HF 目录、训练→导出 GGUF/safetensors/LoRA→再运行；Dynamic GGUF 的量化质量/体积权衡提示「量化档位」应作为可比较的数据而非简单文件名。
4. llmgui 当前只有「一次性下载 + `serve --model` + 静态清单 GUI」；缺库、缺下载队列、缺运行时切换、缺训练产物注册。可直接复用现有 CAS、manifest、GGUF metadata、显存预估与 `Backend::unload`，不需要重写下载/推理底座。
5. 不照搬：LM Studio 的 MLX/多运行时、Unsloth 的托管 Hub 发布与训练算法优化不在本项目范围；`model.yaml` 只做导入/导出子集，不做内部存储格式。

## 2. LM Studio 能力拆解

| 能力 | 文档表述 / 交互 | 对 llmgui 的启示 |
|---|---|---|
| 发现与下载 | Discover 支持关键词、`user/model`、完整 HF URL；同一模型提供多个量化选项，建议 4-bit 以上 | 搜索要接受「仓库 ID / URL / 关键词」三种输入；下载前必须展示量化与体积 |
| 模型目录 | My Models 中可改模型目录；`~/.lmstudio/models/publisher/model/file.gguf` 结构化布局 | 库要有「托管目录 + 外部目录」两种来源，扫描时识别常见布局 |
| 导入 | `lms import <file.gguf>`（实验性）或按预期目录结构放入 | 支持单文件导入与整目录导入，写入注册表前先解析 GGUF metadata |
| 列举 | `lms ls` 列出磁盘模型；`lms ps` 列出已加载模型 | 「已安装」与「已加载」必须是两个状态，GUI 与 CLI 口径一致 |
| 加载参数 | `lms load --gpu=0-1|off|max --context-length --identifier --ttl --estimate-only` | 加载前预估、GPU offload 比例、上下文、别名、空闲卸载都应可配置并可保存 |
| 卸载 | `lms unload [--all]` | 需要显式 unload 与「全部卸载」，以及卸载后的状态回读 |
| 每模型默认值 | My Models 齿轮设置 GPU offload / context / Flash Attention；`lms load` 沿用 | per-model defaults 是库的一部分；GUI 保存后 CLI 必须同样生效 |
| 预设 | Preset = system prompt + 采样参数，JSON 可导入/导出/发布 | 预设与加载参数分开：预设管对话，defaults 管加载 |
| 描述符 | `model.yaml` 描述同一模型的 GGUF/MLX 变体、`minMemoryUsageBytes`、`contextLengths`、`config`（load/operation）、`customFields` | 可移植描述符适合导入/导出；内部仍用自有 registry 保证稳定性 |
| 服务 | 本地 server + OpenAI API，模型标识可用 `--identifier` 覆盖 | 别名解析要贯穿 API、CLI 与 GUI；`/v1/models` 只列当前可服务的模型 |

## 3. Unsloth Studio 能力拆解

| 能力 | 文档表述 / 交互 | 对 llmgui 的启示 |
|---|---|---|
| 模型库与搜索 | 推荐模型 + HF 实时搜索；显示最后更新时间；支持自定义模型目录 | Discover 应同时有策展目录与「按仓库/URL 解析」，并显示来源与更新时间 |
| 量化选择 | 同一仓库多个量化，按可用 RAM/VRAM 选择；示例选 `UD-Q4_K_XL` | 量化列表要给出体积、显存估算、适合档位，而不是只给文件名 |
| 下载即加载 | 点击模型变体后下载并加载；进度在 UI 反馈 | 下载任务与加载动作可以串联，但状态机要分开，失败可重试 |
| 旧模型识别 | 自动检测以前从 HF 下载的 GGUF/safetensors；本地目录 `~/.unsloth/studio/models` | 首次扫描要能接管已有模型目录，避免用户重新下载 |
| 训练闭环 | 训练→导出 GGUF/safetensors/LoRA→在同一界面运行；导出可给 llama.cpp/Ollama/LM Studio 使用 | 训练产物必须进入同一个模型库，并记录 base/adapter/量化 lineage |
| 运行配置 | 自动推理参数、可编辑 chat template、上下文长度选项、GPU/VRAM 展示 | 可提供「建议参数 + 一键应用」，但不静默覆盖用户设置 |
| 系统信息 | `GET /api/system` 返回 GPU/CPU/memory，UI 显示 VRAM 使用 | 显存适配需要「估算 + 实机可用显存」双来源，UI 展示当前占用 |
| 量化质量 | Dynamic 3.0 GGUF 用 KLD / Top-1 对比同体积量化 | 量化档位可附带质量指标（本项目 P2b 起记录 ppl/KLD） |
| 服务与切换 | OpenAI 兼容 API、模型 swapping、连接外部 Agent | 运行时切换与别名解析要支持外部客户端，不只是 GUI |

## 4. 共性抽象

1. **四层模型管理**：产物（文件 + metadata）→ 库（安装/导入/删除/磁盘）→ 运行时（加载/卸载/切换/预估）→ 工作流（下载、训练、导出、预设）。
2. **量化是一等维度**：同一模型的不同量化必须能并列比较（体积、显存、质量、适用档位），并与下载选择、库展示、加载预估共用一份数据。
3. **适配性前置**：下载前与加载前都要回答「这台机器跑不跑得动」，而不是失败后再报 OOM。
4. **状态可回读**：磁盘上有哪些模型、内存里加载了哪个、下载到哪一步，都必须由服务端给出唯一答案，CLI/GUI 只是视图。
5. **可移植性**：别名、预设、描述符让「同一模型 + 不同用途」可复用，而不复制权重。

## 5. 差异与不适用项

| 能力 | LM Studio / Unsloth | llmgui 取舍 |
|---|---|---|
| 多推理格式 | MLX、NVFP4、safetensors 直接推理 | 只做 GGUF 推理；safetensors 仅训练底座（ADR 0001） |
| 托管 Hub / 社交发布 | LM Studio Hub、Unsloth 模型页 | 不做托管与发布；只做仓库/URL 解析与描述符导入导出 |
| 多运行时并存 | LM Studio runtime 管理、多种引擎 | v1.0 单后端单活跃模型；多运行时候选留 post-v1.0 |
| 自动调参 | Unsloth 自动推理参数 | 只给建议值与一键应用，不静默改用户配置 |
| 训练算法 | Unsloth 融合内核 / Dynamic 量化 | 训练算法仍按 ADR 0001；本计划只接产物管理 |
| 云/远程 | 远程实例、Cloudflare 暴露 | 保持本机 loopback 优先；远程仅显式开启 |

## 6. 对 llmgui 的落地建议

| 建议 | 类型 | 说明 |
|---|---|---|
| 以「库」为模型页首页，静态 `models/v0.toml` 降级为种子目录 | 采纳 | 用户先看到真实可用的模型，再进入 Discover |
| 安装态与加载态分离（`ls` / `ps` 语义） | 采纳 | 避免「列表里有但点了跑不了」的误导 |
| 下载任务持久化 + SSE 进度 + 暂停/取消/续传 | 采纳 | 复用现有 `.part`/state.json；任务状态可跨页面刷新 |
| 量化 + 显存 + ctx 三联选择器 | 采纳 | 下载前回答「跑不跑得动」；32GB/24GB 双档 |
| per-model defaults + 别名 + 命名预设 | 采纳 | 对应 LM Studio 的 defaults/Presets 与 Unsloth 的自动参数建议 |
| 加载前 `estimate-only` 与实机 VRAM 双校验 | 采纳 | 复用 `estimate_memory`；新增 NVML 可用显存探测（可选） |
| TTL 空闲卸载、JIT 按需加载 | 采纳（P2a） | 单卡场景降低常驻占用；与 GPU lease 协同 |
| 训练产物以派生模型进入同一库（lineage） | 采纳 | base/adapter/量化/评测写入 registry 与 manifest |
| 旧模型目录导入（LM Studio/Unsloth 常见布局） | 采纳（P0/P1） | 扫描 + 显式 import，不强制搬移 |
| `model.yaml` 兼容导入/导出子集 | 采纳（P3） | 只映射 model/metadata/config，不做内部格式 |
| 多格式推理、Hub 发布、多运行时 | 拒绝/延后 | 与 ADR 0001 边界冲突或收益不足 |

## 7. 与 llmgui 现状的差距

| 差距 | 现状 | 影响 |
|---|---|---|
| 无安装库索引 | `registry.json` 路径已预留但未使用；`llmgui list` 仅遍历 manifest | GUI 无法知道本机装了什么 |
| 无下载任务 | `llmgui pull` 前台执行，进度只打印到 stderr | 下载大模型时无法暂停/恢复/并发管理 |
| 无 Discover | `Source` trait 只有 `list/fetch`；GUI 清单是手写镜像 | 无法按机器能力选量化 |
| 无运行时切换 | `serve --model` 一次性加载，`/v1/models` 固定一条 | 换模型必须重启进程 |
| 无删除/GC | CAS 只增不减 | 长期使用磁盘只涨不降 |
| 无预设/别名 | 模型 id 来自文件名 | 客户端与用户需要记文件名 |
| 训练产物脱节 | 导出工具齐全，但 manifest 不注册到库 | 「训练完还要手工 serve」 |

## 8. 参考

- LM Studio：<https://lmstudio.ai/docs/app/basics/download-model>、<https://lmstudio.ai/docs/app/advanced/import-model>、<https://lmstudio.ai/docs/app/advanced/per-model>、<https://lmstudio.ai/docs/app/presets>、<https://lmstudio.ai/docs/app/modelyaml>、<https://lmstudio.ai/docs/cli>、<https://lmstudio.ai/docs/cli/load>
- Unsloth：<https://unsloth.ai/docs/new/studio>、<https://unsloth.ai/docs/new/studio/start>、<https://unsloth.ai/docs/new/studio/export>、<https://unsloth.ai/docs/desktop>、<https://unsloth.ai/docs/get-started/unsloth-model-catalog>、<https://unsloth.ai/docs/basics/dynamic-3.0-ggufs>
- 内部：`docs/research/download.md`（源与缓存）、`docs/research/runtime.md`（显存与调度）、`docs/adr/0001-local-llm-runtime-finetune.md`（§7.4 / §7.5 / §7.7 / §7.8）
