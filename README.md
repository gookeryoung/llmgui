# llmgui

本地大模型「运行 + 微调」框架：Rust 运行时 + Python 训练层。方案与阶段边界见
[ADR 0001](docs/adr/0001-local-llm-runtime-finetune.md)，当前实现状态见
[ADR §18 落地状态](docs/adr/0001-local-llm-runtime-finetune.md) 与 [CHANGELOG](CHANGELOG.md)。

## 已实现

- **下载**：ModelScope 主源、hf-mirror / Gitee AI 兼容源；分片 Range、断点续传、
  流式 SHA256、CAS 缓存、`models/<id>/manifest.json`。
- **运行时**：Rust 单进程；OpenAI 兼容 `/v1/chat/completions`（SSE）、`/v1/completions`、
  `/v1/models`、`/healthz`、`/metrics`；本地 Web GUI；`mock` 与 `upstream` 后端开箱即用。
- **模型**：GGUF metadata 解析、显存预估；`llama.cpp` in-process 后端（`--features llama`，
  当前 Windows 环境存在运行期阻塞，见下）；`upstream` 后端可接本地 llama-server。
- **微调**：Python worker（stdio JSON-RPC）、SFT/QLoRA trainer（lazy 依赖）、数据格式/
  assistant mask/packing、PEFT adapter 与 GGUF 导出封装。
- **桌面壳**：`llmgui-desktop`（Tauri 2）拉起 `llmgui serve` 并复用 `gui/`；`--selftest` 在真实
  WebView 里驱动 UI 做端到端验收（7B merged-7b-q4km 已跑通）。
- **工程**：Rust workspace 12 crate、Python 200+ tests、Makefile、CI、ADR 门禁。

## 快速开始（mock，无需模型）

```powershell
cargo build --workspace
cargo run -p llmgui-cli -- serve --backend mock --port 11435
# 浏览器打开 http://127.0.0.1:11435/
```

```powershell
curl http://127.0.0.1:11435/v1/chat/completions `
  -H "Content-Type: application/json" `
  -d '{"model":"mock","messages":[{"role":"user","content":"你好"}]}'
```

## 下载真实模型

默认缓存位于 `%LOCALAPPDATA%\llmgui`；可用 `LLMGUI_HOME` 覆盖：

```powershell
$env:LLMGUI_HOME = "C:\Users\me\llmgui-home"
cargo run -p llmgui-cli -- pull Qwen/Qwen2.5-0.5B-Instruct-GGUF `
  --file qwen2.5-0.5b-instruct-q4_k_m.gguf
