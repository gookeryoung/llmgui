# ADR 0001：本地大模型运行与微调框架（Rust + Python）

- 状态：Accepted（需求方 2026-11-02 指示“按此推进到P3”，视为方案评审通过）
- 日期：2026-11-02
- 关联需求卡片：[0001-local-llm-runtime-finetune](../requirements/0001-local-llm-runtime-finetune.md)
- 关联调研：[runtime](../research/runtime.md) · [finetune](../research/finetune.md) · [download](../research/download.md)
- 决策摘要：Rust 单进程运行时（llama.cpp 进程内执行 + Rust 调度/服务/下载/显存治理）+ 独立 Python 训练进程（PyTorch/PEFT/TRL + Liger）；GGUF Q4_K_M 为默认推理格式；ModelScope 主源 + hf-mirror/Gitee AI 兼容入口 fallback；CLI + OpenAI 兼容 API 先交付，Tauri GUI 放 P2b。

## 1. 背景与问题

用户要求基于 Rust、Python 技术栈，参考 Unsloth 的设计，做一个「高性能本地大模型运行 + 微调框架」，做到运行快、省内存，模型从国内源下载以提升速度，并先给出开发方案。

当前市场上的选择各有缺口：

- **纯 Python 方案**（Transformers + FastAPI / vLLM）生态最全，但冷启动、常驻内存、分发体积和 Windows 体验不理想；微调与推理解释器通常各自为政。
- **C++ 推理服务**（llama.cpp server / Ollama）性能与量化成熟，但模型管理、国内下载、微调闭环、桌面体验薄弱，二次开发受限于 C++ 服务边界。
- **Unsloth** 在单卡 QLoRA 上把「省显存、快训练」做到了很高水平，但它是训练库，不解决本地推理服务、模型获取与运行/微调一体化。
- **国内模型下载**：Hugging Face 直连不稳定，ModelScope / hf-mirror 可用但接口、哈希、镜像漂移各自不同，缺少统一的校验、缓存与 fallback 层。

因此本项目的核心不是重写算子，而是把「**执行层（成熟内核）+ Rust 治理层 + Python 训练层 + 国内分发层**」组合成一个可交付的本地产品，并用可复现基准证明「快」和「省」。

## 2. 现状核对

仓库为 greenfield，本项目从零建设：

- `git ls-files` 仅 15 个文件：`AGENTS.md`、`.gitignore`、`.trae/rules/*.md`、`.trae/skills/*/SKILL.md`，以及 `docs/AI_DRIVEN_DEV_FLOW.md`、`docs/BUNDLE_BUDGET.md`、`docs/REFACTOR_BACKLOG.md`、`docs/REQUIREMENT_CARD_TEMPLATE.md`；无 `src/`、`crates/`、`python/`、`Makefile`、`pyproject.toml`、`Cargo.toml`。
- `docs/AI_DRIVEN_DEV_FLOW.md`、`BUNDLE_BUDGET.md`、`REFACTOR_BACKLOG.md` 是 `cndb` 项目残留：引用不存在的 ADR 0004、`scripts/check_adr_landing.py`、`.github/workflows/*`、`frontend/` 与 `src/cndb/`。它们不能作为本项目的流程与门禁依据，P0 需改写或移出本项目口径。
- `AGENTS.md:35-37` 要求开工前读 `docs/AI_DRIVEN_DEV_FLOW.md` 与 `docs/adr/README.md`；`design.md:17-19` 引用 `docs/adr/TEMPLATE.md`、`docs/adr/0004-*.md`。`docs/adr/TEMPLATE.md`、`docs/adr/README.md` 已由本方案补齐；`docs/adr/0004-*.md`、`Makefile`、门禁脚本与双 CI 仍缺失，P0 需补齐或删除失效引用。
- 需求模板 `docs/REQUIREMENT_CARD_TEMPLATE.md` 已存在，本卡片按其字段编写。
- 开发机基线：Windows 11（build 26300）、Rust 1.99.1、Python 3.13.13、Node 25.2.1、RTX 5090 D 32GB（sm_120，需 CUDA 12.8+）、F: 仅约 24GB 空闲、C: 约 617GB 空闲。
- 无历史代码/数据/接口兼容包袱；但模型清单、缓存布局、API 契约、adapter 格式从第一天起就要版本化。

核心缺陷一句话：**当前仓库没有任何可执行能力，且「推理快/省内存」与「微调→再推理」在开源方案里是割裂的**；本项目要补的是组合与治理，而不是再写一套 GEMM/Attention。

## 3. 目标与非目标

### 3.1 目标

- G1：一条命令从国内源拉取 GGUF 模型并启动 OpenAI 兼容本地服务。
- G2：P1 在 RTX 5090D 单卡上达到同机同 commit llama-server 的 95% 以上（服务层额外开销 ≤5%）；P0 只记录自建基线，不设硬性能门槛。
- G3：通过 Q4/iq4 量化、KV Cache q8_0、mmap、前缀复用与显存预估，把 8B 模型 32k 上下文的默认显存控制在约 9.2GB（f16 KV 参考值 ≈11.4GB），并在加载前阻止可预见的 OOM。
- G4：在 24GB 卡上完成 7B 级 QLoRA，在 32GB 卡上完成 14B 级 QLoRA；相对同配置 PEFT 主基线 ≥1.6× 吞吐，相对无 GC 基线 ≤60% 显存（口径见 §13.3）。
- G5：训练产物（PEFT adapter）可导出、可被运行时加载，形成「下载 → 运行 → 微调 → 导出 → 再运行」闭环。
- G6：Windows 11（build 26300）与 Linux 可安装、可复现构建；提供 CLI、本地 API、P2b 桌面 GUI。

### 3.2 非目标

- N1：不做预训练、全参数大模型训练、多机分布式训练（P3 仅评估单机多卡 FSDP）。
- N2：P0/P1 不自研 Attention/GEMM 内核、不自研 PagedAttention、不 fork llama.cpp。
- N3：不做多租户、计费、公网 SaaS、模型市场运营。
- N4：不承诺 16+ 高并发数据中心吞吐；单机目标 1–8 并发。
- N5：不做移动端；不代理闭源 API。
- N6：不复制 Unsloth 代码；不引入 AGPL 依赖（Unsloth Studio UI 为 AGPL-3.0，仅作产品参考）。
- N7：Metal/ROCm、国产加速卡、speculative decoding、embedding/rerank 不纳入 v1.0；接口可预留，实现放 post-v1.0。

## 4. 口径映射（C1–C12）

| 约束 | 设计落点 |
|---|---|
| C1 Rust + Python | §7.1 进程边界、§7.2 crate 分层、§8.3 train bridge |
| C2 高性能运行 | §7.3 执行后端、§7.4 调度与缓存、§13.1 门槛 |
| C3 运行快 | §13.1 TTFT / decode / 并发聚合指标 |
| C4 省内存 | §7.4 显存治理、§7.5 量化、§13.2 预算 |
| C5 参考 Unsloth | §7.6 训练栈与内核策略、§13.3 训练门槛 |
| C6 微调框架 | §7.6、§8.3 IPC、§11 测试 |
| C7 国内源下载 | §7.5 源优先级/fallback/校验/CAS |
| C8 下载提速 | §7.5 分片并发、§13.4 指标 |
| C9 运行+微调闭环 | §7.7 产物契约与热加载 |
| C10 本地与安全 | §10 安全基线 |
| C11 平台与后端 | §7.3 硬件分级 |
| C12 可维护 | §7.2、§11、§12 分期 |

## 5. 不变量

