# ADR 0004：Qwen3-Coder-30B-A3B 单用户吞吐 90–120 tok/s

- 状态：Accepted（2026-11-09 需求方确认范围与验收口径，进入分阶段实现）
- 日期：2026-11-09
- 关联需求卡片：[0004-qwen3-coder-30b-throughput](../requirements/0004-qwen3-coder-30b-throughput.md)
- 关联调研：[runtime](../research/runtime.md) · [model-management](../research/model-management.md)
- 决策摘要：以 GGUF + llama.cpp CUDA+FA 为主路径，新增显式 `turbo` 性能档（ngl 999 / FA / KV q4_0 / batch 2048 / ubatch 512 / threads 8）；ExLlamaV2 + TabbyAPI 作为 OpenAI 兼容 `upstream` 外接，不纳入模型下载与 registry；用单用户 decode tok/s 判定验收（5 次中位数 ≥90 通过，120 为参考上限）。

## 1. 背景与问题

用户要求把 Qwen3-Coder-30B-A3B MoE 的单用户吞吐做到 90–120 tok/s，并给出两条参考路径：llama.cpp CUDA+FA 的 `llama-server` 参数，以及 ExLlamaV2 + TabbyAPI 的 EXL2 配置。

当前项目的真实推理路径存在三个直接缺口：

1. **默认不利用 GPU**：in-process 后端把 `n_gpu_layers = -1` 解释为 0 层，默认走 CPU，无法达到目标吞吐。
2. **缺少性能档**：`LoadConfig` 没有 flash attention、`n_ubatch`、`n_threads_batch` 等字段；`--start-wsl` 生成的 `llama-server` 命令只有 `-c/-t`，没有 `-ngl/-fa/-ctk/-ctv/-tb/--no-context-shift`。
3. **无法证明目标**：bench 只输出并发聚合吞吐，没有单流 decode tok/s，也没有 [90,120] 的目标判定；upstream 的模型名硬编码为 `local`，接 TabbyAPI 时不可配置。

同时，项目已有 `upstream` 后端可接任何 OpenAI 兼容服务，因此 TabbyAPI 不需要新的执行后端；EXL2 与 GGUF 的模型管理口径不同，若把 EXL2 纳入 registry/CAS 会扩大范围并破坏 ADR 0001 的「GGUF 为推理唯一格式」约定。

## 2. 现状核对

| # | 能力 | 现状 / 候选位置 | 判定 | 备注 |
|---|---|---|---|---|
| 1 | 推荐模型目录 | `models/v0.toml:50-57`、`gui/app.js:87-97` | 新增 | 只有 `Qwen3-30B-A3B`，没有 Coder-30B-A3B |
| 2 | 加载参数契约 | `crates/llmgui-backend-api/src/lib.rs:22-43`、`crates/llmgui-core/src/config.rs:69-97` | 新增 | 无 profile / FA / ubatch / threads_batch |
| 3 | GPU offload 语义 | `crates/llmgui-backend-llamacpp/src/real.rs:66-73` | 排除 | `-1` 被映射为 0 层，CPU 路径无法达标 |
| 4 | in-process 上下文参数 | `crates/llmgui-backend-llamacpp/src/real.rs:91-108` | 新增 | 仅 type_k/type_v/n_batch；ubatch 等于 batch，无 FA |
| 5 | WSL llama-server 启动 | `crates/llmgui-cli/src/main.rs:718-736` | 修改 | 缺 `-ngl/-fa/-ctk/-ctv/-tb/--no-context-shift` |
| 6 | upstream / TabbyAPI | `crates/llmgui-backend-upstream/src/lib.rs:51-123` | 确认 + 修改 | OpenAI 兼容可复用；模型名硬编码 `local`，`cache_prompt` 未按引擎区分 |
| 7 | 服务端加载路径 | `crates/llmgui-server/src/runtime.rs:224-231`、`crates/llmgui-server/src/admin.rs:444-452` | 修改 | `n_batch=512`、`n_threads=None` 固定，LoadRequest 无 profile |
| 8 | 基准口径 | `crates/llmgui-bench/src/lib.rs:33-107` | 修改 | 只有并发聚合，无 decode tok/s 与目标区间 |
| 9 | GUI 模型页 | `gui/app.js:12-198`、`gui/app.js:1020-1130` | 修改 | 静态目录与加载默认值需同步 profile |
| 10 | 文档 | `README.md:83-128` | 新增 | 无 Coder-30B 性能档与 TabbyAPI 接入说明 |
| 11 | llama.cpp 能力 | `llama-cpp-2 0.1.159` 的 `with_flash_attention_policy` / `with_n_ubatch` / `with_n_threads_batch` / `with_offload_kqv` | 确认 | 所需原语均已暴露，无需改 FFI |
| 12 | 模型清单实测 | ModelScope `unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF` | 确认 | `Qwen3-Coder-30B-A3B-Instruct-Q4_K_M.gguf` = 18,556,689,568 B |

