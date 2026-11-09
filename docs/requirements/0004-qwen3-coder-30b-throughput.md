# 需求卡片：Qwen3-Coder-30B-A3B 单用户吞吐 90–120 tok/s

- 编号：0004（ADR 同号：`docs/adr/0004-qwen3-coder-30b-throughput.md`）
- 来源 / 提出人：用户（2026-11-09 会话）
- 日期：2026-11-09
- 关联 ADR：[0004-qwen3-coder-30b-throughput](../adr/0004-qwen3-coder-30b-throughput.md)（Proposed）

> 本卡片是需求原文与约束编号的唯一副本；ADR 只引用 C1..Cn，不复制原文。

## 需求原句（逐字）

> 请参考以下内容，完善项目，目标是实现Qwen3-Coder-30B吞吐达到 90–120token，：
> 模型：Qwen3-Coder-30B-A3B MoE Q4_K_M（或 EXL2 4.5bpw）
> 引擎：ExLlamaV2 + TabbyAPI（单用户）
> 或：llama.cpp 编译 CUDA+FA 版（稳）./llama-server \
>   -m model.q4_k_m.gguf \
>   -ngl 999 \
>   -fa \
>   -ctk q4_0 -ctv q4_0 \
>   -c 8192 \
>   --no-context-shift \
>   -t 8 -tb 8，exllamaV2配置max_seq_len: 16384
> cache_mode: Q4
> chunk_size: 2048
> fasttensors: true
> gpu_split: auto

## 可实施约束（C1..Cn）

| # | 需求句（原文片段） | 可实施约束 | 涉及模块 | 验收方式 |
|---|---|---|---|---|
| C1 | `模型：Qwen3-Coder-30B-A3B MoE Q4_K_M` | 推荐目录新增 `qwen3-coder-30b-a3b-q4km`：repo `unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF`、文件 `Qwen3-Coder-30B-A3B-Instruct-Q4_K_M.gguf`、体积 18,556,689,568 B、许可 Apache-2.0；GUI 静态目录同步 | `models/v0.toml`、`gui/app.js` | `llmgui models search coder` 可见；`pull` 后 `models show` 量化/体积正确 |
| C2 | `（或 EXL2 4.5bpw）` | EXL2 不进入 GGUF 推荐目录、下载器与 registry；作为 TabbyAPI 外部模型，仅提供接入与校验说明 | `docs/`、`crates/llmgui-backend-upstream` | 文档给出 EXL2 目录约定与 `/v1/models` 校验；无 EXL2 下载/登记代码 |
| C3 | `吞吐达到 90–120token` | 定义单用户 decode 吞吐口径：RTX 5090D 32GB、Q4_K_M、ctx 8192、输出 256 token、warmup 3、5 次中位数；实测 decode tok/s 落在 [90,120] 为达标；未实测数字标「估算」 | `llmgui-bench`、`llmgui-backend-api`、`docs` | `llmgui bench --profile turbo` 输出 `decode_tokens_per_second` 与区间判定，并归档原始 JSON |
| C4 | `引擎：ExLlamaV2 + TabbyAPI（单用户）` | 通过现有 `upstream` 后端接入 TabbyAPI 的 OpenAI 兼容 `/v1/completions` 与 `/health`；模型名可配置；单用户 `slots=1`；不新增执行后端 crate | `crates/llmgui-backend-upstream`、`crates/llmgui-cli` | TabbyAPI 启动后 `llmgui run/serve --backend upstream` 返回真实输出；健康检查与模型名生效 |
| C5 | `max_seq_len: 16384` / `cache_mode: Q4` / `chunk_size: 2048` / `fasttensors: true` / `gpu_split: auto` | 文档给出 TabbyAPI 配置样例并逐项解释；与 C3 的 8k 验收口径区分（TabbyAPI 可开 16k，报 8k 主口径与 16k 参考） | `docs/`、`README.md` | 按文档启动 TabbyAPI；`/health` 通过、`/v1/completions` 流式返回 |
| C6 | `llama.cpp 编译 CUDA+FA 版` + `-ngl 999 -fa -ctk q4_0 -ctv q4_0 -c 8192 --no-context-shift -t 8 -tb 8` | 新增 `turbo` 性能档：`ngl=999`、`flash_attn=true`、KV K/V 均 `q4_0`、`ctx=8192`、`n_batch=2048`、`n_ubatch=512`、`n_threads=8`、`n_threads_batch=8`；`--no-context-shift` 仅在外部 `llama-server` 启动参数中生效 | `llmgui-core`、`llmgui-backend-api`、`llmgui-backend-llamacpp`、`llmgui-cli`、`llmgui-server` | 单测校验 profile 展开与 LLamaContextParams 映射；`serve --profile turbo` 日志打印生效参数；WSL 启动命令含全部 flag |
| C7 | `完善项目` | 默认 `balanced` 行为不变；`turbo` 为显式 opt-in；配置、CLI、模型加载默认值、GUI、bench、文档与测试同轮落地 | `crates/`、`gui/`、`models/v0.toml`、`docs/` | `make flow` + `make check-docs` 全绿；GUI 可选 turbo 并回显生效参数 |