- I1：Rust 进程不 import/嵌入 PyTorch；Python worker 不监听公网端口，只通过 stdio RPC 与父进程通信。
- I2：模型权重与缓存对运行时只读；所有下载内容必须落盘后校验 SHA256，再原子改名。
- I3：只接受 safetensors / GGUF / JSON；禁止 pickle、`torch.load(weights_only=False)`、路径穿越与绝对路径模型文件。
- I4：本地服务默认只监听 `127.0.0.1`，默认启用本地 token；远程绑定必须显式开启并二次确认。
- I5：每个 decode step 只有一个 batch 写者；取消在 token 边界生效，取消后必须释放 slot 与 KV。
- I12：默认无遥测；指标与日志只落本地，外发必须显式开启并说明内容。
- I6：显存不足必须在 load 前失败，并给出「降 ctx / 换 IQ4_XS / 降 ngl / 减少 slot」的可执行建议；禁止 OOM 后静默降级。
- I7：adapter 切换是原子的；加载失败保留旧 adapter，绝不让请求落到半加载状态。
- I8：prefix cache 的键必须包含模型指纹、chat template 版本、量化类型、adapter id；任一变化即失效。
- I9：训练与推理共用一张 GPU 时必须通过 GPU lease 串行化；不得在未声明的情况下争抢显存。
- I10：所有性能/精度声明必须附「硬件 + 版本 + 参数 + 原始数据」四元组；无实测的数字标「估算」。
- I11：量化与导出参数写入 manifest，保证可复现；模型许可随 manifest 展示，不改变模型许可。

## 6. 总体架构

```text
┌────────────────────────────── llmgui binary (Rust, 单进程) ──────────────────────────────┐
│  CLI (clap)          HTTP/SSE (axum, OpenAI 兼容)         Admin API        Tauri GUI (P2b)│
│      │                         │                              │                         │
│      └──────────────┬──────────┴──────────────┬───────────────┘                         │
│                     ▼                         ▼                                          │
│            ┌─────────────────┐        ┌──────────────────┐                               │
│            │ llmgui-scheduler│        │ llmgui-download   │──► CAS / manifest / SQLite   │
│            │ 准入/队列/取消   │        │ 分片/续传/校验    │                               │
│            │ prefix cache 映射│        │ 测速/fallback     │                               │
│            └────────┬────────┘        └──────────────────┘                               │
│                     ▼                                                                   │
│            ┌─────────────────┐        ┌──────────────────┐                               │
│            │ backend-api     │◄───────│ train-bridge      │◄── stdio NDJSON ──┐          │
│            │ Engine trait    │        │ GPU lease / job   │                   │          │
│            └────────┬────────┘        └──────────────────┘                   │          │
│                     ▼                                                         │          │
│       ┌───────────────────────────┐                              ┌────────────┴───────┐  │
│       │ backend-llamacpp (FFI)    │                              │ Python worker      │  │
│       │ CUDA / CPU / (Metal/ROCm) │                              │ PyTorch/PEFT/TRL  │  │
│       └───────────────────────────┘                              │ 训练 / 导出 / 评测 │  │
│                                                                  └────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────────────────────┘
```

三条数据流：

1. **获取**：源解析 → 分片下载 → 流式 SHA256 → CAS → manifest/SQLite 注册 → 硬链接为模型目录。
2. **推理**：HTTP 请求 → 准入/队列 → slot 与 prefix cache → llama.cpp decode → SSE token 流 → 指标。
3. **训练**：CLI/API 创建 job → GPU lease → Python worker 训练 → adapter + manifest → 注册 → 运行时热加载。

## 7. 关键设计

### 7.1 进程、线程与 GPU 资源

- **单 Rust 二进制**承载 CLI、HTTP 服务、调度、下载与训练编排；不把 Python 运行时嵌进 Rust（C1）。
- tokio runtime 只跑 HTTP/SSE/CLI/下载，`worker_threads = min(4, 物理核/2)`，避免与推理抢核。
- 每个已加载模型一个 **engine 线程**，独占 llama context；scheduler 只做队列与记账，不直接下发 kernel。
- **GPU lease**：单卡默认同一时刻只允许「服务」或「训练」一方持有 GPU；`llmgui train` 先申请 lease，服务侧收到通知后卸载/拒绝新请求（可配置等待），训练结束自动恢复。lease 由 `LLMGUI_HOME/gpu.lock` 文件锁（进程崩溃由 OS 释放）与本地 admin API 通知共同实现：若 `serve` 在跑，`train` 先请求 drain → unload，超时失败并给出提示；训练完成后写 adapter 并通知 `serve` reload。多卡时按 `--device` 把训练与推理钉在不同卡上。
- 与 Python 的边界：训练是独立进程；Rust 用 `std::process::Command` 拉起 `python -m llmgui_train.worker`，stdio 走 NDJSON/JSON-RPC；Python 侧不监听网络端口。v1 不引入 PyO3，避免 GIL、依赖冲突、PyTorch CUDA context 与崩溃域耦合。

### 7.2 Rust workspace 分层

```text
crates/
  llmgui-core/            配置、错误、请求/响应类型、取消、指标抽象
  llmgui-models/          GGUF metadata、指纹、量化识别、显存预估、清单
  llmgui-backend-api/     Engine trait（load/generate/cancel/stats），不泄漏 FFI 类型
  llmgui-backend-llamacpp/ 唯一允许 unsafe/FFI 的边界；封装 llama-cpp-2 + dynamic-backends
  llmgui-scheduler/       slot 池、准入、公平、取消、超时、背压、prefix cache 映射
  llmgui-server/          axum：OpenAI 兼容、SSE、鉴权、限流、metrics
  llmgui-download/        国内源适配、分片并发、续传、SHA256、CAS、测速
  llmgui-train-bridge/    Python worker 生命周期、协议、GPU lease、adapter 注册
  llmgui-bench/           TTFT/TPOT/吞吐/显存采集与报告
  llmgui-cli/             pull/run/serve/estimate/bench/adapter/train
  llmgui-desktop/         Tauri 壳（P2b，不在 P0 crate 依赖内）
```

硬规则：FFI 类型不出 `llmgui-backend-llamacpp`；`Engine` trait 只暴露 owned token id 流与统计；scheduler 不感知具体后端类型，为 post-v1.0 可替换执行层保留边界。

### 7.3 执行后端与硬件分级

**决策：P0/P1 以 llama.cpp 为唯一执行后端，进程内 FFI（`llama-cpp-2`，锁定 `llama-cpp-sys-2` 版本），不自研算子、不 fork、不做 sidecar HTTP。**

理由：llama.cpp 原生支持 GGUF、Q4/IQ 量化、KV 量化、mmap 与多后端（CUDA/HIP/Metal/Vulkan/SYCL/CANN/CPU），MIT 许可；自研执行层无法在 26 周内追平其内核与量化积累。Rust 的价值在调度、服务、显存治理、下载与训练闭环，不在 GEMM。

**能力边界（关键）**：`llama-cpp-2` 走的是 **libllama**，只提供模型加载、context、`llama_batch`/`llama_decode`、sampler、`llama_state_seq_*` 与 KV 类型等原语；`-cb`、`-np`、`-ub`、`--cache-ram`、`--ctx-checkpoints`、`--slot-save-path`、`timings.cache_n` 都是 `tools/server`（llama-server）的实现，**不能假定在进程内直接可用**。因此：

- P0-3 只做**单序列**加载与流式生成，验证 FFI 与输出正确性；
- P1-1 由 Rust 实现 slot 池、每步 batch 拼装（prefill chunk + decode token）、序列状态存取、取消/超时/公平与背压；
- P1-2 由 Rust 实现 prefix cache（基于 `llama_state_seq_get_data/set_data` 或等价 KV seq API 做序列状态快照与恢复），不依赖 server 的 prompt cache；
- 「相对 llama-server 额外开销 ≤5%」的分母定义为**同 commit、同 `-ngl`/KV 类型/ubatch 的 llama-server**，且只在 P1 以同机同测量方法对比；P0 只记录自建基线。

- 后端使用 `dynamic-backends`（dlopen），交付「单二进制 + 按需后端动态库」，降低 Windows 打包复杂度；后端缺失必须显式诊断，禁止静默退回 CPU。
- **upstream 保底后端**：当 in-process FFI 在目标环境不可用时，`llmgui-backend-upstream` 通过 OpenAI 兼容 `/v1/completions` 调用本地 `llama-server`（可运行在 WSL/Linux），复用同一 `Backend` trait、调度、API 与 GUI；需显式 `--backend upstream` + `LLMGUI_UPSTREAM_URL`，不改变默认路径。
- v1.0 不实现原生 Rust 执行层；P1 指标达标后，若需要 PagedAttention / 跨请求 chunked prefill / 国产卡深度适配，另立 ADR 评估 mistral.rs / candle，并以同一基准对比。不得在 P1 期间分叉实现。