## 3. 目标与非目标

### 3.1 目标

- G1：推荐目录与 GUI 静态目录新增 `qwen3-coder-30b-a3b-q4km`，可用现有 `pull` 下载并入库。
- G2：新增 `turbo` 性能档，一条命令把 C6 的参数展开到 in-process 与外部 `llama-server` 两条路径。
- G3：预算内显存可加载（32GB 卡，ctx 8192，KV q4_0），加载前显存闸门使用同一套 profile 参数。
- G4：`bench` 输出单用户 decode tok/s，并对 `>= target_min`（90）给出 pass/fail 与原始 JSON；`target_max`（120）仅作参考上限。
- G5：TabbyAPI 可经 `upstream` 接入，模型名、健康检查与 `cache_prompt` 行为可配置；EXL2 不进入本项目模型管理。
- G6：默认 `balanced` 行为不变，全部改动有单测与文档。

### 3.2 非目标

- N1：不新增 ExLlamaV2/TabbyAPI 专用后端 crate，不实现 EXL2 下载、登记、量化或加载。
- N2：不 fork llama.cpp、不自研 Attention/GEMM 内核。
- N3：不改变默认 `balanced` 的 CPU-safe 语义；`turbo` 必须显式开启。
- N4：不承诺 5090D 之外的硬件达标；其他 GPU/CPU 只记录实测。
- N5：不追求多用户高并发；本目标固定 `slots=1`（单用户）。
- N6：不写没有「硬件 + 版本 + 参数 + 原始数据」四元组的性能数字。

## 4. 口径映射（C1..C7）

| 约束 | 设计落点 |
|---|---|
| C1 | §7.6 推荐目录、§8.1 模型条目 |
| C2 | §7.5 EXL2 边界、§9 兼容策略 |
| C3 | §7.7 基准测量、§13 性能预算 |
| C4 | §7.5 upstream/TabbyAPI、§8.4 环境变量 |
| C5 | §7.5 TabbyAPI 配置样例、§13 验收条件 |
| C6 | §7.2 profile 展开、§7.3 in-process 参数、§7.4 server 命令生成 |
| C7 | §8 契约、§11 测试、§12 分期 |

## 5. 不变量

- I1：默认 `balanced` 的加载参数与输出行为不变；只有显式 `profile = "turbo"` 或 `--profile turbo` 才切换。
- I2：`profile` 是完整预设，设置后覆盖同组显式字段；需要微调时用 `custom` + 显式字段；两者同时出现且冲突时启动期告警。
- I3：KV 量化（`q4_0`/`q8_0`）必须启用 flash attention；不满足时加载前失败并给出「启用 FA 或改 f16」的建议。
- I4：`--no-context-shift` 只属于外部 `llama-server` 启动参数；in-process 每请求新建 context，不做 context shift。
- I5：EXL2/TabbyAPI 的模型文件与进程不由本项目管理；本项目只通过 OpenAI 兼容 HTTP 调用，不写入 registry/CAS。
- I6：性能声明必须附四元组；没有实测的目标值标「目标」或「估算」，不得写成已达成。
- I7：upstream 默认只允许回环地址；远程地址沿用 `server.allow_remote` 的显式开关。
- I8：API/配置新增字段只增不改；旧配置、旧客户端与旧 registry 仍可加载。

