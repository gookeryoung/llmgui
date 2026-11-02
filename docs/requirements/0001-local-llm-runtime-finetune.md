# 需求卡片 0001：本地大模型运行与微调框架

- 状态：待需求方确认
- 日期：2026-11-02
- 来源：用户直接需求
- 关联 ADR：[0001-local-llm-runtime-finetune](../adr/0001-local-llm-runtime-finetune.md)
- 关联调研：[runtime](../research/runtime.md)、[finetune](../research/finetune.md)、[download](../research/download.md)

## 1. 需求原文（逐字保留）

> 基于RUST、Python技术栈。参考unsloth设计一款高性能本地大模型运行 + 微调框架，实现运行快，省内存。大模型使用国内源进行下载，提高速度。请制定开发方案。

## 2. 目标与成功定义

构建一个**单机优先、开箱即用**的本地大模型工具链：Rust 负责推理运行时、模型下载与服务化，Python 负责 LoRA/QLoRA 微调；模型默认从国内源获取；在同等硬件上做到「运行快、省内存」，并把「下载 → 运行 → 微调 → 导出 → 再运行」串成闭环。

成功不是「功能清单齐全」，而是同时满足：

1. 目标硬件上相对成熟基线（llama.cpp server / HuggingFace PEFT）有可复现的性能或显存优势，且不掉精度；
2. 一个命令完成国内源下载并启动 OpenAI 兼容服务；
3. 一条命令在消费级 24GB 显卡上完成 7B 级 QLoRA，并把 adapter 直接加载回运行时。

## 3. 可实施约束

| # | 需求句（原文片段） | 可实施约束 | 涉及模块 | 验收方式 |
|---|---|---|---|---|
| C1 | 基于 RUST、Python 技术栈 | Rust 承担推理运行时、模型下载/校验、HTTP 服务与 CLI；Python 承担训练。两侧以**独立进程 + 本地 RPC + 共享模型缓存**协作，Rust 进程内不嵌入 PyTorch | `crates/`、`python/llmgui_train/`、`llmgui-train-bridge` | `llmgui train` 拉起 worker；进程边界测试；ADR §7.1 |
| C2 | 高性能本地大模型运行 | 推理路径具备 Rust 批拼装、连续批处理、前缀缓存、KV Cache 量化、mmap 权重；分块 prefill 通过请求级 token 预算控制；相对同机 llama.cpp server 的额外开销 ≤5% | `llmgui-scheduler`、`llmgui-backend-llamacpp` | `llmgui-bench` 报告；P1 退出指标（ADR §13.1） |
| C3 | 运行快 | 定义并实测 TTFT、单请求 decode tok/s、并发聚合吞吐、显存峰值四类指标，按硬件档位给出门槛 | `llmgui-bench`、`llmgui-server` | 固定 harness + nightly 性能回归；ADR §13.1 |
| C4 | 省内存 | 默认 Q4_K_M 量化；权重 mmap；KV Cache 支持 Q8/Q4；加载前显存预估，不足时给出可执行建议 | `llmgui-models`、`llmgui-scheduler` | `llmgui estimate`；OOM 前预检；8B/14B/32B 预算表（ADR §13.2） |
| C5 | 参考 unsloth 设计 | 采用 Unsloth 同类机制：4-bit QLoRA、融合内核、梯度检查点、序列 packing；P2a 复用 Liger，自研 Triton 仅按 profile 准入（1–2 个，P3）。相对主基线 ≥1.6× 训练吞吐，相对无 GC 基线显存 ≤60%（双基线口径见 ADR §13.3） | `python/llmgui_train`、`kernels/` | Python 基准 + A1–A8 消融；ADR §7.6、§13.3 |
| C6 | 微调框架 | 支持 SFT LoRA/QLoRA（P2a）、DPO（P2a，未完成顺延 P3）；数据格式支持 JSONL/Alpaca/ShareGPT；可断点续训；产物为 PEFT safetensors adapter | `llmgui_train` CLI、`data/`、`trainers/` | 端到端 tiny-model 测试；ADR §7.6 |
| C7 | 大模型使用国内源下载 | 默认 ModelScope，自动 fallback 到 hf-mirror / Gitee AI（HF 兼容）；支持分片并发、断点续传、SHA256 校验、镜像测速、离线模式；缓存目录可配置 | `llmgui-download`、CLI | 本地 HTTP fixture 集成测试；ADR §7.5 |
| C8 | 提高速度 | 下载器相对单线程 HTTP 目标 ≥3× 吞吐（同源、同文件、≥1GB）；冷启动到可服务 ≤60s（已缓存模型，SSD） | `llmgui-download`、`llmgui-server` | `make bench-download`；ADR §13.4 |
| C9 | 运行 + 微调闭环 | adapter 导出后可被运行时热加载；支持合并导出与 GGUF LoRA 转换；不重启主进程即可切换 adapter | `llmgui-train-bridge`、`backend-llamacpp` | P1 预置 adapter 切换；P2a 全链路 e2e；ADR §7.7 |
| C10 | 本地 | 默认仅监听 `127.0.0.1`；服务启用本地 token；模型与 adapter 仅接受 safetensors/GGUF，禁止 pickle；下载内容强制校验 | `llmgui-server`、`llmgui-download` | 安全测试；ADR §10 |
| C11 | 框架属性 | 单机优先，Windows 11（build 26300）与 Linux 为首发平台，Windows 10 best-effort（QLoRA 需 Win11+）；CUDA/CPU 为一级后端，Metal/ROCm 与国产卡 post-v1.0 | `backend-api`、平台构建 | CI 矩阵；后端 trait；ADR §7.3 |
| C12 | 技术栈可维护 | Rust workspace + Python `uv` 包；核心逻辑测试覆盖率 ≥95%（排除 FFI/生成代码）；提供 OpenAI 兼容 API 与 CLI；桌面 GUI 放 P2b | CI、`docs/`、`gui/` | CI 门禁；API 契约测试；ADR §11、§12 |