cargo run -p llmgui-cli -- list
cargo run -p llmgui-cli -- estimate <下载后的 gguf> --ctx 4096
```

RTX 5090 D 32GB 上优先用 14B / 32B Q4_K_M（GUI 模型页已内置并标注显存估算）：

```powershell
# 14B：32k ctx + q8_0 KV 估算约 13.8GB，余量充足（主力档）
cargo run -p llmgui-cli -- pull Qwen/Qwen3-14B-GGUF --file Qwen3-14B-Q4_K_M.gguf
# 32B：估算约 25.4GB，建议降到 16k ctx 并单路并发，避免 OOM
cargo run -p llmgui-cli -- pull Qwen/Qwen3-32B-GGUF --file Qwen3-32B-Q4_K_M.gguf
```

## 模型管理

模型库由 `$LLMGUI_HOME/registry.json` 索引，可由 `models/*/manifest.json` 与目录扫描重建。
服务端是唯一权威，CLI 与 Web GUI 共用同一套 `/admin/*` 接口。

```powershell
llmgui models list                 # 已安装模型（--json 输出完整注册表）
llmgui models search coder         # 搜索推荐目录（Qwen3 / Coder / Phi-4 / Mistral / MoE）
llmgui models scan                 # 扫描默认目录与 config 里的 extra_dirs
llmgui models import path\to\model.gguf
llmgui models import path\to\merged.gguf --base-id Qwen--Qwen3-14B-GGUF--q4-k-m
llmgui models show <id>             # 详情 + 显存矩阵
llmgui models verify [id]           # 复核 manifest 里的 SHA256
llmgui models remove <id> --purge   # 删除模型并回收未引用缓存

# 运行时（先启动 llmgui serve，可不再传 --model）
llmgui load <id> --ctx 16384 --kv q8_0 --slots 1 --ttl 3600
llmgui ps
llmgui unload --all
```

- `llmgui serve --model` 仍兼容：启动时把该模型登记为已加载；不传 `--model` 时首个聊天请求按已安装模型 JIT 加载。
- 单卡默认只常驻一个模型；切换会 drain 在飞请求（超时可配 `LLMGUI_DRAIN_TIMEOUT_SECS`），失败回滚旧模型。
- 加载前按 GGUF metadata + ctx/KV/slots 估算显存；超过 `LLMGUI_VRAM_BYTES`（默认 32GB）直接拒绝。
- Web GUI 的模型页分「模型库 / 发现 / 下载」：支持搜索、显存筛选、加载/卸载、别名与默认值、删除、下载队列与磁盘回收。
- 推荐目录当前收录 11 个许可友好的条目（Apache-2.0 / MIT）：Qwen2.5 7B/0.5B、Qwen3 4B/8B/14B/32B/30B-A3B、Qwen2.5-Coder 7B、Phi-4 14B、Mistral-Small 24B 与 Qwen2.5-7B 训练底座。

## 真实推理

### 方式 A：WSL/Linux llama-server（当前 Windows 可用）

本机 Windows 上 in-process `llama.cpp` 初始化存在环境阻塞，但 WSL 中构建的
`llama-server` 可正常推理。启动 WSL server：

```bash
# WSL 内（模型路径通过 /mnt/c 访问）
cd /tmp/llama.cpp
./build/bin/llama-server \
  -m /mnt/c/Users/me/llmgui-home/models/Qwen--Qwen2.5-0.5B-Instruct-GGUF/qwen2.5-0.5b-instruct-q4_k_m.gguf \
  --host 127.0.0.1 --port 18080 -c 2048 -t 8
```

Windows 侧通过 upstream 后端接入同一套 OpenAI API / GUI：

```powershell
$env:LLMGUI_UPSTREAM_URL = "http://127.0.0.1:18080"
cargo run -p llmgui-cli -- serve --backend upstream --port 11435
# 或单次推理
cargo run -p llmgui-cli -- run --backend upstream "你好" --max-tokens 64
```

也可以一条命令自动拉起 WSL server（需要 WSL 中已构建 `/tmp/llama.cpp/build/bin/llama-server`）：

```powershell
cargo run -p llmgui-cli -- serve --backend upstream --start-wsl `
  --model C:/Users/me/llmgui-home/jobs/merged-q4km.gguf `
  --wsl-port 18080 --port 11435
```

### 方式 B：in-process llama.cpp（Linux/修复后 Windows）

```powershell
# 需要 MSVC + libclang；先设置 LIBCLANG_PATH，并确保 vcvars64 环境
cargo run -p llmgui-cli --features llama -- serve --model <下载后的 gguf>
```

> **Linux/WSL 已验证**：同一代码在 WSL Ubuntu 24.04 构建后，单流 87 tok/s、TTFT 25ms，
> 同 prompt 两请求命中 in-process prefix cache 1 次；服务地址 `/healthz`、`/metrics` 正常。
>
> **Windows 已知阻塞**：Windows 11 build 26300 + Intel Arrow Lake 上
> `llama_backend_init()` 在 llama-cpp-2 0.1.159/0.1.160、自编译 C++ smoke 与预编译
> llama-cli 上均触发 `STATUS_ACCESS_VIOLATION`；FFI 编译、GGUF 下载/解析、显存预估正常。
> 因此 Windows 使用 `upstream` 后端保底，in-process 路径在 Linux 或修复后使用。

## 微调

```powershell
pip install -e .          # 无 ML 依赖，提供 `llmgui-train` / `llmgui-train-worker` 命令
pip install -e ".[cuda]"  # torch/transformers/peft/trl/bnb

# 真实 CPU LoRA / QLoRA 4-bit 冒烟（已各验证 1 step，输出 adapter_model.safetensors）
cargo run -p llmgui-cli -- train --config examples/train_cpu.json
cargo run -p llmgui-cli -- train --config examples/train_qlora_cpu.json

# GPU QLoRA 4-bit（已在本机 RTX 5090 D 验证）
cargo run -p llmgui-cli -- train --config examples/train_qlora_gpu.json       # 0.5B
cargo run -p llmgui-cli -- train --config examples/train_qlora_7b_gpu.json    # 7B，1 step
cargo run -p llmgui-cli -- train --config examples/train_qlora_7b_bench.json  # 7B，5 step 基准
```

7B QLoRA 实测：CUDA 4-bit NF4、`all-linear`、gradient checkpointing；RTX 5090 D 上
5 step（含加载/导出）约 **19s**，loss 5.08 → 3.04；manifest 记录 `torch 2.9.1+cu128`、
`device=cuda`。Windows 需要 CUDA 版 torch + bitsandbytes。

7B adapter 也已走通 merge → GGUF f16（14.2GB）→ Q4_K_M（4.36GB）→ WSL llama-server，
CPU 推理约 **12.4 tok/s**。

没有 torch 时 worker 会返回结构化 `missing_dependency`，不会静默失败。

### 导出 GGUF（已验证）

```powershell
# 1. 原生 PEFT merge（比 llama-export-lora 更适合 HF adapter）
llmgui-train merge `
  --base-model C:/Users/me/llmgui-home/models/Qwen2.5-0.5B-Instruct `
  --adapter-dir C:/Users/me/llmgui-home/jobs/sft-cpu `
  --output C:/Users/me/llmgui-home/jobs/merged-cpu --native

# 2. HF -> GGUF（指向 llama.cpp 的 convert_hf_to_gguf.py）
llmgui-train export-gguf `
  --model-dir C:/Users/me/llmgui-home/jobs/merged-cpu `
  --output C:/Users/me/llmgui-home/jobs/merged-f16.gguf `
  --tool "python C:/path/to/llama.cpp/convert_hf_to_gguf.py"
```

量化：`llama-quantize merged-f16.gguf merged-q4km.gguf Q4_K_M`（Linux/WSL 构建的 llama.cpp），
也可用封装命令：

```powershell
llmgui-train quantize `
  --input C:/Users/me/llmgui-home/jobs/merged-f16.gguf `
  --output C:/Users/me/llmgui-home/jobs/merged-q4km.gguf `
  --qtype Q4_K_M `
  --tool "C:/path/to/llama-quantize.exe"
```

WSL 用户可直接运行 Linux 版 `llama-quantize`（Windows 路径通过 `/mnt/c` 传入）。

然后用 `llama-server` 加载该 GGUF，即可通过 upstream 后端服务。

## 桌面壳（Tauri 2）

`crates/llmgui-desktop` 是桌面入口：启动本地运行时（`llmgui serve`，也可 `--no-spawn` 附加到
已运行的服务），等 `/healthz` 就绪后把窗口加载到该运行时源站，因此 `gui/` 页面里的相对
`/v1/...` 调用不需要任何额外配置。GUI 只有一份（`gui/`），桌面壳不重复实现业务逻辑。

Linux 需要 webkit2gtk 4.1 等系统库（Ubuntu 24.04）：

```bash
sudo apt-get install -y libwebkit2gtk-4.1-dev libxdo-dev \
  libayatana-appindicator3-dev librsvg2-dev
```

```powershell
# mock 冲烟：壳自己拉起 mock 运行时
cargo run -p llmgui-desktop

# 接入真实模型：用 upstream 后端接 WSL/Linux 的 llama-server
$env:LLMGUI_UPSTREAM_URL = "http://127.0.0.1:18080"
cargo run -p llmgui-desktop -- -- serve --backend upstream `
  --model C:/Users/me/llmgui-home/jobs/merged-7b-q4km.gguf

# 只附加到已在运行的服务
cargo run -p llmgui-desktop -- --no-spawn
```

`make desktop` 支持两个变量：`DESKTOP_ARGS` 原样透传（接 7B 模型用），`DESKTOP_ENV` 追加环境。

```bash
LLMGUI_UPSTREAM_URL=http://127.0.0.1:18080 \
make desktop DESKTOP_ARGS="serve --backend upstream --model /mnt/c/<user>/llmgui-home/jobs/merged-7b-q4km.gguf"
```

WSLg / 无 GPU 直通的 Linux 上给 WebKit 加软件渲染（否则可能出现 EGL/DMA-BUF 报错或空白窗口）：

```bash
make desktop DESKTOP_ENV="GDK_BACKEND=x11 WEBKIT_DISABLE_COMPOSITING_MODE=1 \
  WEBKIT_DISABLE_DMABUF_RENDERER=1 LIBGL_ALWAYS_SOFTWARE=1"
```

`--selftest` 是端到端体验验收：壳在真实 WebView 里驱动真实 UI（切页 → 输入 → 发送 →
等 SSE 结束 → 回读 DOM 里的模型名 / TTFT / tok/s），把结果 POST 回来并按退出码判定，
适合当回归冲烟。7B 实测（merged-7b-q4km，WSL CPU llama-server）：

```bash
LLMGUI_UPSTREAM_URL=http://127.0.0.1:18080 cargo run -p llmgui-desktop -- \
  --selftest --selftest-report /tmp/llmgui-7b.json \
  -- serve --backend upstream --model /mnt/c/<user>/llmgui-home/jobs/merged-7b-q4km.gguf
# => ok: true，TTFT 384 ms，14.2 tok/s，模型显示 merged-7b-q4km
```

## 目录

- `crates/`：Rust workspace（core/models/download/backend/scheduler/server/cli/bench/desktop）
- `python/llmgui_train/`：Python 训练 worker（SFT/QLoRA、导出、评测）
- `gui/`：零构建本地 Web GUI（由 `llmgui serve` 托管，桌面壳 `llmgui-desktop` 复用）
- `docs/`：ADR、需求卡片、调研
- `models/v0.toml`：默认模型清单
- `scripts/`：ADR/文档门禁与本地构建辅助脚本

## 测试与门禁

```powershell
make build          # cargo build --workspace
make release        # 当前平台 release 二进制
make test           # Rust workspace + Python 单测
make check-fast     # fmt/clippy/ruff/compileall/adr-check
make check-docs     # 文档门禁（含 ADR 落地校验）
make flow           # check-fast + test
make push           # 推当前分支到所有 git remote（逐个远端，失败不阻塞其余）
make desktop        # cargo run -p llmgui-desktop（mock 冲烟）
make desktop-accept # 端到端验收（默认 mock；用 DESKTOP_ARGS 传 7B 参数）
```

安装与诊断：

```powershell
powershell -ExecutionPolicy Bypass -File scripts/install.ps1
target\debug\llmgui.exe doctor
```

`make cov` 需要本机安装 `cargo-llvm-cov` 与 `pytest-cov`。