## 6. 总体架构

```text
CLI / GUI / admin API
        │  profile = balanced | turbo | custom
        ▼
llmgui-core::config ──► ProfileParams（预设展开）
        │
        ▼
llmgui-server::runtime ──► LoadConfig（effective）
        │                         │
        │                         ├─► llama.cpp in-process（CUDA + FA）
        │                         └─► upstream（OpenAI 兼容）
        │                                   ├─ llama-server（本项目可拉起）
        │                                   └─ TabbyAPI / ExLlamaV2（外部进程）
        ▼
llmgui-bench：单用户 decode tok/s + [90,120] 判定
```

数据流：

1. `pull` 下载 Qwen3-Coder-30B-A3B Q4_K_M 并写入 registry。
2. `serve --profile turbo` 解析为 effective `LoadConfig`，加载前按同一参数估算显存。
3. in-process 用 FA + KV q4_0 解码；或 `--backend upstream` 调 TabbyAPI/llama-server。
4. `bench --profile turbo` 采集单流 decode 时间戳，输出报告与目标判定。

## 7. 关键设计

### 7.1 引擎边界

- **主路径**：GGUF + llama.cpp CUDA+FA，沿用 ADR 0001 D2 与现有 in-process/upstream 两条实现。
- **外接路径**：ExLlamaV2 + TabbyAPI 通过 `upstream` 接入。TabbyAPI 暴露 OpenAI 兼容 `/v1/completions`，不需要新 crate；本项目不负责 EXL2 的下载、登记与加载。
- **选择理由**：GGUF 路径已具备下载、manifest、显存预估、bench 与 GUI；TabbyAPI 只增加一个 URL 与模型名配置，改动面最小，且不破坏 ADR 0001 的格式契约。

### 7.2 `turbo` 性能档

`profile` 取值：`custom`（默认，沿用现有字段）、`balanced`（与默认等价）、`turbo`。

| 参数 | `balanced` / `custom` 默认 | `turbo` | 说明 |
|---|---|---|---|
| `ctx` | 32768 | 8192 | 用户给定 `-c 8192` |
| `kv_type` | q8_0 | q4_0 | K/V 均为 q4_0 |
| `gpu_layers` | -1（沿用旧语义：CPU-safe） | 999 | 全量 offload |
| `flash_attn` | false / auto | true | 量化 KV 需要 FA |
| `n_batch` | 512 | 2048 | prefill 批 |
| `n_ubatch` | 等于 `n_batch` | 512 | decode/计算微批 |
| `n_threads` | None（运行时默认） | 8 | 用户给定 `-t 8` |
| `n_threads_batch` | None | 8 | 用户给定 `-tb 8` |
| `slots` | 1 | 1 | 单用户 |
| `no_context_shift` | true（in-process 天然无 shift） | true | 仅外部 server 需要显式 flag |

展开规则：

1. `profile=custom`：完全使用显式字段（现状）。
2. `profile=balanced`：等价于 `custom` + 现有默认值。
3. `profile=turbo`：使用上表预设，覆盖同组显式字段；若同时显式设置了非默认字段，启动日志告警一次。
4. CLI `--profile` 覆盖配置文件里的 `profile`；CLI 的 `--ctx` / `--slots` 在档位展开之后生效，可直接微调 preset。

### 7.3 in-process llama.cpp

`LoadConfig` 增加 `flash_attn: FlashAttention`（`auto` / `enabled` / `disabled`，默认 `auto`）、`n_ubatch: u32`、`n_threads_batch: Option<i32>`，映射到：

- `LlamaModelParams::with_n_gpu_layers(999)`（turbo）。
- `LlamaContextParams::with_flash_attention_policy(...)`（turbo 为 `ENABLED`，custom 默认 `AUTO`）。
- `with_n_batch(2048)`、`with_n_ubatch(512)`。
- `with_n_threads(8)`、`with_n_threads_batch(8)`。
- `with_type_k(Q4_0)`、`with_type_v(Q4_0)`。