## 现状影响面（候选，待 ADR 核实）

| # | 疑似受影响能力 | 候选位置 | 备注 |
|---|---|---|---|
| 1 | 推荐模型目录 | `models/v0.toml:50`、`gui/app.js:87` | 只有 Qwen3-30B-A3B，没有 Coder-30B-A3B |
| 2 | 加载参数契约 | `crates/llmgui-backend-api/src/lib.rs:22`、`crates/llmgui-core/src/config.rs:69` | 无 profile / flash attention / ubatch / threads_batch |
| 3 | in-process GPU offload | `crates/llmgui-backend-llamacpp/src/real.rs:66` | `-1` 被映射为 0 层，默认走 CPU |
| 4 | in-process 上下文参数 | `crates/llmgui-backend-llamacpp/src/real.rs:91` | 无 FA、无 threads_batch，ubatch 与 batch 相同 |
| 5 | WSL llama-server 启动 | `crates/llmgui-cli/src/main.rs:718` | 只有 `-c/-t`，缺 `-ngl/-fa/-ctk/-ctv/-tb/--no-context-shift` |
| 6 | upstream/TabbyAPI | `crates/llmgui-backend-upstream/src/lib.rs:51` | OpenAI 兼容可复用；模型名硬编码 `local`，`cache_prompt` 未区分引擎 |
| 7 | 服务端加载路径 | `crates/llmgui-server/src/runtime.rs:224`、`crates/llmgui-server/src/admin.rs:444` | `n_batch=512`、`n_threads=None` 固定，无 profile 入参 |
| 8 | 基准口径 | `crates/llmgui-bench/src/lib.rs:33` | 只有并发聚合，没有 decode tok/s 与目标区间判定 |
| 9 | GUI 模型页 | `gui/app.js:12`、`gui/app.js:1020` | 静态目录与加载默认值需同步 |
| 10 | 文档 | `README.md:83` | 无 Coder-30B 与 TabbyAPI 性能档说明 |

## 未决问题

- [x] 引擎支持范围：llama.cpp 主 + TabbyAPI 外接（用户 2026-11-09 确认）。
- [x] 验收口径：单用户 decode @ ctx 8192（用户 2026-11-09 确认）。
- [x] 性能开关落点：新增 `turbo` 性能档，配置 / CLI / 模型默认值共用（用户 2026-11-09 确认）。
- [x] EXL2 是否纳入模型管理：不纳入，仅作为 TabbyAPI 外部模型接入（与 ADR 0001 的 GGUF 口径一致）。
- [x] 目标硬件：沿用项目验收机 RTX 5090D 32GB；其他硬件只记录实测，不设门槛。

## 卡点

- [x] 每条需求句都有 C 编号约束（无遗漏、无「待定」）。
- [x] 未决问题全部有结论，需求方确认（2026-11-09 AskUserQuestion）。