硬件分级：

| 级别 | 平台 | P0/P1 承诺 | 说明 |
|---|---|---|---|
| T1 | NVIDIA CUDA（sm_80+，含 4090/5090D） | 全功能、性能门槛 | Windows 11（build 26300）+ Linux；CUDA 12.8+；sm_120 需验证内核覆盖 |
| T1 | x86 CPU（AVX2） | 可运行、低吞吐 | 作为兜底与 CI；不设性能门槛 |
| T2 | Apple Metal、AMD ROCm | 可运行、单独标定 | **post-v1.0**，v1.0 只做构建验证 |
| T3 | 昇腾 CANN、摩尔线程 MUSA、沐曦 MACA、寒武纪 MLU | 接口预留/实验 | **post-v1.0**；训练侧 L3 只承诺 bf16 LoRA（见 §7.6） |

> Windows 10 仅 best-effort：bnb QLoRA 官方要求 Win11/Server 2022+，Win10 只承诺 bf16 LoRA；文档需明确支持矩阵。

### 7.4 调度、缓存与显存治理

- **slot 池与批拼装（Rust 实现）**：Rust 维护逻辑 slot（`seq_id` 映射、prompt 进度、采样状态、取消标记）；每个 decode step 组装一个 `llama_batch`，包含各活跃序列的 1 个 decode token 与待推进的 prefill chunk；调用 `llama_decode` 后按 seq 采样并流式返回。默认 slot 数由显存档位与上下文长度推导；请求进入有界队列，支持超时、取消、429 背压；SSE 断开即取消。
- **prefix cache（Rust 实现）**：对 `hash(模型指纹 + template 版本 + 量化类型 + adapter id + 前缀 token ids)` 命中时，用 libllama 的 `llama_state_seq_get_data/set_data`（或等价 KV seq API；若绑定未暴露，则在本 crate 内补最小 FFI）把目标序列的 KV 状态恢复到空闲 slot，跳过该前缀的 prefill；状态快照按字节上限 LRU 存放，默认与模型缓存同盘。命中指标为自建 `prefix_cache_hits`/`prefix_cache_saved_tokens`，**不使用 server 的 `timings.cache_n`**。若该 API 的快照/恢复开销或体积不达标，P1 先只做「同一会话多轮 + 相同 system prompt」的 slot 内复用，跨会话 prefix cache 延后。
- **chunked prefill（Rust 实现）**：P1 在 batch 组装层实现「prefill chunk + decode 混合批」，用请求级 token 预算控制 TTFT 与 TPOT；是否交织由调度策略决定，不依赖 server 的 `-ub`。
- **PagedAttention**：P0/P1 不做。llama.cpp 的 KV 是 unified/连续内存，不自带 block table；触发重启条件：单机并发 ≥16，或 KV 利用率长期 <70%；届时另立 ADR 评估原生后端专项，不纳入 v1.0。
- **KV cache**：默认 `q8_0`，`q4_0` 仅低显存档显式开启；长上下文 ppl 回归通过才允许改默认。
- **显存预估**：加载前用 GGUF metadata 估算权重 + KV + compute buffer + context 开销，误差目标 ≤5%；不足直接失败并给出建议（I6）。
- **mmap**：默认 mmap 权重；Windows 下顺序预读 + 首请求 warmup；不默认 `mlock`，避免 commit 打满。
- **模型生命周期**：默认单大模型常驻；支持 unload / hot-swap；切换前 drain 队列；加载失败回滚旧模型。

### 7.5 模型获取、缓存与加速

**源优先级**（可配置 `source_order`）：

1. **ModelScope**（默认）：国内发布者首发，列表 API 返回每文件 `Size`/`Sha256`，最适合校验。
2. **hf-mirror.com**（HF 兼容）：社区 GGUF 覆盖最全；当 ModelScope 无同名仓库/目标量化文件时前置。
3. **Gitee AI / 模力方舟**（HF 兼容）：作为第三源，复用同一个 HF 适配器，不写专用适配器。
4. WiseModel / OpenI 不进自动 fallback（登录/协议/人机校验），只支持用户显式提供已克隆目录。

**实现策略**：

- Rust 实现 ModelScope REST 适配器与 HF 兼容适配器；HF Xet、gated 授权、git-lfs 仓库在 P1 起用 Python 子进程兜底，不在 Rust 重写 LFS/Xet 协议。
- 文件级跨源：同一文件的所有分片必须来自同一 URL；跨源复用只认 SHA256，不做分片混源。
- 分片：单文件默认 8 片（4–16 可配），片大小 8–32MB 自适应；每 host 连接上限默认 8，文件并发 2–4；服务端不支持 Range 时降级单流。
- 续传：`.part` + `state.json`（ETag/Last-Modified/size/分片位图）；ETag 变化整文件重下；完成后 `fsync` + 原子 rename。
- 校验：流式增量 SHA256；失败删除 `.part` 整文件重试 1 次，再失败冷却该源 10 分钟。
- 测速与 fallback：1MiB Range 探测 3 次取 p50，按 host 缓存 30 分钟；404/403、连续 5xx、连接/首包超时、60s 窗口 <50KB/s、429（尊重 Retry-After）、SHA256 不符触发切换；同源连续失败 2 次冷却。
- 缓存：内容寻址 `blobs/sha256/<ab>/<hex>` + manifest + `snapshots/` 硬链接（Windows NTFS 支持；跨卷退化为复制）。默认根目录 `%LOCALAPPDATA%\llmgui`（Linux `$XDG_DATA_HOME/llmgui`），可用 `LLMGUI_HOME` / `--cache-dir` 覆盖；下载前按 manifest 汇总大小做空间预检（×1.1）。
- 安全：仅 https、限制重定向、拒绝路径穿越；只接受 `.gguf`/`.safetensors`/配置小文件，拒绝 `.bin`/pickle。
- 离线：`--offline` 只读 CAS + 校验 manifest；失败直接报错，不联网。

### 7.6 Python 微调层

**基座**：Python 3.11–3.13、torch 2.9.x、Transformers 5.x、PEFT、TRL、Accelerate、datasets；Linux/Windows CUDA 用 bitsandbytes NF4 QLoRA；Liger-Kernel（BSD-2）提供融合 RMSNorm/RoPE/SwiGLU/FusedLinearCE 与 chunked DPO loss。

**与 Unsloth 的关系**：清洁室实现其公开思路，不 fork、不复制代码。P2a 直接获得同类收益的路径是：

1. 4-bit NF4 + double quant QLoRA，LoRA 打 all-linear；
2. 序列 packing + 正确 varlen/块对角注意力，padding-free；
3. gradient checkpointing + 8-bit optimizer + 分块 CrossEntropy（避免 `B×S×V` logits）；
4. Liger 融合内核 + SDPA；P2a **不自研** Triton/autograd/attention。

| Unsloth 公开机制 | 本项目对应实现 | 阶段 |
|---|---|---|
| 4-bit QLoRA / NF4 double quant | bnb NF4 + PEFT + bf16 compute；非 CUDA 降级 bf16 LoRA / HQQ | P2a |
| 融合 CrossEntropy / RoPE / RMSNorm / SwiGLU | Liger patching；profile 触发才补 1–2 个自研内核 | P2a（Liger）/ P3（自研） |
| padding-free + 序列 packing | TRL `packing=True` + 正确 varlen/块对角 attention；打包效率 ≥0.95 | P2a |
| gradient checkpointing 变体 | HF full GC 起步；P3 A/B selective/offload | P2a / P3 |
| 动态 4-bit（敏感层保精度） | P2a 标准 NF4；P3 再评估分层混合精度 | P3 |
| MoE Split LoRA | P2a 不支持 MoE 4-bit QLoRA；仅记录为后续项 | post-v1.0 |

**自研内核门槛**：仅当 profile 证明「融合相关开销 >15% step time」或「logits 中间张量 >1.5GB」且 Liger 未覆盖时，P3 才补 1–2 个内核，优先 fused linear CE 与 fused RMSNorm+residual；必须带 native 回退、`allclose` 单测、收敛对齐，否则删除。Windows 首发保证 native PyTorch 路径；`triton-windows` 作为可选加速 extra，不阻塞发布。