`auto` 保留既有行为：llama.cpp 在量化 V cache 下自动启用 FA。加载前校验仅拦截显式 `disabled` + 量化 KV 的组合，并提示「启用 turbo 或把 kv 改为 f16」。加载成功后用 `tracing::info!` 打印 effective 参数（profile、ngl、FA、KV、batch、ubatch、threads），便于验收。

### 7.4 外部 `llama-server` 命令生成

`--start-wsl` 与后续的本地 server 启动统一由参数构造器生成，禁止拼接 shell 字符串（逐参数传 `Command`，避免注入）。turbo 生成：

```text
llama-server -m <model> --host 127.0.0.1 --port <port> \
  -ngl 999 -fa -ctk q4_0 -ctv q4_0 -c 8192 \
  --no-context-shift -t 8 -tb 8 -b 2048 -ub 512
```

`balanced` 保持现有 `-c <ctx> -t 8`，不额外加 FA/KV 参数。

### 7.5 upstream / TabbyAPI

- `UpstreamBackend` 增加可配置的模型名：优先 `LLMGUI_UPSTREAM_MODEL`，其次 CLI/config；默认 `local` 保持兼容。
- 增加 `LLMGUI_UPSTREAM_KIND=llama-server|tabbyapi|generic`（默认 `generic`）：仅 `llama-server` 发送 `cache_prompt: true`；`tabbyapi` 不发送该非标字段。
- 健康检查保持 `/health`；TabbyAPI 与 llama-server 均提供该端点。
- TabbyAPI 参考配置（文档给出，不写入项目配置）：

```yaml
model:
  max_seq_len: 16384
  cache_mode: Q4
  chunk_size: 2048
  fasttensors: true
  gpu_split: auto
```

- 单用户：`--slots 1`；不承诺 TabbyAPI 的并发批处理。
- EXL2 模型目录、下载与校验由用户自行通过 TabbyAPI 管理；本项目只验证 `/v1/models` 与一次流式生成。

### 7.6 推荐模型目录

新增条目：

```toml
[[models]]
id = "qwen3-coder-30b-a3b-q4km"
repo = "unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF"
file = "Qwen3-Coder-30B-A3B-Instruct-Q4_K_M.gguf"
quant = "Q4_K_M"
license = "Apache-2.0"
size_bytes = 18556689568
role = "coder"
```

`gui/app.js` 静态目录同步同一字段；`role = "coder"` 复用现有中文标签。EXL2 4.5bpw 不加入该目录。

### 7.7 基准测量与判定

`GenerateStats` 增加：

- `decode_ms`：`total_ms - ttft_ms`（首个 token 之后的耗时）。
- `decode_tokens_per_second`：`(completion_tokens - 1) / decode_ms`，仅在 `decode_ms > 0` 时计算。

`BenchReport` 增加 `median_decode_tps`、`decode_samples`、`target_min`、`target_max`、`target_pass`。判定条件：

- `concurrency = 1`、`iterations >= 5`、`warmup` 次不计入（默认 3）。
- 测量批次按 `concurrency` 分批执行，`concurrency = 1` 时严格串行。
- 取 5 次 `decode_tokens_per_second` 的中位数。
- 中位数 `>= target_min`（90）为 pass；`target_max`（120）是参考上限，超出记为余量而非失败；未达标时输出实测值与差距，不自动调参。

upstream 的 token 计数优先使用最终 `usage.completion_tokens`，缺失时按 SSE delta 计数并在报告中标记 `counted=delta`。

## 8. 数据与接口契约

### 8.1 配置（TOML）

```toml
[model]
profile = "turbo"          # custom | balanced | turbo
# profile = "custom" 时可显式覆盖：
# ctx = 8192
# kv_type = "q4_0"
# gpu_layers = 999
# n_batch = 2048
# n_ubatch = 512
# n_threads = 8
# n_threads_batch = 8
# flash_attn = "enabled"   # auto | enabled | disabled
```

