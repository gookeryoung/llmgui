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
- **工程**：Rust workspace 11 crate、Python 201 tests、Makefile、CI、ADR 门禁。

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
pip install -e python/llmgui_train          # 无 ML 依赖
pip install -e "python/llmgui_train[cuda]"  # torch/transformers/peft/trl/bnb

# 真实 CPU LoRA / QLoRA 4-bit 冒烟（已各验证 1 step，输出 adapter_model.safetensors）
cargo run -p llmgui-cli -- train --config examples/train_cpu.json
cargo run -p llmgui-cli -- train --config examples/train_qlora_cpu.json

# 7B QLoRA（需要 CUDA + bitsandbytes）
cargo run -p llmgui-cli -- train --config examples/train.json
```

没有 torch 时 worker 会返回结构化 `missing_dependency`，不会静默失败。

### 导出 GGUF（已验证）

```powershell
# 1. 原生 PEFT merge（比 llama-export-lora 更适合 HF adapter）
python -m llmgui_train.cli merge `
  --base-model C:/Users/me/llmgui-home/models/Qwen2.5-0.5B-Instruct `
  --adapter-dir C:/Users/me/llmgui-home/jobs/sft-cpu `
  --output C:/Users/me/llmgui-home/jobs/merged-cpu --native

# 2. HF -> GGUF（指向 llama.cpp 的 convert_hf_to_gguf.py）
python -m llmgui_train.cli export-gguf `
  --model-dir C:/Users/me/llmgui-home/jobs/merged-cpu `
  --output C:/Users/me/llmgui-home/jobs/merged-f16.gguf `
  --tool "python C:/path/to/llama.cpp/convert_hf_to_gguf.py"
```

量化：`llama-quantize merged-f16.gguf merged-q4km.gguf Q4_K_M`（Linux/WSL 构建的 llama.cpp），
也可用封装命令：

```powershell
python -m llmgui_train.cli quantize `
  --input C:/Users/me/llmgui-home/jobs/merged-f16.gguf `
  --output C:/Users/me/llmgui-home/jobs/merged-q4km.gguf `
  --qtype Q4_K_M `
  --tool "C:/path/to/llama-quantize.exe"
```

WSL 用户可直接运行 Linux 版 `llama-quantize`（Windows 路径通过 `/mnt/c` 传入）。

然后用 `llama-server` 加载该 GGUF，即可通过 upstream 后端服务。

## 目录

- `crates/`：Rust workspace（core/models/download/backend/scheduler/server/cli/bench）
- `python/llmgui_train/`：Python 训练 worker（SFT/QLoRA、导出、评测）
- `gui/`：零构建本地 Web GUI（由 `llmgui serve` 托管）
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
```

安装与诊断：

```powershell
powershell -ExecutionPolicy Bypass -File scripts/install.ps1
target\debug\llmgui.exe doctor
```

`make cov` 需要本机安装 `cargo-llvm-cov` 与 `pytest-cov`。
