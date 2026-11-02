# 推理运行时方向调研（runtime）

- 状态：初稿（待 Lead / 需求方评审）
- 日期：2026-11-02
- 范围：仅**推理运行时**方向——执行后端选型、调度机制取舍、量化与显存、crate 分层与进程模型、性能门槛。微调与下载见同目录 `finetune.md`、`download.md`。
- 关联：[需求卡片 0001](../requirements/0001-local-llm-runtime-finetune.md) 约束 C1–C4、C7、C11；选型默认沿用卡片 §6 Q3「P0/P1 以 llama.cpp 为执行后端，自研 Rust 调度与服务层」。
- 事实基准日 2026-11-02：关键结论附来源 URL；无来源的数字一律标「估算」并写明硬件 / 量化 / batch 条件。
- **评审修正（2026-11-02）**：文中的 `-cb/--cont-batching`、`-np`、`--cache-ram`、`--ctx-checkpoints`、`--slot-save-path`、`timings.cache_n` 属 `tools/server`（llama-server）实现；`llama-cpp-2` 走 libllama，只提供 `llama_batch`/`llama_decode`/`llama_state_seq_*` 等原语。因此 P1 的 slot 池、每步 batch 拼装、prefix cache 存取必须由 Rust 实现，不能假定 server 能力；最终口径以 [ADR 0001 §7.3/§7.4](../adr/0001-local-llm-runtime-finetune.md) 为准。

## 1. 结论摘要

| 决策项 | 建议 | 一句话理由 |
|---|---|---|
| P0/P1 执行后端 | llama.cpp，**in-process FFI**（`llama-cpp-2` 0.1.160） | 后端覆盖最全（CUDA/HIP/Metal/Vulkan/CPU）、GGUF 与量化是事实标准、MIT 许可 |
| 服务与调度层 | 自研 Rust（tokio + axum），**不 fork、不 vendored 改 llama.cpp** | C2 要求相对 llama-server 额外开销 ≤5%，必须掌握生命周期与取消路径 |
| P2 原生后端评估顺序 | mistral.rs → candle → burn；P0/P1 不重写算子 | mistral.rs 已自带 paged attention / 连续批处理 / ISQ，是最接近「可替换执行层」的 Rust 项目 |
| 默认量化 | GGUF `Q4_K_M`；省显存第二档 `IQ4_XS`（必须配 imatrix） | 同表数据里 IQ4_XS 比 Q4_K_M 更小且生成更快（见 §4.1） |
| KV cache | 默认 `q8_0`；`q4_0` 仅低显存档且默认关闭 | 32k 上下文 f16 KV 约 4.5 GiB，q8_0 约 2.4 GiB（估算） |
| AWQ/GPTQ | **不纳入 P0/P1** | llama.cpp 不支持直接加载；等 P2 接 mistral.rs 再作为可选矩阵 |
| 投机解码 | P1 只上 `ngram-mod`（无额外模型）灰度；P2 再评 EAGLE-3 | 收益依赖接受率与 workload，draft 模型还要额外显存 |
| PagedAttention | **P0/P1 不自研** | 需要自写 attention kernel + block table，单机低并发收益有限（判据见 §3.2） |
| 明确不选 | TensorRT-LLM、`ort` 作为 LLM decode 后端 | 前者无 Rust API 且默认遥测；后者缺连续批处理 / paged KV 原语 |

三条关键判断：

1. Rust 在这一层的价值是**调度、服务、显存治理、下载**，不是重写 GEMM/Attention。llama.cpp 的内核 + 量化格式是多年工程积累，P0 自研同性能内核不是合理投入。
2. 「相对 llama-server 额外开销 ≤5%」只在 in-process 前提下可达成；sidecar HTTP 方案中 SSE 编解码 + 逐 token 转发本身就会吃掉预算。
3. 单机场景下 llama.cpp 的 unified KV + prompt cache + context checkpoints 可覆盖 1–8 并发；vLLM 的 2–4× 来自数据中心高并发与 PagedAttention，不能直接移植期望（见 §3.2）。

