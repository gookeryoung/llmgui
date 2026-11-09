# ADR 0004 P2b：Qwen3-Coder-30B-A3B turbo 实测（2026-11-09）

原始报告：[0004-qwen3-coder-30b-turbo.json](0004-qwen3-coder-30b-turbo.json)

## 硬件与软件

| 项 | 值 |
|---|---|
| GPU | NVIDIA GeForce RTX 5090 D，32,607 MiB |
| 驱动 | 616.56（Windows） |
| 主机 / 运行环境 | Windows + WSL2（`networkingMode=mirrored`），bench 在 WSL 内运行，引擎在 Windows 侧 |
| 引擎 | LM Studio 自带 `llama-server.exe`（CUDA12 AVX2，build 2.53.0，llama.cpp commit `3109914`） |
| 模型 | `unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF` 的 `Qwen3-Coder-30B-A3B-Instruct-Q4_K_M.gguf` |
| 模型体积 | 18,556,689,568 B（下载后 SHA256 校验通过） |
| 运行参数 | `-ngl 999 -fa on -ctk q4_0 -ctv q4_0 -c 8192 --no-context-shift -b 2048 -ub 512 -t 8 -tb 8` |

> 引擎是 LM Studio 随包提供的 Windows CUDA 构建，不是本项目自行编译的 Linux 版
> llama.cpp；结论对「同参数、同模型、同硬件」的 CUDA+FA llama.cpp 有效，但版本差异
> 仍需在自编译构建上复验。

## 结果

| 项 | 值 |
|---|---|
| 并发的请求 | `concurrency=1`，`iterations=5`，`warmup=3`，`max_tokens=256` |
| 单次 decode tok/s | 227.5 / 230.1 / 232.5 / 235.2 / 236.3 |
| **中位数 decode** | **232.5 tok/s** |
| TTFT | 22–51 ms（两次运行区间） |
| 5 次总耗时 | 5,595 ms（1,280 completion tokens） |
| 显存占用峰值 | 25,751 MiB（整卡口径，含 Windows 桌面约 4 GiB） |
| 判定 | `target_min=90` → `target_pass=true`；120 为参考上限，超出记为余量 |

冷启动第一次运行（模型刚加载、GPU 频率未爬升）样本为 134.3 / 161.2 / 240.6 / 245.2 /
253.2，中位数 240.6；第二次运行稳定在 227–236。

## 复现

```bash
# 1) 下载（已归档的模型目录：$LLMGUI_HOME/models/unsloth--Qwen3-Coder-30B-A3B-Instruct-GGUF--q4-k-m/）
LLMGUI_HOME=/mnt/e/llmgui-home cargo run -p llmgui-cli -- pull \
  unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF \
  --file Qwen3-Coder-30B-A3B-Instruct-Q4_K_M.gguf

# 2) 启动引擎（Windows 侧可执行文件经 WSL interop 启动，镜像网络下 localhost 互通）
cd /mnt/e/.lmstudio/extensions/backends/llama.cpp-win-x86_64-nvidia-cuda12-avx2-2.53.0
./llama-server.exe -m 'E:\llmgui-home\models\unsloth--Qwen3-Coder-30B-A3B-Instruct-GGUF--q4-k-m\Qwen3-Coder-30B-A3B-Instruct-Q4_K_M.gguf' \
  -ngl 999 -fa on -ctk q4_0 -ctv q4_0 -c 8192 --no-context-shift -b 2048 -ub 512 \
  -t 8 -tb 8 --host 127.0.0.1 --port 18080

# 3) 验收
make bench-turbo
```

## 备注

- 新版 llama.cpp 的 `-fa` 需要显式取值（`on|off|auto`）；裸 `-fa` 会把 `-ctk` 当作值并
  以 `unknown value for --flash-attn` 启动失败。项目内 Makefile/CLI/README 已统一为 `-fa on`。
- 吞吐判定为下限目标：`median_decode_tps >= target_min` 即 `target_pass=true`；
  `target_max` 仅作参考上限，不参与失败判定。