**训练能力**：P2a SFT LoRA/QLoRA（JSONL/Alpaca/ShareGPT、chat template、assistant-only loss mask、断点续训）；P2a DPO（若延期则在 P3 完成，不阻塞 P2a 退出）；单机多卡 FSDP 为 P3 二选一/post-v1.0。默认 `r=16, alpha=32, dropout=0.05, target_modules=all-linear`，`packing=True`，`gradient_checkpointing=True`。

**数据与隐私**：训练数据默认只读本地文件；示例数据可从 ModelScope datasets 获取并记录来源/许可/指纹；用户数据不经任何网络上传，日志不打印样本正文。数据格式转换、去重、长度过滤与 token 统计在 Python 侧完成，Rust 只透传路径与 manifest。

**平台能力探测**：启动时检测 `torch.cuda` / `torch_npu` / `torch_musa` / `torch.mlu`、Triton 可导入性、bnb 后端，输出 `capabilities`，据此选择 `{quant: bnb|hqq|none} × {kernels: liger|native} × {attn: sdpa|flash}`；不支持的组合显式报错，禁止静默降级。T3 只承诺 bf16 LoRA + Liger，bnb 4-bit QLoRA 标实验。

### 7.7 产物契约与微调闭环

| 产物 | 格式/位置 | 权威性 | 消费方 |
|---|---|---|---|
| PEFT adapter | `adapter_model.safetensors` + `adapter_config.json` + `manifest.json` | **权威** | Python 续训、Rust 注册/导出 |
| merged 模型 | bf16 safetensors 独立目录 | 派生 | 二次量化、GGUF 转换 |
| GGUF / GGUF LoRA | `*.gguf` | 派生 | llama.cpp 运行时 |
| 训练 manifest | JSON（schema 版本、base id/revision、数据指纹、超参、指标、环境、sha256） | 权威元数据 | Rust 校验、GUI 展示 |

- adapter 元数据必须记录 base model id + revision、r/alpha/dropout/target_modules、量化方案；运行时按 `y = Wx + (alpha/r)·B(Ax)` 施加，形状或 base 不匹配即拒绝。
- 转换流水线：Python 侧调用锁定版本的 llama.cpp 工具——`convert_hf_to_gguf.py`（merged）、`convert_lora_to_gguf.py`（adapter）、`llama-quantize`（Q4_K_M，可选 imatrix）；工具版本、量化参数与输入 sha256 写入 manifest，保证可复现。
- 热加载：P1 支持已转换的 GGUF LoRA 原子切换（per-request scale）；P2a 打通 Python 导出 → GGUF LoRA/merged → 运行时加载全链路。
- 禁止 pickle；导出写入临时目录后原子 rename；失败保留旧 adapter。

### 7.8 入口：CLI / API / GUI

P0 先做 CLI + OpenAI 兼容 API：

```text
llmgui pull Qwen/Qwen2.5-7B-Instruct-GGUF --quant Q4_K_M
llmgui serve --model qwen2.5-7b-q4km --ctx 32768
llmgui run --model ... --prompt "..."
llmgui estimate --model ... --ctx 32768 --slots 4
llmgui bench --model ... --concurrency 1,4
llmgui adapter load ./adapter --name my-lora
llmgui train --config train.yaml
```

API（默认 `127.0.0.1:11435`，Bearer token）：

| 方法 | 路径 | 阶段 | 说明 |
|---|---|---|---|
| GET | `/healthz` | P0 | 存活/模型状态 |
| GET | `/v1/models` | P0 | 已加载/已注册模型 |
| POST | `/v1/chat/completions` | P0 | OpenAI 兼容，SSE 流式 |
| POST | `/v1/completions` | P0 | 兼容旧客户端 |
| GET | `/metrics` | P1 | Prometheus 指标 |
| POST | `/admin/models/load` | P1 | 加载/切换 |
| POST | `/admin/adapters/load` | P1 | 热加载 adapter |
| POST | `/v1/embeddings` | P3 | 可选 |

错误统一为 OpenAI 风格 `{"error":{"message","type","code"}}`；未知字段宽松忽略，响应字段只增不改。远程绑定必须 `--allow-remote` 且配置 token，默认关闭 CORS。

P2b 桌面 GUI：Tauri 2 + React/TS，页面为模型库/下载进度、聊天、训练配置与曲线、adapter 管理、基准与设置；业务逻辑只调用本地 API，不在 GUI 重复实现。

**边界与例外**：

- P0/P1 推理只支持 GGUF；AWQ/GPTQ/safetensors 直接推理不承诺（post-v1.0 原生后端再评估）。
- 视觉/音频/多模态、工具调用、结构化输出 P0 不承诺；JSON schema/GBNF 作为 P1 可选项。
- 并发承诺 1–8；超过预算时排队/429，不保证 16+ 吞吐。
- 模型族以 llama.cpp 当版支持为准；新架构（如 MoE）需先验证 GGUF 加载与量化覆盖。
- Windows 10 为 best-effort；Triton/Liger 属可选路径，不是发布阻塞项。

**校验与判定入口**：`llmgui verify` 校验 CAS 与 manifest 哈希；`llmgui estimate` 判定显存可行性；加载器校验 architecture/quant/adapter 形状；Python `capabilities` 判定量化/内核/注意力组合是否受支持。以上任一项失败都必须显式报错并给出替代方案，不允许静默降级。

### 7.9 配置、状态与目录

```text
<LLMGUI_HOME>/                 默认 %LOCALAPPDATA%\llmgui（Linux $XDG_DATA_HOME/llmgui）
  config.toml                  服务/下载/训练默认配置
  state.db                     SQLite（WAL）：models / adapters / jobs / benchmarks / settings
  cache/blobs/sha256/…         内容寻址块
  cache/refs/…                 源 manifest 快照
  cache/snapshots/…            硬链接模型目录
  models/<id>/                 对外模型路径（manifest + 硬链接/复制）
  adapters/<name>/             PEFT adapter + manifest
  jobs/<job-id>/               训练日志、checkpoint、导出产物
  logs/                        tracing JSON 日志
```

配置形态 TOML；密钥不落明文配置，使用 Windows Credential Manager/DPAPI（Linux secret-service 可选），至少支持环境变量注入并日志脱敏。

### 7.10 观测与基准

- 指标：请求数、token 数、TTFT/TPOT 直方图、decode/prefill tok/s、队列深度、slot 占用、prefix cache 命中、KV 使用、GPU 显存、下载速率与源错误。
- 日志：结构化 JSON，默认 info，支持 `--log-level`；请求 id 贯穿 API → scheduler → engine。
- 基准：固定 prompt 集（128/512/1024/4096 token × 20 条，固定 seed）、warmup 3 次、每配置 5 次取中位数、GPU 固定 power limit、NVML 峰值；对照同 commit 的 `llama-server`。原始 JSON 归档，CI 只做阈值回归。
- 精度：Q4_K_M 相对 F16 的 wikitext-2 ppl 增幅 ≤3%；训练前后固定 eval loss 偏差 ≤2%。

## 8. 数据与接口契约

### 8.1 模型 manifest（示例）

```json
{
  "schema": 1,
  "id": "qwen2.5-7b-instruct-q4km",
  "source": {"type": "modelscope", "repo": "Qwen/Qwen2.5-7B-Instruct-GGUF", "revision": "master"},
  "architecture": "qwen2",
  "quant": {"format": "gguf", "type": "Q4_K_M", "bits_per_weight": 4.89},
  "context": {"train": 32768, "max": 32768},
  "files": [{"path": "qwen2.5-7b-instruct-q4_k_m-00001-of-00002.gguf", "size": 3993201344, "sha256": "…"}],
  "license": "Apache-2.0",
  "chat_template": {"source": "tokenizer_config.json", "version": 1},
  "created_at": "2026-11-02T00:00:00Z"
}
```

### 8.2 训练配置（示例）