## 2. 路线对比表

| 路线 | 形态 | 现成能力（2026-11 官方 README） | GGUF | 后端 | 许可 | 活跃度事实 | 对本项目适配度 |
|---|---|---|---|---|---|---|---|
| **llama.cpp**（C/C++，`llama-cpp-2` 绑定） | 执行库 + server | 连续批处理（默认开）、并行 slot、投机解码（draft/EAGLE-3/ngram 等）、KV cache 量化、mmap、OpenAI 兼容 API | ✅ 原生 | CUDA/HIP/Metal/Vulkan/SYCL/CANN/CPU 等 | MIT | 后端列表与实现持续扩张；133.1k stars、2026-11-02 仍有提交（2599 open issues）；绑定 crate `llama-cpp-2` 0.1.160 发布于 2026-10-15，MIT OR Apache-2.0 | **P0 唯一执行后端** |
| **mistral.rs** | 纯 Rust 引擎 + server + Python/Rust SDK | v0.9.5：连续批处理默认全设备、PagedAttention（CUDA/Apple）、前缀缓存、chunked prefill（部分模型族）、ISQ/GGUF/GPTQ/AWQ/HQQ/FP8/BNB | ✅（2–8bit） | CUDA、Metal、多卡 | MIT | 7.8k stars、2026-11-02 仍有提交（420 open issues）；v0.9.5 为当前文档版本；自报基准覆盖 GB10/B200/H100（厂商自测） | P2 首选评估对象 |
| **candle** | 极简 Rust 张量框架 | 带 llama/qwen/mixtral 等示例与量化示例；无内置服务与调度 | 部分（quantized 示例） | CUDA/Metal/CPU/WASM | Apache-2.0 / MIT 双许可 | 21.3k stars，2026-11-01 仍有提交，957 open issues | 适合做 kernel/模型实验，不适合直接当运行时 |
| **burn** | Rust 训练+推理框架 | 0.22：CubeCL 后端（CUDA/ROCm/Metal/Vulkan/WebGPU），JIT 融合；LibTorch 后端已在 0.22 弃用并从 main 移除 | ❌（需自行实现） | 同上 | MIT/Apache-2.0 | 版本节奏稳定但仍有 breaking change | 训练侧备选，推理解码无公开对标 |
| **ort**（ONNX Runtime） | FFI 绑定 | 2.0.0-rc 系列，onnxruntime 1.31；EP 覆盖 CUDA/TensorRT/CoreML/DirectML/OpenVINO 等 | ❌ | 取决于 EP | Apache-2.0 | 2.6k stars；issue 少、维护集中 | 适合 embedding/rerank/小模型，不适合 LLM decode |
| **TensorRT-LLM** | Python/PyTorch 原生推理框架 | 1.4.0rc3，CUDA 13.4.1，PyTorch 2.14；自定义 kernel、prefill-decode 分离、投机解码 | ❌ | NVIDIA only | Apache-2.0 | NVIDIA 官方维护；**默认开启匿名遥测**（可用 `TRTLLM_NO_USAGE_STATS=1` 关闭） | 不作为本期后端（无官方 Rust API、平台锁定、部署形态不符） |

补充判断：

- **llama.cpp 仍是唯一「拿来即用」的执行层**：它的 server 已内置连续批处理、投机解码与 prompt cache，我们要做的是在 Rust 里把它当作可编排的引擎，而不是重新实现它。
- **mistral.rs 的价值在 P2**：官方 README 显示它已把 paged attention / 前缀缓存 / 连续批处理做成默认能力，并给出与 llama.cpp 的 Q8 对比（如 Gemma 4 E4B 在 GB10 上 Q8 prefill 7395.7 vs 3973.7 tok/s）；但这是**厂商自报数据**，且 0.9.x 仍在快速演进，直接替换执行层风险高。
- **candle 的定位是组件而非运行时**：社区项目 `candle-vllm`、`vllm.rs` 说明在 Rust 中做 PagedAttention 仍是「项目级」投入，不是开箱能力。
- **burn 的核心优势是「训练代码即推理代码」**，与我们「Python 训练 + Rust 推理」的分工不重合；且其 LLM 解码路径没有可对标的公开基准。
- **ort 的问题不是性能而是抽象层级**：ONNX Runtime 不提供自回归 decode 的连续批处理 / KV 分页原语，逐 token 图调用开销大。
- **TensorRT-LLM 是正确但不可用的答案**：性能最强，但无 Rust API、NVIDIA 锁定、默认遥测，与 C10/C11 冲突。

