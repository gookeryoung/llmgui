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
- CLI：`pull/list/serve/run/estimate/bench/train`。
- Python 训练包：配置、数据（Alpaca/ShareGPT/OpenAI messages）、assistant mask、packing、
  显存估算、PEFT adapter/GGUF 导出封装、stdio NDJSON JSON-RPC worker、QLoRA lazy 训练器；
  200+ 个测试、>92% 覆盖率。
- 端到端训练冒烟：Qwen2.5-0.5B + CPU LoRA / QLoRA 4-bit（各 1 step）经 `llmgui train` 跑通，
  生成 `adapter_model.safetensors` 与训练 manifest；修复了 worker 在 Windows 上 torch 导入
  与 stdin 读线程的死锁。
- Web GUI：模型库、流式聊天、训练 YAML 生成、设置；零构建、离线、无外部资源。
- 文档门禁：`scripts/check_adr_landing.py`、`scripts/gitkeep_check.py`、Makefile、CI。

### Known limitations

- Rust 调度器目前提供并发槽位与取消；upstream 通过 `cache_prompt` 使用 llama-server 前缀缓存，
  **in-process llama.cpp 的 KV prefix cache 与逐步 batch 交织尚未实现**；`llmglui-backend-llamacpp`
  每次请求新建 context。
- llama.cpp 后端在部分旧版 MSVC 工具链下运行期初始化崩溃（`STATUS_ACCESS_VIOLATION`），
  需要 VS2022 新工具链或 Linux/WSL；upstream 后端已验证真实推理，mock 后端不受影响。
- Python 真实 QLoRA 4-bit 已用 bitsandbytes CPU 验证 1 step；7B/GPU 训练性能、GGUF 转换未验证。
- Tauri 桌面壳未构建，GUI 以本地 Web 形式交付；多卡、DPO、speculative、国产卡为 post-v1.0。