```yaml
base_model: Qwen/Qwen2.5-7B-Instruct
method: qlora
load_in_4bit: true
lora: {r: 16, alpha: 32, dropout: 0.05, target_modules: all-linear}
seq_len: 2048
micro_batch: 1
grad_accum: 16
packing: true
gradient_checkpointing: true
optim: adamw_8bit
dataset: {path: data/sft.jsonl, format: sharegpt}
output_dir: jobs/my-sft
```

### 8.3 训练 IPC（v1）

Rust → Python：`hello/hello_ok`、`estimate`、`train.start`、`train.cancel`、`export.merge`、`export.gguf`；Python → Rust：`event.progress`、`event.checkpoint`、`event.log`、`done`、`error`。每行一个 JSON 对象，带协议版本与请求 id；一次性 token 通过环境变量传入；Rust 父进程退出时杀掉子进程组，worker 检测 stdin EOF 自行退出。

## 9. 兼容与迁移

- greenfield，无存量数据迁移；从 P0 起固定 `manifest schema=1`、SQLite migration、cache layout v1。
- API 采用 semver：1.0 前允许新增字段，不删除/改义；`/v1` 路径冻结。
- 模型/量化/lib 版本写入 manifest 与 benchmark 报告；升级 llama.cpp 或训练依赖必须跑 golden 输出与 ppl 回归。
- 缓存迁移：P0 只支持 `--cache-dir` 指定；P1 提供 `llmgui cache move`（复制 + 校验 + 切 manifest），不做静默迁移。
- 文档骨架：P0 先裁剪 cndb 遗留文档，`docs/adr/TEMPLATE.md`/`docs/adr/README.md` 已由本方案建立；P0-1b 再补 Makefile、`scripts/check_adr_landing.py`、双 CI，并把 `AI_DRIVEN_DEV_FLOW.md` 改写成本项目版本。

## 10. 安全与合规

| 威胁 | 控制 |
|---|---|
| 恶意模型文件/GGUF 解析 | 只读 mmap + 边界检查；GGUF parser fuzz；`unsafe` 仅限 FFI crate |
| 路径穿越/符号链接 | 规范化路径并限制在 cache 根内；拒绝绝对路径与 `..`；原子 rename |
| 反序列化 RCE | 禁 pickle / `torch.load` 非 weights_only；只接受 safetensors/GGUF/JSON |
| 下载投毒/中间人 | 强制 https、SHA256 全量校验、manifest 固定 revision、重定向限制；私有仓库不自动切第三方镜像 |
| 本地 API 暴露 | 默认 127.0.0.1 + Bearer token；远程需显式开关；默认关 CORS |
| 凭据泄露 | token 存 Credential Manager/DPAPI；日志脱敏；不把 HF token 隐式发给镜像 |
| 供应链 | `cargo-deny` + `cargo-audit` + `pip-audit`；锁文件；SBOM；禁止 AGPL 依赖 |
| 许可 | 软件 Apache-2.0；模型许可随 manifest 展示，不重分发权重；Unsloth 只清洁室参考 |
| 遥测/隐私 | 默认无遥测；日志与指标只落本地；外发需显式 opt-in 并说明字段（I12） |
| 生成式 AI 合规 | 本地工具本身不构成对外生成式 AI 服务；若未来提供托管/在线分发，需评估备案、内容标识与使用者协议 |

## 11. 测试与质量门禁

### 11.1 分层

- Rust 单测：manifest/配置解析、源选择与 fallback、分片规划、SHA256 流式校验、batch 拼装、prefix key/LRU、seq state 快照/恢复、显存估算、chat template、错误映射。
- Rust 属性/模糊：GGUF parser（cargo-fuzz）、range 续传状态机、路径安全。
- 集成：本地 HTTP fixture 模拟 Range/断连/429/哈希错误；OpenAI API 契约（流式/非流式/取消/超时）；adapter 原子切换；4 并发压测与取消释放。
- Python 单测：数据格式/掩码/packing、量化加载、LoRA 注入、导出往返、resume、能力探测；内核 `allclose`（如启用）。
- Python GPU：tiny model（0.5B/1B）SFT 10–50 step、显存峰值、adapter 导出→加载→生成；nightly 在 5090D 上跑，4090 仅在租用/采购后加入。
- E2E：本地 fixture 拉取 tiny GGUF → serve → chat → SFT → adapter 导出 → 热加载 → 对比输出。
- 性能：`make bench-runtime`（固定 prompt 集）与 `make bench-download`（≥1GB 同源文件、3× 单线程、50% 中断续传 SHA 一致）；报告归档 `benchmarks/reports/<date>/`，nightly 与基线对比，波动 >5% 需解释或修复。
- 精度：固定 eval 集 100 条 + wikitext-2 切片；训练前后 loss/ppl 报告入 manifest。
- 安全：路径穿越、恶意 GGUF、未授权访问、pickle 拒绝、token 泄露扫描、遥测默认关闭检查。

### 11.2 门禁

- `cargo fmt --check`、`cargo clippy -D warnings`、`ruff`、类型检查（pyrefly/mypy 二选一）、`pytest`、`cargo test`。
- 覆盖率：Rust 核心 crate + Python 核心模块 ≥95%，排除 FFI 绑定、生成代码、CLI 参数壳；禁止注水。
- `make check-fast` / `make test` / `make flow` / `make check-docs`（含 `adr-check`、`gitkeep-check`）按 AGENTS 分级执行；P0 建立 Makefile 与 CI。
- 性能/精度门禁只在带 GPU 的 nightly/self-hosted runner 跑，失败阻断发版。

## 12. 分期计划

总周期约 26 周，按 6 人配置：2 名 Rust（运行时/平台）、1 名 Python 训练、1 名全栈/GUI、1 名 QA/性能、0.5 名构建/发布、0.5 名产品/设计。2–3 人团队按 P0→P1→P2a 核心裁剪，GUI 与 P3 延后；P2b 可滑入 P3。

### P0（第 1–5 周）：跑通「下载 → 运行」

| 编号 | 工作 | 交付/验收 |
|---|---|---|
| P0-1a | 裁剪 cndb 遗留文档：`docs/AI_DRIVEN_DEV_FLOW.md`、`BUNDLE_BUDGET.md`、`REFACTOR_BACKLOG.md` 中与本项目不符的段落；保留/改写 `REQUIREMENT_CARD_TEMPLATE.md`；本 ADR 已提供模板范式 | 文档不再引用 cndb 的 ADR 0004/plugin/fspack |
| P0-1b | 校验已建立的 `docs/adr/TEMPLATE.md`/`docs/adr/README.md`；新建 `Makefile`、`scripts/check_adr_landing.py`、`gitkeep-check`、双 CI（Windows/Linux）、LICENSE | `make check-docs`/`check-fast` 可跑；ADR 索引一致 |
| P0-2 | `llmgui-models` + `llmgui-download`：ModelScope/HF 兼容适配、分片续传、CAS、SHA256、空间预检、`make bench-download` fixture | fixture 测试；真实下载 Qwen2.5-7B-GGUF Q4_K_M（列表 API 实测 4.68GB）并通过 SHA256 |
| P0-3 | `llmgui-backend-llamacpp`：in-process FFI、单序列加载、chat template、流式生成 | 单模型单序列可跑；CPU/CUDA 均可启动 |
| P0-4 | `llmgui-server` + CLI：OpenAI chat/completions 流式、`/v1/models`、`pull/serve/run/list` | OpenAI 客户端可直连；token 鉴权 |
| P0-5 | `llmgui-bench` 初版 + `make bench-runtime`、Windows 预编译说明、P0 报告 | 固定 prompt 集可复现；记录自建基线；无 OOM |

**P0 退出标准**：按文档 `make build`（或使用预编译产物）→ `llmgui pull` → `llmgui serve` → `curl /v1/chat/completions` 成功；冷启动 ≤60s（已缓存模型）；记录基线但**不设硬性能门槛**；无阻断性缺陷。安装包留 P1-5，不在 P0 验收。

### P1（第 6–13 周）：快且省