## 3. 关键机制取舍

### 3.1 连续批处理（continuous / iteration-level batching）

- 事实：llama-server 的 `-cb/--cont-batching` **默认开启**，配合 `-np` slot 实现多用户并行；Orca 以 iteration-level 调度在 OPT-30B 上取得 36.9× 吞吐提升（OSDI'22，二手整理见参考 [12]）。
- 取舍：批拼装留在 llama.cpp 内部，Rust 层**不重新实现 batching**，只做「准入 / 公平 / 取消 / 超时 / 背压」。理由：llama.cpp 的 batch 组成对上层不可编程，自研调度器无法真正改变 kernel 行为，收益低而回归面大。
- 落地：Rust 维护 slot 池（默认 `-np` = 显存预算允许值），请求进入有界队列；取消以「下一个 token 边界生效」为语义；每路序列独立 SSE 通道。P2 若接原生后端，才需要真正的 batch 调度器。

### 3.2 PagedAttention

- 事实：vLLM 论文（SOSP'23）相对 FasterTransformer / Orca 在同延迟下吞吐提升 **2–4×**，长序列与大模型收益更明显（arXiv:2309.06180 摘要）。
- 现状：llama.cpp **没有** PagedAttention，而是 unified KV buffer（`--kv-unified`）+ context checkpoints（`--ctx-checkpoints`，默认 32）+ slot save/restore 来缓解碎片与复用。
- 取舍：P0/P1 不自研。代价估算：需要自定义 attention kernel、block table、copy-on-write 与 prefix 共享；`candle-vllm`、`mistral.rs` 都投入了长期迭代。
- 何时重启：当单机并发需求 ≥16、或长短请求混跑导致 KV 利用率 <70% 时再立项（估算触发线，需实测碎片率）。
- 单机现实：4 路 × 8k 上下文的固定预留会浪费 20–35% KV 空间（估算），低于 PagedAttention 的立项门槛，用更小的 per-slot ctx + prefix cache 抵消即可。

### 3.3 前缀缓存（prefix caching）

- llama.cpp 提供三层：进程内 prompt cache（`--cache-ram`，默认 8192 MiB）、context checkpoints（按 token 间隔切分）、slot 级 save/restore（文件持久化）；响应 `timings.cache_n` 直接暴露命中量。
- 建议：Rust 层维护「前缀内容 hash → (slot, checkpoint)」映射。system prompt、工具定义、多轮历史是最稳定前缀；命中后跳过 prefill，直接增量 decode。
- 工程约束：缓存键必须包含模型指纹 + chat template 版本 + 量化类型，否则换模型/换模板会静默复用错误 KV；缓存容量进入配置项（默认放模型缓存同盘，LRU 上限可配）。

### 3.4 chunked prefill

- 事实：Sarathi（arXiv:2308.16369）用 chunked-prefill + decode-maximal batching，在 LLaMA-13B/A6000 上 decode 吞吐最高 10×、端到端 1.33×；LLaMA-33B/A100 上端到端 1.25×。
- 现状：llama.cpp 用 `-ub`（默认 512）把 prefill 物理分片，属于「分片推进」，不是「与其它请求的 decode 交织调度」；mistral.rs 在部分模型族明确支持 chunked prefill。
- 取舍：P1 用「ubatch 调参 + 请求级 prefill token 预算」达成 TTFT 与 TPOT 的可控折中，不做跨请求交织。若 P2 自研调度，再引入 Sarathi 式 piggyback。
- 风险：ubatch 调小会显著降低 prefill 吞吐（更多 kernel 启动），必须在 §6 的指标上同时约束 TTFT 与 prefill tok/s。