### 8.2 CLI

```text
llmgui serve --profile turbo --model <gguf>
llmgui serve --backend upstream --profile turbo --start-wsl --model <gguf>
llmgui bench --profile turbo --iterations 5 --concurrency 1 --max-tokens 256 \
  --target-min 90 --target-max 120
```

### 8.3 管理端加载请求

```json
{
  "profile": "turbo",
  "ctx": null,
  "kv": null,
  "gpu_layers": null,
  "slots": 1
}
```

响应回显 effective 参数：`profile`、`ctx`、`kv`、`gpu_layers`、`flash_attn`、`n_batch`、`n_ubatch`、`n_threads`、`n_threads_batch`。

### 8.4 环境变量

| 变量 | 默认 | 说明 |
|---|---|---|
| `LLMGUI_UPSTREAM_URL` | `http://127.0.0.1:18080` | 现有；TabbyAPI 可指向其端口 |
| `LLMGUI_UPSTREAM_MODEL` | `local` | 新增；TabbyAPI 已加载模型名 |
| `LLMGUI_UPSTREAM_KIND` | `generic` | 新增；`llama-server` 才发 `cache_prompt` |

## 9. 兼容与迁移

- `profile` 缺省为 `custom`，旧配置解析后行为与当前一致。
- `LoadConfig` 新字段均有 serde 默认值；旧 registry 无 `profile` 字段时按 `custom` 处理。
- `--start-wsl` 在 `balanced/custom` 下生成的命令与当前一致；只有 `turbo` 追加参数。
- 不迁移 EXL2 模型；不改变 CAS、manifest、registry schema。
- 回滚：删除 `profile` 配置或改用 `custom` 即回到旧行为。

## 10. 安全与合规

| 威胁 | 控制 |
|---|---|
| 外部 server 命令注入 | 参数构造器逐参数传 `Command`，不做 shell 字符串拼接 |
| upstream 被指向远程地址 | 默认只允许回环；远程沿用 `server.allow_remote` 显式开关 |
| TabbyAPI token/密钥泄露 | 不写入配置与日志；通过环境变量注入，日志脱敏 |
| EXL2 文件与许可 | 不由本项目下载/分发；文档提示用户自行确认模型许可 |
| 性能数字误导 | 未实测标「目标/估算」；报告保留原始 JSON 与参数 |

## 11. 测试与质量门禁

- 单测：profile 展开（custom/balanced/turbo）、CLI 覆盖配置、显式字段冲突告警、`kv q4_0 + FA off` 报错。
- 单测：`LoadConfig` → `LlamaContextParams` 映射（FA、batch、ubatch、threads、KV 类型）。
- 单测：`llama-server` 参数构造器（turbo 含全部 flag；balanced 不含新增 flag）。
- 单测：`GenerateStats` 的 `decode_ms` / `decode_tokens_per_second` 边界（0 token、0 耗时、TTFT 缺失）。
- 单测：upstream 模型名与 `cache_prompt` 按 `kind` 发送；最终 `usage` 计数优先。
- 集成：`bench --concurrency 1 --iterations 5` 的报告字段与 pass/fail。
- 手工验收（5090D 32GB）：Qwen3-Coder-30B-A3B Q4_K_M + turbo，记录 5 次 decode tok/s、显存峰值、llama.cpp commit、CUDA 版本与原始 JSON。
- 门禁：`make flow`、`make check-docs`；性能门槛只在带 GPU 的验收机执行，不阻塞无 GPU CI。

## 12. 分期计划