| 编号 | 工作 | 交付/验收 |
|---|---|---|
| P1-1 | Rust 调度器：slot 池、每步 `llama_batch` 拼装（prefill chunk + decode）、取消/超时/公平/背压 | 4 并发压测；取消释放 slot 与 KV |
| P1-2 | Rust prefix cache（seq state 快照/恢复）、KV q8_0、显存预估、模型 unload/hot-swap | 32k ctx q8 峰值 ≤9.5GB；命中率指标 |
| P1-3 | 指标/日志、`/metrics`、bench 完整化、nightly 性能回归、4090 外部参照（租用或采购） | P1 性能门槛连续 3 晚全绿 |
| P1-4 | adapter 热加载（**预置转换好的** GGUF LoRA）、原子切换、GPU lease | 预置 adapter 切换测试；转换全链路留 P2a-3 |
| P1-5 | Windows 安装包/动态后端/诊断/签名脚本；P1 报告 | 干净机安装成功；后端缺失有诊断 |
| P1-6 | 崩溃恢复、日志脱敏、`llmgui verify`、安装/升级回滚演练 | kill -9 后可恢复；回滚可用 |

**P1 退出标准**：§13.1 的 P1 指标全部达标；相对同 commit llama-server 开销 ≤5%；4 并发聚合达标；训练/服务 GPU lease 生效；安装包可装可卸。

### P2a（第 14–19 周）：训练闭环

| 编号 | 工作 | 交付/验收 |
|---|---|---|
| P2a-1 | Python worker/协议/能力探测/训练配置；SFT LoRA/QLoRA；数据 pipeline、packing、GC、resume | 7B QLoRA 在 24GB 完成 1 epoch |
| P2a-2 | Liger 接入、显存预估、训练基准与 A1–A8 消融 | 主基线 ≥1.6× 吞吐；显存按 §13.3 双基线判定 |
| P2a-3 | adapter 导出、merge、GGUF 转换/量化（`convert_lora_to_gguf`、`llama-quantize`）；运行时加载全链路 | e2e：训练→导出→加载→生成 |
| P2a-4 | 固定 SFT 冒烟集、固定 eval 集与 ppl 评测（许可复核） | 每次训练可复现 eval 指标 |
| P2a-5 | 训练文档、示例配置、数据格式说明；P2a 报告 | 新用户 30 分钟内完成一次微调 |
| P2a-6 | DPO（若排期不足则顺延 P3，不阻塞 P2a 退出） | 小规模偏好数据跑通；产物仍为 PEFT adapter |

**P2a 退出标准**：一条命令完成「下载→训练→导出→热加载」；24GB 卡 7B QLoRA 达标；数据与 eval 可复现。

### P2b（第 20–22 周）：GUI

- Tauri 2 + React/TS MVP：模型库/下载、聊天、训练配置与曲线、Adapter、设置五页。
- GUI 只调用本地 API；主流程 e2e；教程与截图。
- 退出标准：三條主流程（拉模型→聊天、训练→看曲线、加载 adapter→对话）通过验收；可滑入 P3，不阻塞 v1.0 核心。

### P3（第 23–26 周）：硬化与发布

- DPO 与单机多卡 FSDP **二选一**（按用户反馈），另一项 post-v1.0。
- 安全审计、性能长稳、崩溃恢复、安装签名、SBOM、更新通道（opt-in）与回滚。
- v1.0 文档与发布候选；Metal/ROCm、国产卡、speculative、embedding 均 post-v1.0。
- 退出标准：连续 7 天稳定性测试；安全 checklist 全过；v1.0 RC。

## 13. 性能、显存与下载预算

### 13.1 推理目标（P1 门槛；P0 只记录基线）

基准条件：RTX 5090 D 32GB（验收机）、Windows 11、CUDA 12.8+，Qwen3-8B Q4_K_M（功能默认可用 Qwen2.5-7B Q4_K_M，见 §13.5），prompt 128/512/1024/4096 × 20，output 128，warmup 3，5 次中位数，NVML 峰值。RTX 4090 仅作外部参照（2026-10-22 单次测量：Qwen3-8B Q4_K_M decode 155.0 tok/s、prefill 2155.2 tok/s、TTFT 51ms），不是本机验收门槛。

| 指标 | P0（5090D） | P1（5090D） | 口径 |
|---|---|---|---|
| 单流 decode | 记录基线 | ≥230 tok/s，且 ≥同 commit llama-server 的 95% | 230 为按带宽估算，验收以同机对照为准 |
| prefill @1k | 记录基线 | ≥3,000 tok/s，且 ≥95% 对照 | 估算 |
| TTFT p95 @1k | 记录基线 | ≤550 ms | 含 tokenize + 首 token |
| 4 并发聚合 decode（每路 8k ctx） | 不设门槛 | ≥450 tok/s | 4 客户端同时长输出 |
| 相对 llama-server 额外开销 | 不适用 | ≤5% | 同 commit、同 `-ngl`/KV/ubatch，decode 与 prefill 分别测 |
| 8B Q4 32k **q8_0** KV 峰值显存 | 记录基线 | ≤9.5GB | NVML 峰值；f16 参考 ≈11.4GB，不作为门槛 |
| 冷启动到 `/health`（已缓存） | ≤60s | ≤30s | 进程启动到 200 |
| Q4_K_M 相对 F16 ppl 增幅 | ≤3% | ≤3% | wikitext-2 固定切片，固定 seed |

- 4090 列如需门槛，必须租用/采购同型号机器并写入 CI；否则只作为外部量级参照。
- CPU/Metal/ROCm 只做功能验收，不设性能门槛（Metal/ROCm 为 post-v1.0）。
- 所有「估算」数字在报告中保留原始 JSON；P1 达标判定以同机 llama-server 比值为主。

### 13.2 显存预算（32k ctx，batch=1，估算）

统一换算为 GB（1GiB≈1.0737GB），四舍五入；compute buffer 1.0GB、context/tokenizer/运行时 0.6GB 已计入合计。权重为 Q4_K_M，KV 按典型 GQA 配置估算；实际以 GGUF metadata 计算为准。

| 模型 | 权重 | KV f16 | KV q8_0 | KV q4_0 | 合计 f16 | 合计 q8_0 | 合计 q4_0 |
|---|---|---|---|---|---|---|---|
| 8B（Qwen3-8B） | 5.0GB | 4.8GB | 2.6GB | 1.4GB | ≈11.4GB | **≈9.2GB** | ≈8.0GB |
| 14B | 9.0GB | 6.4GB | 3.2GB | 1.6GB | ≈17.0GB | ≈13.8GB | ≈12.2GB |
| 32B | 19.5GB | 8.6GB | 4.3GB | 2.2GB | ≈29.7GB | ≈25.4GB | ≈23.3GB |

结论：24GB 卡默认 q8_0 可跑 8B 4 路 8k；32GB 卡可上 14B Q4 + q8 KV；32B 在 32GB 上余量很小，建议 48GB 或降 ctx/量化，不作为 32GB 默认承诺。P1 的 8B 32k q8 门槛为 ≤9.5GB（±5%）。`llmgui estimate` 误差目标 ≤5%，加载前拦截 OOM。

### 13.3 训练目标

基线定义（必须同数据、同超参、同 seed）：

- **主基线 A（吞吐）**：同版本 PEFT/TRL + GC，关闭 packing、关闭 Liger；用于 ≥1.6× 吞吐判定。
- **基线 B（显存）**：同版本 PEFT/TRL，关闭 GC、packing、Liger；用于显存下降判定（大模型可能 OOM，需在 48GB 或更小 seq 上测）。
- Unsloth 公开的「2× / 70% less VRAM」作为参考，不作为验收数字；两者版本、模型与基线定义不同。

| 指标 | 门槛 |
|---|---|
| 7B QLoRA（r=16，s=2048，b=1，GC+packing） | 24GB 卡完成 1 epoch；峰值 ≤60% 基线 B；相对基线 A 吞吐 ≥1.6× |
| 14B QLoRA | 32GB 卡 1 epoch；24GB 卡需降 s/b 且不做产品承诺 |
| 吞吐 | ≥1.6× 基线 A（有效 token/s，3 次中位数） |
| 精度 | 固定 eval 集 loss 偏差 ≤2%；无 NaN/发散 |
| 打包效率 | 有效 token / 总 token ≥0.95 |
| 步时稳定性 | P95/P50 ≤1.2 |

