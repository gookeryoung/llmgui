# 调研：Python 微调层（对标 Unsloth 的训练加速与省显存）

- 日期：2026-11-02　|　性质：方向调研（不写实现代码，不作为最终架构决策）
- 关联：需求卡片 [0001](../requirements/0001-local-llm-runtime-finetune.md) 约束 C5/C6/C9/C11/Q4
- 范围：训练侧（SFT/DPO、LoRA/QLoRA、融合内核、显存预算、产物交接、IPC、验收消融）；推理运行时与下载源见同目录 runtime.md / download.md

## 1. 结论摘要

| 结论 | 选择 | 理由（关键事实见 §2、§9） |
|---|---|---|
| 与 Unsloth 的关系 | **不 fork、不复制代码**，清洁室实现其公开思路（已在需求 Q4 定调） | Unsloth 核心 Apache-2.0，但 Studio UI 为 AGPL-3.0；其性能数字与自家 Triton 内核、`transformers 5.x/trl 0.27.x` 版本强绑定，fork 后维护成本 > 收益 |
| 训练框架基座 | **PyTorch + Transformers + PEFT + TRL + Accelerate** | 事实标准，PEFT 覆盖 bnb/torchao/HQQ/EETQ/GPTQ 等量化训练路径，TRL 提供 SFT/DPO/GRPO Trainer 且已集成 Unsloth/Liger 作为加速后端 |
| 融合内核 | **P0 默认复用 Liger-Kernel（BSD-2），P1 仅按 profile 补 1–2 个自研 Triton 内核** | Liger 已覆盖 RMSNorm/RoPE/SwiGLU/FusedLinearCE + DPO 系 chunked loss，官方 CI 含 Ascend NPU；自研属于高风险低边际收益 |
| 4-bit 训练 | **CUDA: bitsandbytes NF4 QLoRA；非 CUDA: 降级 bf16 LoRA 或纯 PyTorch NF4 兜底** | bnb 官方支持矩阵只有 CPU/CUDA(含 ROCm)/XPU/HPU/MPS，无昇腾/MUSA/MACA/MLU |
| 首发显存档位 | 24GB：7B bf16 LoRA、14B QLoRA；32GB（本机 5090D）：14B QLoRA/bf16 临界、32B QLoRA；48GB：32B bf16 LoRA | 见 §4 估算，与 Unsloth 官方最低显存表交叉校验 |
| 产物契约 | **PEFT safetensors adapter + 训练 manifest** 为唯一权威产物；merged bf16、GGUF(+LoRA) 为派生产物 | 运行时可直接按 `Wx + scale·B(Ax)` 施加 adapter，无需重训；GGUF 交给 llama.cpp 路径 |
| 进程边界 | Rust 主进程负责 CLI/服务/缓存/GPU 排他；Python worker 只做训练与导出，**NDJSON/JSON-RPC over stdio（v1）** + 共享文件系统 | 与 C1「Rust 进程内不嵌入 PyTorch」一致，且跨 Windows/Linux 最简单 |
| 验收门槛 | 相对同配置 PEFT 基线 **≥1.6× 吞吐、显存 ≤60%**，eval loss 偏差 ≤2% | 对齐需求 C5；消融矩阵见 §7 |

## 2. Unsloth 机制拆解与可复现性评级

Unsloth 官方口径：训练 **2× 快、70% 显存**（README）；引入新 RoPE/MLP Triton 内核 + padding-free/packing 后可达 **3×（最高 5×）**，显存再降 30%–90%，且宣称 loss 与不打包训练**完全一致**。这些是"同类硬件同类配置"的相对值，不可直接当作我方验收数字。