### 3.5 投机解码（speculative decoding）

- 事实：Leviathan 等（ICML'23）在 T5-XXL 上取得 **2–3×** 加速且输出分布不变；llama.cpp 已支持 draft 模型、EAGLE-3、DFlash/DSpark、MTP 头与 n-gram 系列（`--spec-type`），并在 metrics 中暴露接受率。
- 取舍：投机收益 = 接受率 × 单步节省，代码补全/改写类高、开放对话低；draft 模型额外显存 0.5–2 GB（估算），batch 越大收益越小。
- 建议：P1 只灰度 `ngram-mod`（官方文档称约 16 MB、常数内存，无额外模型）；P2 再评估 EAGLE-3，并以「接受率 ≥40%、端到端 decode 提升 ≥15%」作为默认开启门槛（估算阈值）。

### 3.6 采样 / tokenizer / chat template

- 统一使用 llama.cpp 的 sampler 与 Jinja 模板（`--jinja` / chat template kwargs），Rust 侧只做 OpenAI 参数映射与约束（JSON schema / grammar 交给 llama.cpp 的 `json_schema`、GBNF）。
- **不要**用 Rust `tokenizers` 另起一套分词：GGUF vocab、特殊 token、BOS/EOS 行为必须与执行器一致，否则 logits 错位、tool call 解析失败。Rust 侧只持有 token id，不持有 vocab 副本。

## 4. 量化与内存

### 4.1 GGUF 量化谱系

以下 bits/weight 与尺寸来自 llama.cpp 官方 `tools/quantize/README.md`（Llama-3.1-8B 参考表；**该表未注明测试硬件**，只能做相对参考）：

| 量化 | bits/weight | Llama-3.1-8B 尺寸 | 生成 t/s（该表机器） | 用途 |
|---|---|---|---|---|
| Q4_K_M | 4.8944 | 4.58 GiB | 71.93 | **默认** |
| IQ4_XS | 4.4597 | 4.17 GiB | 77.51 | 省显存第二档（需 imatrix） |
| Q5_K_M | 5.7036 | 5.33 GiB | 67.23 | 质量优先 |
| Q3_K_M | 3.9960 | 3.74 GiB | 71.68 | 24GB 跑 14B（或 32B + 小 ctx） |
| IQ2_M | 2.9294 | 2.74 GiB | 74.44 | 极端低显存（必须 imatrix） |
| Q8_0 | 8.5008 | 7.95 GiB | 50.93 | 参考上限 |
| F16 | 16.0005 | 14.96 GiB | 29.17 | 精度基线 |

- 结论：默认 `Q4_K_M`；「省显存」主推 `IQ4_XS`（同表更小且更快），但必须提供 imatrix 才能接近 K-quant 质量；低于 Q3 的档位默认关闭，只在显式开关下提供并附 ppl/KLD 报告。
- IQ 系列依赖 importance matrix（`--imatrix`）；无 imatrix 的 IQ 量化质量不稳定，不应作为默认分发物。

### 4.2 AWQ / GPTQ

- llama.cpp 不支持直接加载 AWQ/GPTQ；mistral.rs README 明确支持 GPTQ/AWQ/HQQ/FP8/BNB/ISQ。
- 本期不引入（与 Q3 一致）：模型清单只收 GGUF，避免「下载格式 × 后端」组合爆炸。P2 接 mistral.rs 后再作为可选矩阵，且需单独做质量回归。

### 4.3 KV cache 量化

llama.cpp 允许的 KV 类型：`f32/f16/bf16/q8_0/q4_0/q4_1/iq4_nl/q5_0/q5_1`（`-ctk` / `-ctv`，默认 f16）。计算公式：

```
bytes/token = 2 × layers × kv_heads × head_dim × bytes_per_element
```