## 4. 现状影响面（greenfield）

| # | 疑似受影响能力 | 候选位置 | 备注 |
|---|---|---|---|
| 1 | 产品代码 | 仓库根目录 | 无 `src/`、`crates/`、`python/`、`frontend/`；`git ls-files` 仅 15 个文件 |
| 2 | 构建与门禁 | 根目录 | 无 `Makefile`、`pyproject.toml`、`Cargo.toml`、CI；AGENTS 的门禁命令当前不可执行 |
| 3 | 流程文档 | `docs/BUNDLE_BUDGET.md`、`docs/REFACTOR_BACKLOG.md`、`docs/AI_DRIVEN_DEV_FLOW.md` | 内容来自 `cndb` 项目（引用不存在的 ADR 0004、`scripts/check_adr_landing.py`、`.github/workflows/*`），属模板残留，P0 需改写或移出本项目口径 |
| 4 | 需求模板 | `docs/REQUIREMENT_CARD_TEMPLATE.md`、`docs/adr/TEMPLATE.md` | 已存在，本卡片按其字段编写；ADR 模板由本方案补齐 |
| 5 | 运行环境 | 开发机 | Windows 11（build 26300）、Rust 1.99.1、Python 3.13.13、Node 25.2.1、RTX 5090 D 32GB（sm_120）、F: 仅约 24GB 空闲；默认缓存必须落用户目录 |

- 无历史数据/接口兼容包袱；但模型清单、缓存布局、API 契约、adapter 格式从第一天起就要版本化。
- 现有 `docs/AI_DRIVEN_DEV_FLOW.md` 是 cndb 流程的副本，不能直接当本项目流程依据；P0 需基于 AGENTS 规则重写为本项目版本，并补齐 Makefile/门禁/CI。

## 5. 范围

**本期纳入**：模型发现/下载/校验/缓存；GGUF 推理与 OpenAI 兼容服务；量化模型加载与显存管理；LoRA/QLoRA SFT；adapter 导出与运行时加载；基准与评测；CLI；P2b 的桌面 GUI（Tauri）。

**本期不纳入**：预训练、全参数大模型训练、多机分布式训练、多租户鉴权与计费、移动端、模型市场运营、对闭源 API 的代理；Metal/ROCm、国产加速卡、speculative decoding、embedding/rerank 均为 post-v1.0。

## 6. 未决问题与默认收敛

| 编号 | 问题 | 默认结论（评审确认） |
|---|---|---|
| Q1 | 首发目标用户与部署形态？ | 个人开发者/小团队的单机工作站；CLI + 本地 API 优先，GUI 后置 |
| Q2 | 是否必须支持国产加速卡？ | P0/P1/v1.0 不做；接口预留，post-v1.0 实验适配，先交付 CUDA/CPU |
| Q3 | 推理内核自研还是复用？ | P0/P1 以 llama.cpp 为执行后端，Rust 自实现 slot/批拼装/prefix cache；v1.0 不评估替代后端，P1 达标后另立 ADR |
| Q4 | 训练是否 fork Unsloth？ | 不 fork、不复制其代码；按其公开思路清洁室实现，复用 PyTorch/PEFT/TRL 生态 |
| Q5 | 模型许可如何约束？ | 只做技术分发与许可展示，不改变模型许可；默认清单优先 Apache-2.0/可商用模型 |
| Q6 | 团队与周期？ | 以 6 人、约 26 周规划；若 2–3 人，按 ADR §12 裁剪到 P0 + P1 + P2a 核心，GUI/P3 延后 |

## 7. 交付物

1. 本卡片（约束基线）。
2. ADR 0001（架构、选型、接口、测试、分期、决策记录）。
3. 三份方向调研（runtime / finetune / download），作为 ADR 的论据附件。
4. P0 启动清单：裁剪 cndb 遗留文档、`Makefile` 门禁/CI、ADR 模板与索引、模型清单 v0。

## 8. 卡点

- [x] 每条需求句都有 C 编号约束（无遗漏）
- [x] 未决问题已按默认结论收敛（Q1–Q6）
- [ ] 需求方确认（人工卡点）

## 9. 确认记录

- 需求方确认：待确认
- 评审结论：待评审
- 备注：本卡片确认后，ADR 才可从 `Proposed` 进入 `Accepted` 并开始实现。