| 机制 | 具体做法（公开信息） | 宣称效果 | 可复现性 | 我方策略 |
|---|---|---|---|---|
| 融合 Triton 内核 | QK-RoPE 合并为一个内核并支持 varlen（packing 前提）、RoPE 完全原地化去掉 clone/contiguous；SwiGLU/GeGLU 用 int64 索引与 `LONG_INDEXING` 特化；CrossEntropy 融合 | RoPE 长上下文 2.3×（短 1.9×）；省中间张量 | **高**（思路清晰，Liger 有等价实现） | 直接用 Liger；必要时补自研 |
| 手写 autograd / 原地反向 | 为融合算子手写 backward，反向尽量原地写回，省 activation 与 malloc | 显存下降的主要来源之一 | 中（Liger 同样"exact"实现并有收敛对齐测试） | 不自研 autograd 框架 |
| 无污染 packing + 默认 padding-free | 多条样本拼成 1D token 流，靠 seq metadata 做块对角 attention + 重置 position ids；padding-free 默认开启且 loss 不变 | packing 2–5×（短样本占比越高越快）；padding-free 1.1–2×、-30% 显存 | **高**（TRL `packing=True` + 正确 attention 后端即可） | P0 必做：packing + 正确 varlen 注意力 |
| 动态 4-bit 量化 | 发布 `unsloth/*-bnb-4bit` 动态量化权重，按层保留敏感层精度以弥补 QLoRA 精度损失 | 精度接近 16-bit LoRA | 中（需自有校准流程与评测） | P0 先用标准 NF4；P1 再评估分层混合精度 |
| gradient checkpointing 变体 | `use_gradient_checkpointing="unsloth"`，更省显存的重算策略 | 降显存、吞吐损失小 | 中（等价于 selective/offload 策略） | 用 HF 标准 full GC 起步，P1 评估 selective |
| Split LoRA（MoE） | 不物化 `B@A` 到所有专家，改为 `X@A`→`@B` 顺序（结合律），MoE 场景省 35%+ 显存 | MoE 快 1.4–12× | 中高（数学等价） | P0 不做 MoE；仅作为未来优化项记录 |
| 版本绑定 | 官方声明良好支持 `trl==0.27.1` / `transformers==5.1.0`，Windows 原生支持无需 WSL | — | — | 我方 pin 版本并做 CI 兼容矩阵 |

需要注意 Unsloth 自己的限制：**MoE 的 4-bit QLoRA 不可用**（bitsandbytes 不支持 grouped_mm/`nn.Parameter` 形式专家权重），这也提示我们：不要把"4-bit 训练"当作全模型族通用能力。

## 3. 技术栈选型

### 3.1 推荐版本区间（Python 侧）

- Python 3.11–3.13（开发机 3.13；Unsloth 文档要求 3.11 ≤ v < 3.14，bnb/triton 生态同理）
- `torch 2.9.x`（CUDA 12.x / ROCm 7.x）、`transformers 5.x`、`peft`、`trl`、`accelerate`、`datasets`
- `bitsandbytes`（CUDA/ROCm/XPU/CPU；**Windows 官方要求 Win11/Server 2022+**，Win10 需实测）
- `liger-kernel`（Triton：CUDA ≥2.1.2/triton ≥2.3.1；ROCm torch ≥2.5/triton ≥3.0）
- **Windows 的 Triton 是半官方状态**：官方 triton 无 Windows wheel，社区维护的 `triton-windows`（现挂在 `triton-lang` 组织下）已发到 3.9.0，需显式安装并锁版本；这是 Windows 首发平台的最大不确定性。

### 3.2 训练方法（P0/P2）

| 方法 | 实现 | 关键参数 | 阶段 |
|---|---|---|---|
| LoRA SFT | PEFT `LoraConfig` + TRL `SFTTrainer` | `r=16、alpha=32、target_modules="all-linear"、dropout=0.05、bias="none"` | P0 |
| QLoRA SFT | `BitsAndBytesConfig(load_in_4bit, nf4, double_quant, bf16 compute)` + `prepare_model_for_kbit_training` | 4-bit 权重 + bf16 计算；LoRA 用 bf16 | P0 |
| DPO | TRL `DPOTrainer` + Liger chunked DPO loss（宣称省最多 80% 对齐阶段显存） | beta/长度控制；参考模型可 4-bit 加载 | P2 |
| 数据 | JSONL/Alpaca/ShareGPT 统一转为 chat template；断点续训（`resume_from_checkpoint`） | packing 默认开、`assistant_only` 掩码正确性单测 | P0 |

### 3.3 量化后端与 bitsandbytes 替代矩阵