Qwen3-8B（已核对 config：36 层 / 8 KV heads / head_dim 128 / 训练上下文 40960）：f16 为 144 KiB/token → 32k 上下文 **4.5 GiB**；q8_0 约 **2.4 GiB**；q4_0 约 **1.3 GiB**（后两者为按 8.5/4.5 bit 折算的估算）。

- 建议：默认 K/V 都用 `q8_0`；`q4_0` 仅 <16GB 显存档兜底，且必须跑长上下文 ppl 回归后才允许开启。
- 注意：KV 量化对长上下文与数学/代码任务更敏感，A/B 时固定 prompt 集与生成长度。

### 4.4 mmap 与「零拷贝」的真实边界

- llama.cpp 的 `--load-mode auto` 默认走 mmap（server README 明示），CPU 后端可直接按页读取权重，这是真正的「按需加载」。
- **CUDA/GPU 后端不存在零拷贝**：权重必须 H2D 拷贝进显存；「省内存」体现在 host 侧不产生额外副本 + 按 `-ngl` 分片 + `--no-host` 等开关控制 host buffer。
- Windows 上 mmap 的页错误开销明显：建议顺序预读 + 首请求前 warmup；不要默认 `mlock`（会把 commit 顶满，反而触发系统换页）。
- `llama-cpp-2` 提供 `dynamic-backends`（dlopen ggml 后端）特性，可做「单二进制 + 多后端动态库」，显著降低 Windows 打包复杂度；但后端缺失必须有明确诊断，不能静默退回 CPU。

### 4.5 内存预算（Qwen3-8B 的 config 已核对，各数值为估算）

| 组成部分 | f16 KV | q8_0 KV | q4_0 KV | 说明 |
|---|---|---|---|---|
| 权重 Q4_K_M | 5.0 GB | 5.0 GB | 5.0 GB | 估算：Llama-3.1-8B 的 4.58 GiB + Qwen3 更大词表约 +0.1 GB |
| KV @32k | 4.8 GB | 2.6 GB | 1.4 GB | 由 4.5/2.4/1.3 GiB 换算（公式见 4.3） |
| compute buffer / graph | 1.0 GB | 1.0 GB | 1.0 GB | 估算，ubatch 512、FlashAttention 开 |
| CUDA context + tokenizer + 运行时 | 0.6 GB | 0.6 GB | 0.6 GB | 估算 |
| **合计** | **≈11.4 GB** | **≈9.2 GB** | **≈8.0 GB** | — |
| 24GB 卡结论 | 可跑，剩 ~12.6 GB 可再开 2–3 路 8k 序列 | 可开 ≥4 路 8k | 可开 ≥5 路 8k | 估算 |
| 32GB 卡（5090D） | 可上 14B Q4_K_M（约 16.3 GB = 9 GB 权重 + 5.4 GB KV@32k + 1.9 GB buffer/context，估算） | 32B dense 需 q8_0 KV + 降 ctx 才可能 | 32B 可试 16k ctx | 估算 |

- 加载前预估接口（`llmgui estimate`）：权重 size 直接读 GGUF metadata，KV 用上面的公式，buffer/context 用固定经验常数（按后端 + ubatch 分档），目标误差 ≤5%。

## 5. crate 分层建议与进程 / 线程模型

### 5.1 workspace 分层

