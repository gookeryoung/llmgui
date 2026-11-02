# Changelog

所有重要变更记录在此。格式参考 Keep a Changelog；版本遵循 SemVer。

## [Unreleased]

### Docs

- 新增 ADR 0002《模型管理（模型库 / 发现下载 / 运行时生命周期 / 训练产物）》（Proposed，待评审）
  与需求卡片 0003、调研 `docs/research/model-management.md`：参考 LM Studio / Unsloth 规划
  注册表驱动的模型库、任务化下载、加载/卸载/切换/JIT/TTL、预设/别名与训练产物派生注册。

### Fixed

- 修复模型库卡片「加载」在未发送 JSON 请求体时被 415 拒绝
  （`Expected request with Content-Type: application/json`）：`load` 接口现在允许空 body
  并使用模型默认参数，GUI 卡片同时显式发送 `{}`。
- 下载小文件（如 `.gitattributes`、config）时直接单流拉取，不再触发「server ignored
  Range」告警；仅包含非 GGUF 文件的 manifest 不再注册成模型。

### Added

- ADR 0002 模型管理落地（P0/P1 完成，P2a/P2b/P3 部分）(ADR 0002 P0/P1/P2a/P2b/P3)：
  - 注册表驱动的模型库 `$LLMGUI_HOME/registry.json`（原子写 + 文件锁 + 可由 manifest 重建），
    扫描/导入 GGUF、详情与 24/32GB fit 矩阵、别名/加载默认值/预设、SHA256 校验、删除与引用检查；
  - `llmgui models list|show|scan|import|remove|verify|search --json`；`pull` 完成后自动入库，
    按量化写入独立目录（`<repo-id>--<quant>`）；
  - `/admin/models*`、`/admin/catalog*`、`/admin/downloads*`、`/admin/storage`、`/admin/cache/gc`；
    下载任务支持进度、暂停/继续/取消/重试、持久化恢复与 SSE 快照，CAS 引用计数 GC；
  - 运行时 load/unload/switch、加载前显存闸门、JIT 按需加载、TTL 空闲卸载、drain/回滚；
    `llmgui serve` 不再必须 `--model`，`/v1/models` 反映当前已加载模型；
  - 派生模型注册（`--base-id` / `--adapter-id`）与 `model.yaml` 兼容 JSON 描述符导入导出；
  - Web GUI 模型页重构为「模型库 / 发现 / 下载」：搜索/显存筛选/排序、详情抽屉、加载/卸载/
    删除/校验、推荐目录与仓库解析、下载队列与磁盘回收。
- 发现目录扩充到 11 个许可友好的条目：新增 Qwen3-8B、Qwen3-4B、Qwen2.5-Coder-7B、
  Qwen3-30B-A3B（MoE）、Phi-4 14B（MIT）、Mistral-Small-24B（Apache-2.0），体积按
  2026-11-03 ModelScope 列表实测；发现页角色标签中文化。
- 修复：识别 `Q4_K` / `Q5_K` / `Q3_K` 等无后缀量化；仓库同时提供单文件与分片时
  优先单文件，避免重复下载权重。
- 模型清单新增 5090D 32GB 大模型档位：`Qwen/Qwen3-14B-GGUF`（Q4_K_M，32k ctx 估算
  ≈13.8GB）与 `Qwen/Qwen3-32B-GGUF`（Q4_K_M，估算 ≈25.4GB，建议 16k ctx + 单路并发），
  `models/v0.toml` 与 GUI 内置清单同步（需求卡片 0002）。
- 模型页选择 GUI：关键词搜索、按用途/显存筛选 chip（带数量）、按体积排序、结果计数与空态
  提示；卡片增加角色色条、32GB「可跑 / 紧张」徽标、显存估算列与文件行，保留复制命令。

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
- Tauri 2 桌面壳 `crates/llmgui-desktop`：拉起 `llmgui serve`（或 `--no-spawn` 附加已运行服务），
  等 `/healthz` 后就绪再把窗口指到运行时源站，复用 `gui/` 零构建页面（UI 只一份，不重复实现
  业务逻辑）；`--` 之后的参数原样透传给 `llmgui`（如 7B/upstream），退出时回收子进程。系统依赖（webkit2gtk 4.1）
  与三条启动方式写入 README。
- 桌面壳端到端验收 `--selftest`：在真实 WebView 中驱动真实 UI（切页 → 输入 → 发送 → 等 SSE
  结束 → 回读模型名 / TTFT / tok/s / 无错误气泡），结果经 loopback POST 回收并以退出码判定；
  7B 实测（merged-7b-q4km + WSL CPU llama-server）`ok: true`，TTFT 384 ms、14.2 tok/s；mock 冒烟同绿。
- 工程便利：`make push` 按 `git remote` 逐个远端推送当前分支（单远端失败不阻塞其余，末态
  仍以非零退出暴露失败）；`make desktop` / `make desktop-accept` 提供桌面壳入口。
- 打包入口：根 `pyproject.toml` 统一声明 Python 分发包（hatchling）与命令入口
  `llmgui-train`、`llmgui-train-worker`；仓库根 `pip install -e ".[cuda]"` 后即可直接调用，
  不再需要 `python -m llmgui_train.cli`。
- GPU QLoRA 4-bit：RTX 5090 D 上跑通 0.5B 与 7B（4-bit NF4、all-linear、gradient
  checkpointing）；7B 5 step（含加载/导出）约 19s，loss 5.08 → 3.04，manifest 记录
  `torch 2.9.1+cu128`、`device=cuda`。7B adapter → merge → GGUF f16（14.2GB）→
  Q4_K_M（4.36GB）→ WSL llama-server 也已完成，CPU 推理约 12.4 tok/s。
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