| 后端 | 可训练 | 平台现实 | 取舍 |
|---|---|---|---|
| **bitsandbytes NF4/INT8** | 是（QLoRA 事实标准） | Linux/Win11 CUDA、ROCm、Intel XPU；Gaudi 部分；CPU/MPS 可跑 | 首选；内核为手写 CUDA/HIP，国产卡不可移植 |
| **torchao** (weight-only) | 是（PEFT 明确支持 LoRA；int8 路径可靠，`int4_weight_only` 经 transformers 路径**尚不支持**，见 PEFT 文档） | 依赖 PyTorch 后端与硬件 | 可作非 bnb 候选；merge 仅在 int8 weight-only 下正确；需实测 |
| **HQQ** | 是（PEFT 支持） | 纯 PyTorch 反量化，可移植性最好 | 国产卡兜底方案：慢但能跑，适合功能验证 |
| **EETQ** | 是（int8，宣称快于 LLM.int8） | CUDA 为主 | 仅作性能候选 |
| **GPTQ/AWQ** | 仅"加载后 PEFT 再训"，merge 受限 | 推理生态成熟 | 定位推理产物，不做训练默认 |
| **纯 PyTorch NF4 反量化** | 是（自研 ~200 行） | 任意 torch 设备 | 三级兜底 + 数值对照基准；不追求性能 |
| **GGUF** | 否（llama.cpp 不支持反向训练） | — | 只做导出/部署 |

### 3.4 「自研 Triton 内核 vs 复用 Liger/Unsloth」