**固定数据与评测**：P2a 选定一个许可清晰的公开 SFT 样例集（候选 UltraChat/OpenAssistant，需复核许可）作为冒烟集；固定 wikitext-2 切片做 ppl；从训练集外留出 100 条做固定 eval。数据集指纹、许可与切片方式写入 manifest，禁止用训练集自身做 eval。

### 13.4 下载目标

- 同源同文件（≥1GB、Q4_K_M 多分片）相对单线程 ≥3× 吞吐；尽可能接近链路带宽的 80%。
- 断点续传：50% 中断后恢复，最终 SHA256 一致。
- 源切换：首包超时/5xx/哈希失败在 10 秒内切换；失败原因可诊断。
- 7B Q4（4.68GB）在 500Mbps 链路目标 <2 分钟（估算，随源波动）。

### 13.5 默认模型清单 v0

| 用途 | 仓库 | 格式/量化 | 许可 | 备注 |
|---|---|---|---|---|
| 功能验收 | `Qwen/Qwen2.5-7B-Instruct-GGUF` | Q4_K_M（2 分片 4.68GB） | Apache-2.0 | 2026-11-02 列表 API 实测三源存在；P0 完成真实下载验收 |
| 性能基准 | `Qwen/Qwen3-8B` GGUF | Q4_K_M | Apache-2.0（上线前复核） | 用于 §13.1 对标 |
| 极省内存 | 同上 Qwen2.5 | IQ4_XS（需 imatrix） | Apache-2.0 | 显式选项，不设默认 |
| 训练底座 | `Qwen/Qwen2.5-7B-Instruct` | BF16 safetensors ~15.2GB | Apache-2.0 | QLoRA/SFT |
| 冒烟/CI | `Qwen/Qwen2.5-0.5B-Instruct` | safetensors ~1GB | Apache-2.0 | CPU 可跑 |
| MIT 备选 | `zai-org/GLM-4-9B-0414` | BF16 / GGUF | MIT | 需复核量化文件 |
| 不列默认 | InternLM2.5 / Baichuan2 | — | `license:other` / 自定义 | 需人工审核 |

> 本表只作 v0 口径；P0 将其下沉为 `models/v0.toml`（单一数据源），manifest 与 GUI 从该文件读取，避免 ADR 与实现漂移。

## 14. 风险与缓解

| # | 风险 | 影响 | 缓解 |
|---|---|---|---|
| 1 | `llama-cpp-2` 0.1.x 绑定漂移 | 升级 break | 锁 `llama-cpp-sys-2` 版本；FFI 隔离单 crate；每版 golden 输出对比 |
| 2 | Windows 构建（MSVC/CMake/CUDA 12.8+/sm_120） | 交付阻塞 | 预编译 + dynamic-backends；CI 缓存；工具链文档化 |
| 3 | Triton/Liger 在 Windows 半官方 | 训练加速不可用 | 默认 native SDPA 路径；`triton-windows` 可选 extra；能力探测 |
| 4 | bnb 仅 Win11+ 官方支持 | Win10 QLoRA 不可用 | 启动检测、明确报错、bf16 LoRA 兜底；文档支持矩阵；平台承诺统一为 Win11 |
| 5 | ModelScope 列表截断/接口漂移 | 漏文件或下载失败 | 条目自检/重枚举；fixture 测试；锁客户端版本；fallback |
| 6 | 公益镜像无 SLA | 下载失败 | 多源 fallback + 测速 + 冷却；用户可配源 |
| 7 | 显存预估不准/OOM | 体验崩塌 | 保守常数、load 前试算、失败建议、默认 q8 KV |
| 8 | 高并发预期错配（对标 vLLM） | 指标不达预期 | 对外承诺 1–8 并发；≥16 才立 PagedAttention 项 |
| 9 | 训练/推理抢同一张卡 | OOM/卡死 | GPU lease 串行化；多卡钉卡；GUI 明示 |
| 10 | 自研内核数值/收敛问题 | 训练事故 | 默认不自研；P3 才做且带回退/allclose/收敛门禁 |
| 11 | 模型许可/合规 | 法务风险 | 默认只列 Apache/MIT；manifest 展示；不重分发权重 |
| 12 | 范围蔓延 | 延期 | 严格 P0/P1/P2a/P2b 门禁；GUI/多卡/国产卡后置；P3 只保留一项扩展 |
| 13 | 无 4090 验收机 | 4090 门槛不可验 | P1 租用/采购 4090；否则只作外部参照，验收以 5090D + 同机 llama-server 比值 |
| 14 | Rust 调度器/prefix cache 被低估 | P1 延期 | P1 给 8 周；P0 只做单序列；每两周用 4 并发压测校准 |
| 15 | cndb 遗留文档污染流程 | 门禁不可执行 | P0-1a/1b 先裁剪并建 Makefile/模板；本 ADR 暂作模板范式 |

## 15. 决策记录

| 编号 | 议题 | 结论 | 依据 | 被否决项 | 落点 |
|---|---|---|---|---|---|
| D1 | 总体架构 | Rust 运行时 + 独立 Python 训练进程 | 性能/依赖/崩溃隔离；C1 | 全 Python、全 Rust、PyO3 单进程 | §7.1 |
| D2 | 执行后端 | P0/P1 llama.cpp in-process FFI，锁版本 | 成熟度、GGUF/量化/多后端、MIT | 自研算子、TensorRT-LLM、ort、sidecar HTTP | §7.3 |
| D3 | 调度与 PagedAttention | Rust 自实现 slot/批拼装/prefix cache；PagedAttention v1.0 不做，≥16 并发或 KV 利用率 <70% 时另立 ADR | 单机 1–8 并发；避免重写 kernel | 自研 PagedAttention、跨请求 chunked prefill | §7.4 |
| D4 | prefix cache | Rust 用 seq state 快照/恢复实现，hash 键含模型/模板/量化/adapter；P1 先做会话内复用，跨会话按 API 表现决定 | 复用 KV 状态；防错复用 | 依赖 llama-server prompt cache、仅按 system prompt 文本缓存 | §7.4 |
| D5 | 量化 | Q4_K_M 默认，IQ4_XS 二档，KV q8_0 默认，q4_0 显式 | 质量/体积/速度平衡 | AWQ/GPTQ v1.0、f16 默认、q2 默认 | §7.4/§13 |
| D6 | 下载 | ModelScope 主 + HF 兼容 fallback，Rust 下载器 + CAS，文件级跨源 | 国内可用性、哈希、性能 | git-lfs 全量、Python-only、分片混源 | §7.5 |
| D7 | 训练栈 | torch 2.9 + Transformers 5 + PEFT/TRL + Liger；训练层不自研内核 | 事实标准、风险可控、Unsloth 思路可复现 | fork Unsloth、首期自研 Triton、DeepSpeed-only | §7.6 |
| D8 | 平台承诺 | T1 CUDA/CPU 为 v1.0；T2 Metal/ROCm 与 T3 国产卡 post-v1.0，T3 只承诺 bf16 LoRA | 交付确定性 | 首发全平台、国产卡 bnb QLoRA 承诺 | §7.3/§7.6 |
| D9 | 产品入口 | CLI + OpenAI API P0；Tauri GUI P2b | 先闭环后体验 | Electron、Web-only、GUI 先行 | §7.8 |
| D10 | 产物契约 | PEFT safetensors adapter 权威；merged/GGUF 派生；禁 pickle | 可逆、安全、运行时兼容 | 自定义格式、pickle、只留 merged | §7.7 |
| D11 | 元数据 | SQLite + 文件 CAS + TOML | 查询/迁移/GUI 需要 | 纯 JSON、嵌入式 KV | §7.9 |
| D12 | 单卡资源 | 训练/推理 GPU lease 串行 | 避免 OOM 与互相拖慢 | 默认并发训练+推理 | §7.1 |
| D13 | API | OpenAI 兼容 + admin；默认回环 + token | 生态兼容、安全 | 私有协议、默认公网 | §7.8/§10 |
| D14 | 性能口径 | 固定 harness + 对照 llama-server + 原始数据 | 可复现、防营销数字 | 无基线宣称、只报峰值 | §13 |
| D15 | 许可 | 自有代码 Apache-2.0；清洁室参考 Unsloth；禁 AGPL | 商用友好、合规 | 复制 AGPL/Unsloth 代码 | §10 |
| D16 | Windows FFI 阻塞保底 | 新增 `upstream` 后端接本地 llama-server；in-process 仍为默认目标 | 目标环境 `llama_backend_init` 崩溃，真实推理需可交付路径 | 只保留 FFI、用 mock 冒充真实推理 | §7.3/§18 |