| crate | 职责 | 允许依赖 | 禁止事项 |
|---|---|---|---|
| `llmgui-core` | 配置、错误、请求/响应类型、取消 token、metrics 抽象 | serde/thiserror/tracing | 不依赖任何 FFI 类型 |
| `llmgui-models` | GGUF metadata 解析、量化识别、显存预估、模型清单与指纹 | memmap2/serde | 不执行推理 |
| `llmgui-backend-api` | `Engine` trait（load / generate / embed / cancel / stats）与事件流类型 | 仅 core | 不泄漏 llama.cpp 类型 |
| `llmgui-backend-llamacpp` | 唯一允许 `unsafe`/FFI 的边界；封装 `llama-cpp-2`、后端 feature、dynamic-backends | llama-cpp-2 | 不实现调度策略 |
| `llmgui-scheduler` | slot 池、准入、公平、取消、超时、背压、prefix cache 映射 | core/backend-api | 不依赖具体后端类型 |
| `llmgui-server` | axum：OpenAI/Anthropic 兼容 API、SSE、本地 token 鉴权、限流 | scheduler | 不直接调 FFI |
| `llmgui-download` | 国内源解析、分片并发、断点续传、SHA256 校验、镜像测速（与 `download.md` 对齐） | hf-hub/tokio | 不感知推理引擎 |
| `llmgui-train-bridge` | 拉起 Python worker、adapter 元数据、热加载指令（本地 RPC） | core | 不嵌入 PyTorch |
| `llmgui-bench` | llama-bench 口径的 TTFT/TPOT/吞吐/显存采集与报告 | server/CLI | 不含生产逻辑 |
| `llmgui-cli` | `pull / run / serve / estimate / bench / adapter` 子命令 | 以上全部 | — |

分层硬规则：FFI 类型不出 `llmgui-backend-llamacpp`；`Engine` trait 只暴露 owned 的 token id 流与统计；scheduler 不感知 llama.cpp 类型，否则 P2 换后端等于重写。

### 5.2 进程 / 线程模型

- **单进程**：`llmgui` 一个 Rust 二进制承载 CLI + HTTP 服务 + 调度 + 下载；不引入 Python 运行时（C1）。
- 线程组：
  1. tokio runtime（HTTP/SSE/CLI/下载）：`worker_threads = min(4, 物理核/2)`，避免与推理抢核；
  2. engine 线程：每个已加载模型 1 个，独占 llama context（context 非 `Send`，只在此线程 decode）；
  3. scheduler：只做队列与记账，不阻塞、不下发 kernel；
  4. sampler：P0 交给 llama.cpp 内置，不单独起线程；
  5. 下载 worker：tokio + 多连接 Range（详见 `download.md`）。
- **与 Python 的关系**：训练是独立进程（`llmgui train` 拉起 uv 环境），本地 RPC + 共享模型缓存；adapter 热加载 P1 只支持同架构 LoRA 切换，合并导出 / GGUF LoRA 转换放 P2。
- 不变量：单写者（同一 context 同时只有一个 decode 发起者）；取消在 token 边界生效；显存不足必须在 load 前失败并给出可执行建议（降 ctx / 换 IQ4_XS / 降 ngl），不做 OOM 后重试。

## 6. 性能验收目标

### 6.1 基准条件（必须逐项固定，否则数字不可比）

- 硬件：RTX 5090 D 32GB（需求卡片 §4 记录的开发机，Blackwell sm_120）+ RTX 4090 24GB 作对照；Windows 11；CUDA 12.8+（Blackwell 需 12.8 起，见参考 [18][19]）。
- 模型：Qwen3-8B **Q4_K_M** 为默认；对照 Llama-3.1-8B Q4_K_M。
- 协议：prompt 128/512/1024/4096 tokens 各 20 条（固定 seed，从公开对话数据采样），output 128 tokens；warmup 3 次；每配置 5 次取中位数；GPU 固定 power limit；报告 p50/p95 TTFT、TPOT、decode / prefill tok/s、NVML 显存峰值。
- 对照组：官方 `llama-server`（同 llama.cpp commit、同 `-ngl`、同 KV 类型、同 `-ub`），这是我们「额外开销 ≤5%」的分母。

### 6.2 实测基线（外部来源，非本项目实测）

llm-speed 于 2026-10-22 在 **RTX 4090 24GB** 上测 Qwen3-8B Q4_K_M + llama.cpp（build 11803）：**decode 155.0 tok/s、prefill 2155.2 tok/s、TTFT 51.0 ms**（单用户短对话，单条 run）。同机 Llama-3.1-8B Q4_K_M 为 decode 160.8 tok/s、prefill 4283.1 tok/s、TTFT 31.5 ms。仅作量级基线，prefill 随 prompt 长度显著变化。