| 维度 | 复用 Liger | 自研 Triton |
|---|---|---|
| 许可 | BSD-2，可商用、可魔改 | 自主 |
| 覆盖面 | RMSNorm/RoPE/SwiGLU/GeGLU/CE/FusedLinearCE + DPO/ORPO/SimPO/KTO/GRPO chunked loss，支持 30+ 架构 patching | 从 0 开始 |
| 硬件 | CUDA/ROCm/Intel GPU/**Ascend NPU 官方 CI** | 需逐后端适配（triton-ascend/triton-musa 语法子集不同） |
| Windows | 依赖 triton-windows（社区维护） | 同样依赖，风险不降 |
| 风险 | 版本漂移（transformers 5.x 快速演进） | 数值/收敛风险、长尾 bug、维护人力 |

**建议（明确取舍）**：P0 **不自研 Triton 内核**，训练栈 = HF/PEFT/TRL 原生算子 + Liger patching + `torch.nn.functional.scaled_dot_product_attention`；只有当 profile 证明"融合内核相关开销 >15% step time"或"logits 中间张量占用 >1.5GB"且 Liger 未覆盖时，才在 P1 补 1–2 个自研内核（优先 **fused linear cross-entropy（含 4-bit 反量化 + LoRA 旁路）** 与 **fused RMSNorm+residual**），并强制要求：native 回退路径、与 torch 参考实现的 `allclose` 单测、收敛对齐测试。**不要**自研 attention 或 autograd 框架。

### 3.5 国产加速卡分级支持策略（对应 C11「三级接口」）

| 级别 | 平台 | 训练现实（2025–2026） | 承诺口径 |
|---|---|---|---|
| L1 | NVIDIA CUDA | 完整：bnb 4-bit + Liger + SDPA/FlashAttention + packing | 全功能、性能门槛按 §7 |
| L2 | AMD ROCm | 基本完整：bnb 提供 CDNA/RDNA 支持，Liger 支持 triton ≥3.0 | 功能同 L1，性能门槛单独标定 |
| L3 | 昇腾 Ascend | `torch_npu` + `triton-ascend`（3.2.x/CANN 9.x，规划升级 Triton 3.5）；Liger 有 Ascend CI；**bnb 无 NPU 支持** | 承诺 **bf16 LoRA SFT + Liger**；QLoRA 仅作实验特性（HQQ/纯 torch NF4 兜底），不承诺性能 |
| L3 | 摩尔线程 MUSA | Torch-MUSA 2.x + Triton-MUSA + vLLM/SGLang-MUSA；bnb 无 MUSA 后端 | 同上（bf16 LoRA），需自建镜像与实测 |
| L3 | 沐曦 MACA / 寒武纪 MLU | 各自 `torch_maca` / `torch_mlu` + vLLM 插件；bnb 无官方后端 | 仅"接口预留 + 社区验证"，不进 P0/P1 依赖 |
| L0 | CPU | bnb CPU 后端可跑 QLoRA；极慢 | 仅 tiny-model CI 冒烟与正确性对照 |

落地方式：`llmgui_train` 启动时做**能力探测**（`torch.cuda.is_available` / `torch_npu` / `torch_musa` / `torch.mlu`，triton 是否可 import，bnb 是否可 import），输出 `capabilities`，据此选择 `{quant: bnb|hqq|none} × {kernels: liger|native} × {attn: sdpa|flash}` 组合；不支持的组合必须"显式报错并给出可执行建议"，禁止静默降级。

## 4. 显存预算：公式与算例

**假设**：bf16 激活；AdamW fp32 状态；LoRA r=16 打在 all-linear；GQA 模型（Qwen2.5 系配置，V=152064）；梯度累积不计入峰值；数值为**估算**（误差 ±20%），已与 Unsloth 官方最低显存表交叉校验。

```
M_total = M_w + M_lora + M_act + M_logits + M_ctx

M_w      = P · b_param · (1 + overhead)        # 权重；bnb NF4 另加未量化的 embedding 表
  b_param: bf16=2, int8=1, nf4=0.5, nf4+double-quant≈0.55 字节/参数
M_lora   = 12 · P_lora                          # bf16 参数2 + bf16 梯度2 + Adam m/v fp32 8
  P_lora = r · Σ_layers (in_dim + out_dim)
M_act    = b · s · h · 2 · k_act
  k_act ≈ L（开 full GC，每层只存 block 输入）| 8~12·L（不开 GC，注意力/MLP 中间量）
M_logits = b · s · V · bytes                    # 未融合 CE：bf16 logits + fp32 upcast，常被忽略的大头
  Liger FusedLinearCE 分块后 ≈ 0.1–0.3 GB
M_ctx    ≈ 0.8–1.5 GB（CUDA context、碎片、通信缓冲、allocator 预留）
```

模型配置与单卡结论（s=2048, b=1, GC 开, packing 开, Liger CE 开）：

| 模型 | P / L / h / ff | 权重 bf16 | 权重 NF4+embed | P_lora(r=16) | Adapter | M_act | 估算峰值 bf16 LoRA | 估算峰值 QLoRA |
|---|---|---|---|---|---|---|---|---|
| 7B | 7.6B / 28 / 3584 / 18944 | 15.2 GB | 5.3 GB | 42M | 0.5 GB | 0.4 GB | **~18 GB** | **~8 GB** |
| 14B | 14.8B / 48 / 5120 / 13824 | 29.6 GB | 9.7 GB | 77M | 0.9 GB | 1.0 GB | **~33 GB** | **~13 GB** |
| 32B | 32.8B / 64 / 5120 / 27648 | 65.6 GB | 19.6 GB | 134M | 1.6 GB | 1.3 GB | **~70 GB** | **~25 GB** |

可行配置建议（对齐需求 C5 验收机与开发机）：

| 显卡 | 7B | 14B | 32B |
|---|---|---|---|
| 24GB（4090/3090） | bf16 LoRA s=2048 b=1 ✅；QLoRA 可上 s=4096/b=2 | **QLoRA s=2048 b=1 ✅**；bf16 不可 | QLoRA 临界 ❌（需压到 s=1024/b=1 并留足上下文缓冲，不建议做产品承诺） |
| 32GB（5090D，本机） | bf16 LoRA s=4096 b=1 ✅ | QLoRA s=4096 b=1 ✅；bf16 LoRA s=1024 b=1 临界 ⚠️ | **QLoRA s=2048 b=1 ✅**（约 25GB，余量小，需 GC+packing） |
| 48GB（A6000/L40S） | bf16 s=8192 ✅ | **bf16 LoRA s=2048 b=1 ✅**（~33GB） | bf16 不可 ❌（70GB，需 80GB 卡或 FSDP/offload） |

交叉校验（Unsloth 官方最低显存表，QLoRA 4-bit / LoRA 16-bit）：7B 5/19 GB、14B 8.5/33 GB、32B 26/76 GB —— 与上表同量级，说明公式可用；我方预算普遍留 1–2GB 余量用于 Windows/WDDM 与碎片。**注意**：`s` 翻倍近似使 `M_act` 翻倍（GC 路径）或更多，`b` 同理；32B QLoRA 在 24GB 上不要做产品承诺。

## 5. 训练产物与 Rust 推理端交接

| 产物 | 格式 | 生产者 | 消费者 | 契约要点 |
|---|---|---|---|---|
| **PEFT adapter（权威）** | `adapter_model.safetensors` + `adapter_config.json` | `llmgui_train export adapter` | Rust runtime（热加载） | 记录 base model id+revision、`r/alpha/dropout/target_modules`、量化方案；运行时对每层算 `y = Wx + (alpha/r)·B(Ax)` |
| **merged 模型** | bf16 safetensors + `config.json` + tokenizer | `export merge`（`merge_and_unload`） | 二次量化/GGUF 转换 | 不覆盖原模型；输出独立目录 + sha256 |
| **GGUF（含 LoRA）** | `convert_hf_to_gguf.py`（合并模型）/ `convert_lora_to_gguf.py`（adapter）/ `llama-export-lora` 应用多条 LoRA | Python 侧调用 llama.cpp 工具 | llama.cpp / Ollama 路径 | 记录量化类型（Q4_K_M/IQ4_XS 等）与 llama.cpp 版本 |
| **训练 manifest** | JSON（`manifest.json`） | 训练结束时原子写出 | Rust 校验与 UI 展示 | schema 版本、dataset 指纹、超参、指标曲线、产物 sha256、环境指纹（torch/驱动/GPU） |
| **检查点** | 训练中间 `checkpoint-*/` | Trainer | 断点续训 | 仅 Python 内部使用，Rust 只透传路径 |

安全约束（对应 C10）：只接受 safetensors/GGUF/JSON，**禁止 pickle / `torch.load(weights_only=False)`**；Rust 侧加载 adapter 时做 tensor 名校验与形状校验，失败即拒绝热加载并保留旧 adapter（原子切换）。

## 6. Python 包结构与 IPC / 职责边界

```
python/llmgui_train/
  pyproject.toml            # uv 管理；extras: [cuda] [rocm] [ascend] [cpu] [dev]
  src/llmgui_train/
    cli.py                  # 人类入口，仅做参数解析 → 转 worker
    worker.py               # 进程入口：握手、事件循环、取消、退出码
    protocol.py             # 消息 schema（版本号 + 校验）
    capabilities.py         # 设备/后端/量化/内核能力探测
    estimate.py             # 与 §4 同源的显存预估（供 `llmgui estimate`）
    config.py               # 训练配置 dataclass + 校验
    data/ (formats.py, packing.py, masks.py)
    model/ (loader.py, quant.py, patching.py)
    kernels/ (liger.py, native.py, __init__.py)
    trainers/ (sft.py, dpo.py)
    export/ (adapter.py, merge.py, gguf.py)
    manifest.py, telemetry.py, errors.py
  tests/ (unit + tiny-model smoke + golden protocol fixtures)