| 期 | 工作 | 交付 / 退出标准 |
|---|---|---|
| P0 | 推荐目录、turbo profile、`LoadConfig` 扩展、in-process FA/ubatch/threads 映射、显存闸门复用、单测 | `serve --profile turbo` 打印 effective 参数；单测全绿；默认 balanced 行为不变 |
| P1 | `llama-server` 参数构造器、upstream 模型名/kind、bench decode 口径与目标判定、TabbyAPI 文档 | turbo 启动命令含全部 flag；bench 输出 `target_pass`；TabbyAPI 流式生成通过 |
| P2 | GUI profile 选择与回显、README/文档收尾、5090D 实测报告归档 | GUI 可切换 turbo；文档含实测四元组；`make flow` 全绿；中位数 232.5 tok/s |

## 13. 性能预算

基准条件（2026-11-09 实测，见 §13.1）：RTX 5090D 32GB、CUDA 12.8+、Qwen3-Coder-30B-A3B-Instruct Q4_K_M、ctx 8192、KV q4_0、FA on、ngl 999、`slots=1`、输出 256 token、warmup 3、5 次中位数。

| 指标 | 目标 | 口径 |
|---|---|---|
| 单用户 decode | **≥90 tok/s**（120 为参考上限；实测 232.5） | 首 token 之后按 `decode_tokens_per_second` 计算 |
| TTFT @ 512 prompt | 记录实测，不设门槛 | 含 tokenize + prefill |
| 显存峰值 | ≤ 32GB，目标 ≤ 24GB | NVML 峰值 |
| 16k ctx 参考 | 记录实测 | TabbyAPI `max_seq_len: 16384`；不作为主门槛 |

显存估算（Qwen3-Coder-30B-A3B：48 层、4 个 KV head、head_dim 128，按 GGUF metadata 复核）：

| 项 | 估算 |
|---|---|
| 权重 Q4_K_M | 18.56GB（ModelScope 实测体积） |
| KV q4_0 @ 8192 | ≈0.20GB（估算） |
| KV q4_0 @ 16384 | ≈0.40GB（估算） |
| compute buffer | ≈1.5–2.5GB（估算） |
| runtime/context | ≈0.6GB |
| 合计 @ 8192 | ≈21–22GB（估算，以 `llmgui estimate` 为准） |

> 估算行为预算；实测结果见 §13.1，原始 JSON 在 [docs/bench/0004-qwen3-coder-30b-turbo.json](../bench/0004-qwen3-coder-30b-turbo.json)。

### 13.1 实测结果（2026-11-09）

环境：RTX 5090D 32GB、驱动 616.56、Windows + WSL2 `networkingMode=mirrored`；引擎为
LM Studio 自带 `llama-server.exe`（CUDA12 build 2.53.0，llama.cpp commit `3109914`），参数
`-ngl 999 -fa on -ctk q4_0 -ctv q4_0 -c 8192 --no-context-shift -b 2048 -ub 512 -t 8 -tb 8`。

| 项 | 实测 |
|---|---|
| decode tok/s（5 次中位数） | **232.5**（227.5 / 230.1 / 232.5 / 235.2 / 236.3） |
| TTFT | 22–51 ms |
| 显存峰值 | 25,751 MiB（整卡口径，含桌面约 4 GiB） |
| `target_pass` | true（`target_min=90`） |

冷启动首轮中位数 240.6（样本 134.3 / 161.2 / 240.6 / 245.2 / 253.2），第二轮回落后稳定在
227–236。原始 JSON 与复现命令见 [docs/bench/0004-qwen3-coder-30b-turbo.md](../bench/0004-qwen3-coder-30b-turbo.md)。

> 引擎为第三方（LM Studio）Windows CUDA 构建，不是本项目自编译的 Linux llama.cpp；
> 结论对同参数同硬件成立，自编译构建仍需复验。

## 14. 风险与缓解