### 6.3 验收门槛（**目标值为估算**，须按 6.1 方法实测确认）

| 指标 | RTX 4090 24GB | RTX 5090 D 32GB | 口径 |
|---|---|---|---|
| 8B Q4_K_M 单流 decode | ≥ 140 tok/s（基线 155 的 90%） | ≥ 230 tok/s（按带宽比 ~1.5× 估算） | output 128，中位数 |
| prefill（1k prompt） | ≥ 2,000 tok/s | ≥ 3,000 tok/s | 同上 |
| TTFT p95 @1k prompt | ≤ 800 ms | ≤ 550 ms | 含 tokenize + 首 token |
| 4 并发聚合 decode（每路 8k ctx） | ≥ 300 tok/s | ≥ 450 tok/s | 4 客户端同时长输出 |
| 相对 `llama-server` 额外开销 | ≤ 5% | ≤ 5% | decode 与 prefill 分别测 |
| 8B Q4 32k f16 KV 峰值显存 | ≤ 11.5 GB | ≤ 11.5 GB | NVML 峰值，`-ngl all` |
| 冷启动（已缓存模型，SSD）到可服务 | ≤ 60 s（C8 口径） | ≤ 60 s | 进程启动到 `/health` 200 |
| 量化质量 | Q4_K_M 相对 F16 ppl 增幅 ≤ 3%、KLD 无异常抬升（估算门槛） | 同 | wikitext-2 固定切片，固定 seed |

### 6.4 达标判定

P0 通过 = 上表全部满足 + 连续 3 个 nightly 无回归（任一指标波动 >5% 需给出解释或修复）；任何一项靠「换更小模型」达标视为未通过。

## 7. 风险与对策

| # | 风险 | 影响 | 对策 | 触发信号 |
|---|---|---|---|---|
| 1 | `llama-cpp-2` 仍是 0.1.x，绑定 commit 漂移、上游 API 变动 | 升级即 break | 固定 `llama-cpp-sys-2` 版本；FFI 只在一个 crate 内；每版跑 golden 输出对比 | 上游 breaking PR / 升级后输出差异 |
| 2 | Windows 上 llama.cpp 构建（MSVC + CMake + CUDA 12.8+）不可复现 | 交付阻塞 | 预编译二进制 + `dynamic-backends` dlopen；CI 缓存；工具链版本写进文档 | 新机器构建失败 |
| 3 | 5090 D / sm_120 内核覆盖不全 | 性能不达标或崩溃 | 以 CUDA 12.8+ 构建并跑后端自检；保留 CPU/Vulkan 降级路径 | CUDA error / 输出异常 |
| 4 | 对 PagedAttention 的期望错配（对标 vLLM 的 16+ 并发） | 并发指标不达预期 | 对外只承诺 1–8 并发，并以 §6.3 作为契约；≥16 并发需求走 P2 立项 | 用户提出高并发场景 |
| 5 | 投机解码收益不稳定 | 默认开后掉速 | P1 只灰度 `ngram-mod` + 接受率指标，低于阈值自动关闭 | 接受率 <40% |
| 6 | KV 量化掉质量（长上下文、数学/代码） | 质量事故 | 默认 q8_0；q4_0 默认关闭；ppl 回归进 CI | ppl 增幅 >5% |
| 7 | Windows mmap 随机读放大 | TTFT 抖动、首启慢 | 顺序预读 + warmup；提供 `--no-mmap` 诊断开关 | 冷启 TTFT 飙升 |
| 8 | 许可：Unsloth 核心 Apache-2.0，但 Unsloth Studio UI 为 AGPL-3.0 | 合规风险（跨调研） | 只按公开论文/文档清洁室实现，不复制代码（卡片 Q4）；依赖树禁止出现 AGPL | 依赖或代码相似度检查命中 |

## 8. 参考链接

