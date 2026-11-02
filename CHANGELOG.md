# Changelog

所有重要变更记录在此。格式参考 Keep a Changelog；版本遵循 SemVer。

## [0.1.0] — 2026-11-02

首个可运行版本，对应 ADR 0001 的 P0/P1/P2a/P2b 代码骨架与部分 P3 硬化项。

### Added

- Rust workspace：`llmgui-core`、`llmgui-models`、`llmgui-download`、`llmgui-backend-api`、
  `llmgui-backend-mock`、`llmgui-backend-llamacpp`、`llmgui-backend-upstream`、`llmgui-scheduler`、
  `llmgui-server`、`llmgui-cli`、`llmgui-bench`。
- 国内源下载：ModelScope 列表/下载、hf-mirror/Gitee AI HF 兼容源、分片 Range、断点续传、
  流式 SHA256、CAS、硬链接快照、`pull/list` CLI；本地 HTTP fixture 34 个测试。
- GGUF metadata 解析与显存预估（权重 + KV + compute + runtime），`llmgui estimate`。
- llama.cpp in-process 后端（`--features llama`）：模型加载、tokenize、prefill/decode、
  greedy/temperature/top-p 采样、UTF-8 跨 token 拼接、取消与统计；默认 CPU 路径。
- OpenAI 兼容服务：`/v1/chat/completions`（SSE/非流式）、`/v1/completions`、`/v1/models`、
  `/healthz`、`/metrics`、Bearer token、本地 Web GUI 静态托管。
- `upstream` 后端：对接本地/ WSL `llama-server`（OpenAI 兼容），在 Windows in-process
  阻塞期间提供真实模型推理；已验证 Qwen2.5-0.5B CPU 输出与 137.5 tok/s 量级。
- `llmgui serve --backend upstream --start-wsl`：一条命令检测/拉起 WSL llama-server，
  等待健康后启动 OpenAI 服务；WSL server 用 `setsid` 脱离会话保持运行。
- 4 并发 upstream 基准：4×32 token，聚合 288 tok/s（单流约 140 tok/s）。
- CLI：`pull/list/serve/run/estimate/bench/train`。
- Python 训练包：配置、数据（Alpaca/ShareGPT/OpenAI messages）、assistant mask、packing、
  显存估算、PEFT adapter/GGUF 导出封装、stdio NDJSON JSON-RPC worker、QLoRA lazy 训练器；
  200+ 个测试、>92% 覆盖率。
- GPU QLoRA 4-bit：RTX 5090 D 上跑通 0.5B 与 7B（4-bit NF4、all-linear、gradient
  checkpointing）；7B 5 step（含加载/导出）约 19s，loss 5.08 → 3.04，manifest 记录
  `torch 2.9.1+cu128`、`device=cuda`。
- 端到端训练冒烟：Qwen2.5-0.5B + CPU LoRA / QLoRA 4-bit（各 1 step）经 `llmgui train` 跑通，
  生成 `adapter_model.safetensors` 与训练 manifest；修复了 worker 在 Windows 上 torch 导入
  与 stdin 读线程的死锁。
- 端到端导出/推理闭环：PEFT adapter → `merge_adapter_peft`（`merge_and_unload`）→
  `convert_hf_to_gguf.py` → `llama-quantize Q4_K_M` → WSL `llama-server` → upstream 后端输出；
  微调后的 GGUF 已在 18081 端口实测推理成功。
- Web GUI：模型库、流式聊天、训练 YAML 生成、设置；零构建、离线、无外部资源。
- 文档门禁：`scripts/check_adr_landing.py`、`scripts/gitkeep_check.py`、Makefile、CI。
- P3 发布准备：`make release`、`scripts/install.ps1`、GitHub Release 工作流、`llmgui doctor`
  环境诊断（home/cache/models/Python/WSL/upstream）。

### Known limitations

- Rust 调度器提供并发槽位/取消（3 个测试）；upstream 通过 `cache_prompt` 使用 llama-server
  前缀缓存；in-process llama.cpp 已在 Linux/WSL 验证：单流 87 tok/s、同 prompt 两请求
  prefix cache 命中 1 次；Windows 初始化崩溃，逐步 batch 交织未实现。
- llama.cpp 后端在 Windows 11 + Arrow Lake 上 `llama_backend_init` 触发
  `STATUS_ACCESS_VIOLATION`（自编译/预编译/MSVC/MinGW 均复现）；Linux/WSL 与 upstream 正常。
- QLoRA 4-bit 已在 CPU 与 CUDA（0.5B/7B）验证；多卡/长上下文吞吐未验证；
  GGUF 转换/量化与微调后 GGUF 推理已验证。
- Tauri 桌面壳未构建，GUI 以本地 Web 形式交付；多卡、DPO、speculative、国产卡为 post-v1.0。