| # | 风险 | 影响 | 缓解 |
|---|---|---|---|
| 1 | sm_120 上 llama.cpp CUDA FA 内核覆盖不足 | turbo 无法加载或回退 | 用目标机实测；失败时记录并回退 q8_0 KV/无 FA；不静默降级 |
| 2 | MoE 30B-A3B 在当前 llama.cpp 版本吞吐不足 | 达不到 90 tok/s | 使用最新稳定 llama.cpp 与 CUDA 构建；只记录实测，不 fork |
| 3 | TabbyAPI 的 OpenAI 兼容字段漂移 | upstream 请求失败 | `LLMGUI_UPSTREAM_KIND` 控制非标字段；未知字段不发送；以 `/health` 与一次生成做冒烟 |
| 4 | KV q4_0 长上下文质量下降 | 代码补全质量受损 | 保留 `custom` + q8_0；长上下文 ppl 回归通过后再默认 turbo |
| 5 | upstream SSE 计数偏差 | 吞吐报告偏低/偏高 | 优先 `usage.completion_tokens`；报告标记计数来源 |
| 6 | profile 与显式字段冲突 | 用户误以为生效 | 启动日志打印 effective 参数并告警冲突 |
| 7 | 只有 5090D 验收机 | 其他硬件数字不可比 | 目标只对 5090D 承诺；其他硬件记录实测与配置 |
| 8 | `turbo` 全量 offload 导致小显存 OOM | 加载失败 | 加载前按 profile 估算显存并拒绝；提示降 ctx/换 q8_0/减少 ngl |

## 15. 决策记录

| 编号 | 议题 | 结论 | 依据 | 被否决项 | 落点 |
|---|---|---|---|---|---|
| D1 | 引擎范围 | llama.cpp CUDA+FA 为主，ExLlamaV2/TabbyAPI 经 `upstream` 外接 | 复用现有 GGUF/下载/预估/bench；改动最小 | 新增 EXL2 后端、EXL2 纳入 registry | §7.1 |
| D2 | 默认性能档 | 默认 `custom/balanced` 不变，`turbo` 显式开启 | 保护现有 CPU/低显存用户 | 全局默认 turbo | §7.2 |
| D3 | 量化 KV | turbo 用 q4_0 + FA；显式 `disabled` 时拒绝量化 KV，`auto` 保留 llama.cpp 自动启用 | 兼顾性能与旧配置兼容 | q4_0 + 强制关闭 FA、默认 f16 | §7.3 |
| D4 | GPU offload | turbo 显式 `ngl=999`；`balanced` 保持旧语义 | 避免改变默认行为 | 全局把 -1 改成全量 | §7.2/§7.3 |
| D5 | TabbyAPI 兼容 | 模型名可配置；`cache_prompt` 仅 llama-server kind 发送 | 兼容两家 API 差异 | 硬编码 `local`、对所有引擎发非标字段 | §7.5 |
| D6 | 吞吐口径 | 单流 decode（排除 TTFT）+ 5 次中位数 + ≥90 判定（120 为参考上限） | 与「单用户」目标一致 | 用含 prefill 的 wall-clock 吞吐 | §7.7 |
| D7 | 验收硬件 | RTX 5090D 32GB、ctx 8192、输出 256 | 项目既有验收机与用户参数 | 对所有 GPU 承诺 | §13 |
| D8 | EXL2 管理 | 不纳入本项目下载/登记/加载 | 保持 GGUF 唯一推理格式 | 双格式模型管理 | §7.5/§9 |

## 16. 后果

**正面**：一条命令把用户给定的 llama.cpp 参数落地；默认行为不被破坏；TabbyAPI 零新增后端即可接入；bench 能直接证明是否达到 ≥90 tok/s（实测 232.5）。

**负面**：`turbo` 与 `balanced` 的参数差异需要文档解释；q4_0 KV 有质量风险；目标硬件之外无法承诺。

**中性**：ExLlamaV2 仍由外部进程管理，本项目只做 OpenAI 兼容调用；若未来要一等公民支持 EXL2，需另立 ADR。

## 17. 与既有 ADR 的关系

- ADR 0001：延续 D2（llama.cpp 执行后端）、D5（Q4_K_M 默认）、D16（`upstream` 保底路径）；不改变 GGUF 唯一推理格式。
- ADR 0002：复用模型库/发现/加载默认值；新增 profile 字段为向后兼容扩展。

## 18. 落地状态

