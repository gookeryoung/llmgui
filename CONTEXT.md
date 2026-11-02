# CONTEXT：术语与产品口径

本文件是仓库的共享术语表。新术语先在需求卡片/ADR 中定义，再登记到这里；同一术语只保留一处权威解释。

## 产品定位

**llmgui**：单机优先的本地大模型「运行 + 微调」框架。Rust 负责推理运行时、模型下载与服务化，Python 负责训练；目标是运行快、省内存、国内源下载快，并把下载→运行→微调→导出→再运行串成闭环。

## 核心术语

| 术语 | 英文/缩写 | 含义 |
|---|---|---|
| 运行时 | runtime | 加载模型、调度请求、执行推理并对外提供 API 的进程；本项目为 Rust 单进程 |
| 执行后端 | backend | 真正执行 token 生成的库；v1.0 为 llama.cpp；`upstream` 为接本地 llama-server 的保底后端；原生后端另立 ADR 评估 |
| GGUF | — | llama.cpp 的模型文件格式，含 metadata、tokenizer 与量化张量；推理侧唯一模型格式 |
| safetensors | — | 无代码执行的张量序列化格式；训练权重、PEFT adapter、merged 模型的唯一权重格式 |
| 量化 | quantization | 以更低比特表示权重/缓存；默认 Q4_K_M，备选 IQ4_XS，KV 默认 q8_0 |
| Q4_K_M | — | GGUF 的 4-bit K-quant 中等档，默认推理量化 |
| IQ4_XS | — | 更小更快的 4-bit i-quant，需 importance matrix；省显存第二档 |
| KV Cache | — | 自回归解码缓存的 Key/Value；显存大头之一，支持 q8_0/q4_0 量化 |
| 前缀缓存 | prefix cache | 复用相同前缀的 KV 状态以跳过 prefill；Rust 用 seq state 快照/恢复实现，键含模型/模板/量化/adapter 指纹 |
| 连续批处理 | continuous batching | 每个 decode step 动态组批；P1 由 Rust 调度器拼 `llama_batch`，不依赖 llama-server |
| 分块预填充 | chunked prefill | 把长 prompt 的 prefill 分片推进；Rust 在 batch 组装层与 decode 混合调度 |
| 分页注意力 | PagedAttention | 以 block table 管理 KV 的机制；P0/P1 不自研，≥16 并发或 KV 利用率 <70% 才重启 |
| 投机解码 | speculative decoding | 用 draft/ngram 先猜多个 token 再验证；不纳入 v1.0，post-v1.0 评估 |
| 槽位 | slot | Rust 维护的逻辑并发序列单元；映射到 libllama 的 seq_id 与 KV 状态 |
| 首 token 延迟 | TTFT | 请求到第一个输出 token 的时间 |
| 每 token 时间 | TPOT | 解码阶段每个输出 token 的平均耗时 |
| 预填充/解码 | prefill/decode | 处理 prompt / 逐 token 生成两个阶段，吞吐分别计量 |
| 显存预估 | estimate | 加载前按 GGUF metadata + KV 公式 + 经验常数估算峰值，失败则拒绝加载 |
| GPU 租约 | GPU lease | 单卡上训练与推理互斥的协调机制，避免争抢显存 |
| PEFT | Parameter-Efficient Fine-Tuning | 参数高效微调生态；本项目产物以 PEFT adapter 为准 |
| LoRA | Low-Rank Adaptation | 低秩适配器；训练默认 r=16、alpha=32、all-linear |
| QLoRA | Quantized LoRA | 4-bit 量化底座 + LoRA；默认 NF4 + double quant + bf16 计算 |
| 适配器 | adapter | LoRA 权重及配置；权威格式为 PEFT safetensors，运行时用 GGUF LoRA 热加载 |
| GGUF LoRA | — | 由 PEFT adapter 转换出的 llama.cpp LoRA 文件；运行时热加载的格式 |
| Liger | Liger-Kernel | BSD-2 的融合内核库；P0 训练加速默认复用，不自研 Triton |
| SFT / DPO | — | 监督微调 / 直接偏好优化；SFT P2a，DPO P2a（未完成顺延 P3） |
| FSDP | — | PyTorch 分片数据并行；P3 单机多卡二选一，post-v1.0 视情况 |
| 内容寻址存储 | CAS | 以 SHA256 为键存放下载块，多个模型快照硬链接复用 |
| 模型库 | library | 本机已安装/已导入模型的统一索引；安装态与加载态分离 |
| 注册表 | registry | `<LLMGUI_HOME>/registry.json`，记录模型索引与用户偏好（别名/默认值/收藏）；可由 manifest 重建 |
| 模型发现 | discover | 按仓库 ID / URL / 关键词检索模型，并按量化、显存、许可以选 |
| 模型适配性 | fit | 按 GGUF metadata + ctx + KV 估算的 24GB/32GB 可跑结论 |
| 即时加载 | JIT | 收到请求时才加载已安装模型；并发首请求合并为一次加载 |
| 空闲卸载 | TTL | 模型在无请求且空闲超过设定秒数后自动卸载；0 表示不自动卸载 |
| 每模型默认值 | per-model defaults | 每个模型的 ctx/KV/GPU offload/slots/TTL，CLI 与 GUI 共用 |
| 模型别名 | alias | 全局唯一的可读标识，`model` 参数按「id → 别名 → 唯一前缀」解析 |
| 预设 | preset | 命名保存的 system prompt 与采样参数，不含加载参数 |
| 派生模型 | derived model | 由训练/merge/量化产物注册出的模型，记录 base 与 adapter lineage |
| 训练底座 | training base | 可训练的 safetensors 模型，只进入训练选择器，不作为推理模型加载 |
| 清单 | manifest | 模型/adapter/训练产物的版本化元数据；含来源、哈希、许可、量化参数 |
| 源 | source | 模型下载源；默认 ModelScope，fallback hf-mirror / Gitee AI（HF 兼容） |
| 能力探测 | capabilities | Python 侧检测设备、量化后端、Triton、注意力实现，选择受支持组合 |
| imatrix | importance matrix | 量化校准矩阵；IQ4_XS 等 i-quant 需要它才能稳定质量 |
| ppl / KLD | perplexity / KL divergence | 量化质量回归指标；固定 wikitext-2 切片与 seed |
| eval loss | — | 固定留出集上的验证损失；与训练 loss 分开，防数据泄漏 |
| 遥测 | telemetry | 默认关闭；指标与日志只落本地，外发需显式 opt-in |
| OpenAI 兼容 API | — | `/v1/chat/completions` 等与 OpenAI 客户端兼容的本地 HTTP 接口 |
| Tauri | — | P2b 桌面 GUI 壳，Rust 后端 + React 前端 |

## 默认口径

- 默认平台：Windows 11（build 26300）+ Linux；Windows 10 仅 best-effort（QLoRA 需 Win11+）。
- 默认推理格式：GGUF；默认量化：Q4_K_M；默认 KV：q8_0。
- 默认服务地址：`127.0.0.1:11435`，Bearer token，默认不开 CORS。
- 默认模型目录：`<LLMGUI_HOME>`，Windows 为 `%LOCALAPPDATA%\llmgui`；不使用仓库所在磁盘作为默认缓存。
- 默认并发承诺：1–8；不承诺数据中心级 16+ 并发。
- 默认训练：单卡 QLoRA；训练与推理共用一张卡时通过 GPU lease 串行。
- 默认许可：自有代码 Apache-2.0；模型许可随 manifest 展示；禁止 AGPL 依赖。
- 默认无遥测；数据与日志不出本机。