1. llama.cpp README（后端列表、量化位宽、MIT）：https://github.com/ggml-org/llama.cpp
2. llama.cpp server README（连续批处理、投机解码、KV 类型、cache-ram、ctx-checkpoints、load-mode）：https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md
3. llama.cpp 投机解码文档（draft/EAGLE-3/DFlash/DSpark/ngram 与统计）：https://github.com/ggml-org/llama.cpp/blob/master/docs/speculative.md
4. llama.cpp 量化工具与量化表（bits/weight、尺寸、t/s）：https://github.com/ggml-org/llama.cpp/blob/master/tools/quantize/README.md
5. `llama-cpp-2` crate（0.1.160，MIT OR Apache-2.0，feature 列表）：https://crates.io/crates/llama-cpp-2
6. mistral.rs README（v0.9.5、paged attention、连续批处理、量化支持、自报基准）：https://github.com/EricLBuehler/mistral.rs
7. `mistral.rs` 许可证（MIT）：https://github.com/EricLBuehler/mistral.rs/blob/master/LICENSE
8. candle README（示例、量化、后端；MIT/Apache 双许可）：https://github.com/huggingface/candle
9. candle 仓库活跃度（21.3k stars，2026-11-01 提交）：https://api.github.com/repos/huggingface/candle
10. burn README（0.22、CubeCL 后端、LibTorch 后端移除）：https://github.com/tracel-ai/burn
11. PagedAttention / vLLM（SOSP'23，2–4× 吞吐）：https://arxiv.org/abs/2309.06180
12. Orca 连续批处理整理（OSDI'22，36.9×；二手整理）：https://github.com/qfc-network/ai-infra/blob/main/foundational/orca/en.md
13. Sarathi（chunked prefill，decode 最高 10× / e2e 1.33×）：https://arxiv.org/abs/2308.16369
14. Speculative decoding（ICML'23，2–3×）：https://arxiv.org/abs/2211.17192
15. `ort` README（onnxruntime 1.31、EP）：https://github.com/pykeio/ort
16. `ort` 仓库信息（Apache-2.0、活跃度）：https://api.github.com/repos/pykeio/ort
17. TensorRT-LLM README（1.4.0rc3、CUDA 13.4.1、默认遥测、Apache-2.0）：https://github.com/NVIDIA/TensorRT-LLM
18. NVIDIA CUDA 12.8 Release Notes（Blackwell 支持）：https://docs.nvidia.com/cuda/archive/12.8.0/cuda-toolkit-release-notes/index.html
19. llama.cpp「CUDA: update build CTK version to 12.8」PR：https://github.com/ggml-org/llama.cpp/pull/13360
20. RTX 4090 / Qwen3-8B Q4_K_M 实测（2026-10-22，llama.cpp）：https://llm-speed.com/r/r_kiilk85gcx_
21. RTX 4090 硬件汇总（8B/14B/32B 量级）：https://llm-speed.com/hw/rtx-4090-24gb
22. Qwen3-8B config（36 层 / 8 KV heads / head_dim 128）：https://hf-mirror.com/Qwen/Qwen3-8B/raw/main/config.json
23. `hf-hub` 1.0.0（`HF_ENDPOINT` 与 builder endpoint，Apache-2.0）：https://docs.rs/hf-hub/latest/hf_hub/
24. HF-Mirror（`HF_ENDPOINT=https://hf-mirror.com` 用法）：https://hf-mirror.com/
25. ModelScope 库 README（模型下载 API / git，Apache-2.0）：https://github.com/modelscope/modelscope
26. Unsloth README（2× faster / 70% less VRAM 宣称；Apache-2.0 + Studio AGPL-3.0）：https://github.com/unslothai/unsloth
27. mistral.rs 仓库活跃度（7.8k stars、420 open issues）：https://api.github.com/repos/EricLBuehler/mistral.rs
28. llama.cpp 仓库活跃度（133.1k stars、2026-11-02 提交）：https://api.github.com/repos/ggml-org/llama.cpp