| 期 | 状态 | 代码落点 | 测试 | 提交 |
|---|---|---|---|---|
| P0 | 已落地 | `models/v0.toml`、`gui/app.js`、`crates/llmgui-core/src/config.rs`、`crates/llmgui-backend-api/src/lib.rs`、`crates/llmgui-models/src/library.rs`、`crates/llmgui-server/src/runtime.rs`、`crates/llmgui-server/src/admin.rs`、`crates/llmgui-backend-llamacpp/src/real.rs`、`crates/llmgui-cli/src/main.rs` | `cargo test -p llmgui-core -p llmgui-backend-api -p llmgui-models -p llmgui-server`（profile/映射/加载） | — |
| P1 | 已落地 | `crates/llmgui-cli/src/main.rs`（`llama_server_args` / bench）、`crates/llmgui-backend-upstream/src/lib.rs`、`crates/llmgui-backend-api/src/lib.rs`、`crates/llmgui-bench/src/lib.rs`、`README.md` | `cargo test --workspace`（参数构造、decode 统计、upstream kind） | — |
| P2a | 已落地 | `gui/app.js`（性能档下拉 + 加载/默认值透传）、`README.md`、`gui/README.md`、`CHANGELOG.md` | `node --check gui/app.js`、`make check-docs` | — |
| P2b | 已落地 | `docs/bench/0004-qwen3-coder-30b-turbo.md`、`docs/bench/0004-qwen3-coder-30b-turbo.json` | `make bench-turbo`（5090D，中位数 232.5 tok/s，`target_pass=true`；冷启动首轮 240.6） | — |
| P3 | 不适用 | — | — | — |

## 19. 交接

- 当前阶段：P0 / P1 / P2a / P2b 全部落地，无待办。
- 已完成：需求卡片 0004；推荐目录新增 Coder-30B；`RuntimeProfile`/`FlashAttention`/`ProfileParams` 与 `LoadDefaults.profile_params()`；`LoadConfig` 的 FA/ubatch/threads_batch；in-process 映射与量化 KV 校验；CLI `--profile`；admin load/defaults 接受 profile；`llama-server` turbo 参数构造器（`-fa on`）；upstream 模型名与 `LLMGUI_UPSTREAM_KIND`；bench 分批执行、warmup、`median_decode_tps` 与 ≥90 判定；TabbyAPI 文档；`make llama-server-turbo` / `bench-turbo` / `serve-turbo`；5090D 实测归档（中位数 232.5 tok/s）。
- 下一步：无。若要复验，用自编译 Linux CUDA llama.cpp 重跑 `make bench-turbo`，对比本 ADR §13.1 的 232.5 tok/s。
- 已知坑：Windows in-process `llama_backend_init()` 仍可能崩溃（ADR 0001 §18），Windows 验收优先走 `upstream`/WSL；本环境缺 libclang，`--features llama` 未能本地编译验证；§13.1 的引擎是 LM Studio 随包的 Windows CUDA 构建，不是本项目自编译产物；TabbyAPI 不支持 GGUF，EXL2 模型需用户自备；新版 llama.cpp 的 `-fa` 必须带 `on|off|auto`，裸 `-fa` 会吞掉下一个 flag。

## 20. 参考

- 需求卡片：[0004-qwen3-coder-30b-throughput](../requirements/0004-qwen3-coder-30b-throughput.md)
- ADR：[0001-local-llm-runtime-finetune](0001-local-llm-runtime-finetune.md) · [0002-model-management](0002-model-management.md)
- 调研：[runtime](../research/runtime.md) · [model-management](../research/model-management.md)
- 模型文件（ModelScope）：`unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF`，`Qwen3-Coder-30B-A3B-Instruct-Q4_K_M.gguf`，18,556,689,568 B
- llama.cpp server：https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md
- TabbyAPI：https://github.com/theroyallab/tabbyAPI
- ExLlamaV2：https://github.com/turboderp-org/exllamav2
- Qwen3-Coder-30B-A3B-Instruct：https://huggingface.co/Qwen/Qwen3-Coder-30B-A3B-Instruct