## 16. 后果

**正面**：以成熟执行层换取性能与量化能力；Rust 掌握生命周期、显存与取消，能做到低开销服务；Python 训练生态完整；国内下载与校验是一等公民；运行/微调闭环有明确产物契约。

**负面**：与 llama.cpp 强耦合，上游 API 变动需要跟进；双语言带来构建/发布复杂度；P0/P1 不支持高并发 PagedAttention；Windows Triton 不保证；单卡默认不能同时训练与推理。

**中性**：性能上限受 llama.cpp 内核限制；差异化主要来自治理、下载、闭环与体验，而非算子；`Engine` trait 为 post-v1.0 原生后端保留升级路径。

## 17. 与既有 ADR 的关系

无。本 ADR 为仓库首个 ADR；本方案已补 `docs/adr/TEMPLATE.md` 与 `docs/adr/README.md`，Makefile/门禁脚本/CI 仍由 P0-1b 建立。

## 18. 落地状态

| 期 | 状态 | 代码落点 | 测试 | 提交 |
|---|---|---|---|---|
| P0 下载→运行 | 部分落地 | `crates/llmgui-download`、`llmgui-server`、`llmgui-cli`、`llmgui-models`、`gui/`、`llmgui-backend-upstream` | `cargo test --workspace`；mock 与 upstream 真实推理冒烟通过（WSL llama-server）；真实 GGUF 下载 + `estimate` 通过；in-process FFI 仍受环境阻塞 | feat/llmgui-p0-p3 |
| P1 快且省 | 进行中 | `crates/llmgui-scheduler`（槽位/取消，3 个并发/取消测试）、`backend-llamacpp`（FFI 编译通过，exact-prefix `SeqState` 快照/恢复）、`backend-upstream`（`cache_prompt` 复用 llama-server 前缀缓存） | 调度/取消测试通过；in-process prefix cache 已实现但因 Windows FFI 阻塞未运行时验证；KV 量化门槛、4 并发压测未完成 | e027796 |
| P2a 训练闭环 | 部分落地 | `python/llmgui_train`（config/data/estimate/export/worker/trainer）、CLI `train` 桥 | Python tests 全绿；真实 CPU LoRA 与 QLoRA 4-bit（bitsandbytes CPU）各 1 step 跑通；adapter→native merge→GGUF f16→Q4_K_M→WSL llama-server 推理闭环已验证；7B/GPU 性能未验证 | a856002 |
| P2b GUI | 已落地 | `gui/index.html`、`styles.css`、`app.js` | 静态自检 + 无浏览器 DOM/SSE 集成测试；`llmgui serve` 托管 200 | feat/llmgui-p0-p3 |
| P3 硬化与发布 | 进行中 | Makefile、CI、ADR 门禁、`--start-wsl`、`doctor`、`install.ps1`、release 工作流 | `make check-docs`/clippy 通过；4 并发 upstream 288 tok/s；doctor 通过；签名/长稳未完成 | 3f25b65 |

> 真实 llama.cpp 运行：Windows 11 build 26300 + Intel Arrow Lake 上，`llama_backend_init()` 在 llama-cpp-2 0.1.159/0.1.160、自编译 C++ smoke 和预编译 llama-cli 上均触发 `STATUS_ACCESS_VIOLATION`；FFI 编译与模型下载/解析正常。已新增 `upstream` 后端，通过 WSL 中的 `llama-server` 完成真实 Qwen2.5-0.5B 推理，并走通 `llmgui serve` OpenAI API；此外完成真实 CPU LoRA/QLoRA 4-bit 训练、PEFT 原生 merge、GGUF 转换/量化，并用微调后的 GGUF 在 WSL 中推理。in-process FFI 仍需在 Linux/新工具链上修复。

## 19. 交接

- 当前阶段：P3 进行中（Rust 运行时 + 真实 upstream 推理 + Python 训练包 + Web GUI 可运行；in-process FFI 仍受 Windows 环境阻塞）。
- 已完成：需求卡片 0001；三份调研；ADR 模板/索引；Rust workspace（11 crate）；ModelScope/HF 下载器；GGUF 解析/显存预估；OpenAI 兼容服务/CLI/bench；`upstream` 后端；Python 训练 worker；Web GUI；Makefile/CI/文档门禁。
- 已验证：`cargo test --workspace` 全绿；Python 201 tests / 92.91%；`cargo clippy -D warnings` 通过；`make check-docs` 通过；ModelScope 下载 Qwen2.5-0.5B-GGUF 并 SHA256 校验；`llmgui serve --backend mock` + SSE + GUI 冒烟通过；WSL `llama-server` 提供 Qwen2.5-0.5B，`llmgui run/serve --backend upstream` 返回真实模型输出（单流约 140 tok/s，4 并发聚合 288 tok/s，CPU）；`--start-wsl` 可一条命令检测/拉起 WSL server。
- 下一步（按优先级）：
  1. 在 WSL/Linux 上跑通或修复 in-process FFI；`upstream` 后端已可作 Windows 保底路径；
  2. 实现 P1 的 prefix cache（seq state 快照/恢复）与逐步 batch 拼装，补 4 并发性能门槛；
  3. 安装 `torch/transformers/peft/trl/bitsandbytes` 跑通 7B QLoRA 与 adapter→GGUF 全链路；
  4. 安装脚本/release workflow 已就绪；剩余签名、Tauri 壳、崩溃恢复与性能长稳。
- 已知坑：
  1. 仓库现有 `docs/AI_DRIVEN_DEV_FLOW.md`、`BUNDLE_BUDGET.md`、`REFACTOR_BACKLOG.md` 是 cndb 遗留，部分链接不存在；
  2. Windows 上 llama.cpp 初始化崩溃（见 §18）；mock/upstream 后端不受影响；upstream 需要用户先启动 `llama-server`（可运行在 WSL）；
  3. `llama-cpp-2` 固定为 `=0.1.159`；`llama` feature 需要 MSVC + libclang（当前用 pip `libclang` + VS2022 vcvars）；
  4. Python 真实训练依赖未安装，worker 会返回结构化 `missing_dependency`；
  5. Tauri 壳未构建，GUI 以本地 Web 形式交付；单机训练/推理 GPU lease 仅完成设计。


## 20. 参考

内部：

- [需求卡片 0001](../requirements/0001-local-llm-runtime-finetune.md)
- [runtime 调研](../research/runtime.md)
- [finetune 调研](../research/finetune.md)
- [download 调研](../research/download.md)

外部（关键）：

- llama.cpp：https://github.com/ggml-org/llama.cpp
- llama.cpp server：https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md
- llama.cpp 量化表：https://github.com/ggml-org/llama.cpp/blob/master/tools/quantize/README.md
- `llama-cpp-2`：https://crates.io/crates/llama-cpp-2
- RTX 4090 / Qwen3-8B Q4_K_M 外部基线（2026-10-22，单次测量，仅作量级参考）：https://llm-speed.com/r/r_kiilk85gcx_
- mistral.rs：https://github.com/EricLBuehler/mistral.rs
- Unsloth：https://github.com/unslothai/unsloth
- Liger-Kernel：https://github.com/linkedin/Liger-Kernel
- PEFT 量化训练：https://github.com/huggingface/peft/blob/main/docs/source/developer_guides/quantization.md
- TRL：https://github.com/huggingface/trl
- ModelScope 文件列表（示例）：https://modelscope.cn/api/v1/models/Qwen/Qwen2.5-7B-Instruct-GGUF/repo/files?Revision=master&Recursive=true
- hf-mirror：https://hf-mirror.com/
- 模力方舟下载：https://moark.com/docs/products/models/download
- Tauri：https://v2.tauri.app/
- OpenAI Chat Completions API：https://platform.openai.com/docs/api-reference/chat