```

**进程模型（v1）**：Rust `llmgui train` 用 `std::process::Command` 拉起 `python -m llmgui_train.worker`，**stdin/stdout 走 NDJSON 帧（JSON-RPC 2.0 风格）**，stderr 落日志文件；一次性 token 通过环境变量传入，worker 校验 `--rpc-stdin` 模式，不监听任何网络端口。升级路径：换成 Windows named pipe / Unix domain socket，协议不变。

| 消息 | 方向 | 说明 |
|---|---|---|
| `hello` / `hello_ok` | 双向 | 协议版本、包版本、capabilities、GPU 列表 |
| `estimate` / `estimate_ok` | R→P | 时长与显存预估（复用 §4 模型） |
| `train.start` | R→P | 配置 + 数据路径 + 输出目录 + resume 路径 |
| `event.progress` | P→R | step、loss、lr、tokens/s、VRAM 峰值、ETA |
| `event.checkpoint` / `event.log` | P→R | 检查点路径 / 结构化日志（Rust 落盘+展示） |
| `train.cancel` | R→P | 协作式取消（SIGINT/`CTRL_BREAK` 语义），保证 checkpoint 完整 |
| `export.merge` / `export.gguf` | R→P | 导出任务，产物路径写 manifest |
| `done` / `error` | P→R | 结构化错误码（OOM/无权限/数据格式/版本不兼容） |

职责边界：Rust 拥有 **CLI、GPU 排他锁、模型缓存、下载校验、HTTP 服务、adapter 热加载、UI/日志落盘**；Python 拥有 **数据预处理、显存探测、训练、导出**。二者只通过协议 + 落盘产物交互，Rust 不 import torch，Python 不监听端口（除非未来 `--serve` 模式）。孤儿进程：Rust 在父进程退出时 kill 子进程组；worker 通过 stdin EOF 检测父进程消失并自行退出。

## 7. 性能验收指标与消融实验

**指标定义（基线 = 同版本 PyTorch/PEFT/TRL、同数据同超参、无 Liger、无 packing、无自研内核）**

| 指标 | 定义 | 门槛（P0） |
|---|---|---|
| 训练吞吐 | `tokens/s`（有效 token，不含 padding），取 3 次中位数 | ≥1.6× 基线（C5） |
| 显存峰值 | `torch.cuda.max_memory_allocated` + `nvidia-smi` 双记录，取较大者 | ≤60% 基线；且 7B QLoRA 在 24GB 完成 1 epoch |
| 步时稳定性 | step time P95/P50 | ≤1.2 |
| 打包效率 | 有效 token / 总 token | ≥0.95（长短混合数据集） |
| 精度 | 固定 eval set 的 eval loss / 任务指标 | eval loss 相对基线偏差 ≤2%；无 NaN/发散 |
| 内核微基准 | Liger vs torch 的逐算子延迟与峰值显存 | RoPE/CE/RMSNorm 至少不劣化，CE 显存显著下降 |
| 恢复性 | 中断后 resume 到相同 step 的 loss | 差异在随机性容差内 |
| 平台冒烟 | L3（昇腾/MUSA）100 step 无 NaN、产物 manifest 完整 | 必须 |

**消融矩阵（做成表格化基准报告，纳入 nightly）**

| 编号 | 变量 | 取值 | 期望结论 |
|---|---|---|---|
| A1 | 融合内核 | none / Liger / Liger+FusedLinearCE | 量化 kernel 收益与显存下降 |
| A2 | packing | off / on（fixed seed，同 max_steps） | 短样本占比与吞吐关系，验证 loss 一致性 |
| A3 | gradient checkpointing | off / full / selective | 显存-吞吐权衡，确定默认值 |
| A4 | 量化 | bf16 / int8 / nf4(+double quant) | 精度-显存曲线，找到 24GB 最优 |
| A5 | 序列长度 | 1024 / 2048 / 4096 / 8192 | 显存斜率与 OOM 边界 |
| A6 | batch/累积 | b∈{1,2,4} × accum 互补 | 稳定吞吐的配置 |
| A7 | 注意力后端 | SDPA / FlashAttention(可用时) | Windows/驱动兼容性 |
| A8 | 自研内核（P1） | 开关 + `allclose` 对照 | 证明增量收益，否则删除 |

基准硬件与标注：RTX 5090D 32GB（sm_120，本机）、RTX 4090 24GB；数字必须带"硬件 + 版本 + seed + 数据"四元组，未实测的数字在文档中标注「估算」。CI 只跑 tiny-model（0.5B/1B，CPU 或 1 卡）+ 协议 golden 测试，性能回归放 nightly。

## 8. 风险与缓解

| 风险 | 影响 | 缓解 |
|---|---|---|
| Windows 上 Triton 非官方（`triton-windows` 社区维护） | Liger/自研内核在 Windows 可能不可用或崩溃 | 内核层可降级到 native；CI 双跑 with/without triton；首发文档明确支持矩阵 |
| transformers 5.x / TRL / PEFT 演进快 | patching API 漂移、Liger 与新版不兼容 | pin 精确版本 + 升级窗口；`capabilities` 探测版本并拒绝不兼容组合 |
| bnb 仅 Win11+ 官方支持，需求写 Win10/11 | Win10 用户无法 QLoRA | 启动检测 + 明确报错；bf16 LoRA 兜底；文档标注 |
| sm_120 等新架构 FlashAttention wheel 缺失 | 只能用 SDPA，吞吐略低 | 默认 SDPA，perf 达标即可；不把 FA 设为必需 |
| 国产卡承诺过度 | 交付失信 | 严格按 §3.5 分级，L3 只承诺 bf16 LoRA，QLoRA 标实验 |
| QLoRA 精度损失 | 微调效果差 | NF4 + double quant + 全 linear 目标；P1 评估分层混合精度；用 eval 而非 loss 判断 |
| 显存预算误差 | 线上 OOM | `estimate` 与训练前试运行；默认 GC + packing；OOM 自动降 batch 重试（可选） |
| 许可 | 合规 | 不复制 AGPL 代码；模型清单优先 Apache-2.0；产物中记录模型许可 |
| 数据安全 | 训练数据/权重泄露 | 本地 127.0.0.1、token、禁 pickle、日志脱敏 |

## 9. 参考链接

- Unsloth 仓库 README（2× faster / 70% less VRAM、支持矩阵、Apache-2.0 + AGPL-3.0 双许可）：https://github.com/unslothai/unsloth
- Unsloth「3× Faster LLM Training with Unsloth Kernels + Packing」（RoPE 融合、int64 索引、padding-free、packing 收益与 loss 一致性）：https://unsloth.ai/docs/blog/3x-faster-training-packing
- Unsloth「MoE 训练快 12 倍」（Split LoRA、`torch._grouped_mm`、bnb 不支持 MoE 4-bit）：https://unsloth.ai/docs/zh/ji-chu-zhi-shi/faster-moe
- Unsloth 微调指南（LoRA/QLoRA/FFT、动态 4-bit、精度建议）：https://unsloth.ai/docs/get-started/fine-tuning-llms-guide
- Unsloth 显存要求表（7B/14B/32B 最低 QLoRA/LoRA）：https://unsloth.ai/docs/get-started/fine-tuning-for-beginners/unsloth-requirements
- Liger-Kernel README（内核清单、20% 吞吐/60% 显存、DPO 系 chunked loss 最多 80%、Ascend NPU CI、BSD-2）：https://github.com/linkedin/Liger-Kernel
- Liger 技术报告：https://openreview.net/forum?id=36SjAIT42G
- bitsandbytes README（加速器支持矩阵、QLoRA、Win11/Server 2022+）：https://github.com/bitsandbytes-foundation/bitsandbytes
- PEFT 量化训练指南（bnb/torchao/HQQ/EETQ/GPTQ/AQLM/INC/TE 与 merge 限制）：https://github.com/huggingface/peft/blob/main/docs/source/developer_guides/quantization.md
- TRL README（SFT/DPO/GRPO Trainer、PEFT/Unsloth 集成、AMD 支持）：https://github.com/huggingface/trl
- Triton-Ascend（版本/CANN 兼容矩阵、2026 升级 Triton 3.5 计划）：https://github.com/triton-lang/triton-ascend
- 摩尔线程 Triton-MUSA 文档：https://docs.mthreads.com/musa-sdk/musa-sdk-doc-online/libraries/openlibs/kernel_dsl_compiler_stack/triton/
- 摩尔线程 Torch-MUSA 插件文档：https://docs.mthreads.com/torchmusa/torchmusa-doc-online/introduction
- triton-windows（Windows Wheel，社区维护）：https://pypi.org/project/triton-windows/
- safetensors 格式规范与 Rust crate（零拷贝、无 pickle、lazy loading）：https://github.com/huggingface/safetensors
- llama.cpp `export-lora`（多 LoRA 合并与缩放）：https://github.com/ggml-org/llama.cpp/tree/master/tools/export-lora
- mistral.rs README（Rust 侧 LoRA/X-LoRA 每请求切换、GGUF/GPTQ/AWQ/HQQ/BNB 量化）：https://github.com/EricLBuehler/mistral.rs
