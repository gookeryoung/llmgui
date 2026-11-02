# 需求卡片：模型管理（库 / 发现下载 / 运行时生命周期 / 训练产物）

- 编号：0003
- 来源 / 提出人：用户会话（2026-11-03）
- 日期：2026-11-03
- 关联 ADR：`docs/adr/0002-model-management.md`（Proposed，待方案评审）

> 本卡片是**需求原文与约束编号的唯一副本**：ADR 只引用 C1..Cn，不复制原文；需求澄清后先改这里，再同步 ADR「口径」表。

## 需求原句（逐字）

> 请参考UNSLOTH和LLMSTUDIO针对模型管理功能制定完善计划

## 可实施约束（C1..Cn）

| # | 需求句（原文片段） | 可实施约束 | 涉及模块 | 验收方式 |
|---|---|---|---|---|
| C1 | 模型管理功能 | 建立**本机模型库**：已安装/已导入/外部目录统一索引，列出体积、量化、上下文、许可、来源、哈希、安装时间；安装态与加载态分离 | `llmgui-core`、`llmgui-download`、`llmgui-server`、`gui/` | 库列表与磁盘一致；`llmgui models list --json` 可被 GUI 复用；重启后索引可重建 |
| C2 | 参考 LM Studio | **发现与量化选择**：支持仓库 ID / URL / 关键词输入，按量化并列展示体积、显存估算、24GB/32GB 适配；下载前必须看到许可 | `llmgui-download`、`llmgui-models`、`gui/` | fixture 搜索/解析测试；32GB 档位标注与 `estimate` 一致 |
| C3 | 模型管理功能 | **下载任务化**：队列、并发上限、进度、暂停/恢复/取消/重试，断点跨进程可续；任务状态可回读 | `llmgui-download`、`llmgui-server`、`gui/` | 50% 中断后续传 SHA256 一致；取消后无残留 `.part` 泄漏；SSE 事件顺序正确 |
| C4 | 模型管理功能 | **模型详情**：GGUF metadata、量化、上下文上限、许可、文件表与哈希、来源、显存预估矩阵（24/32GB × ctx/KV） | `llmgui-models`、`llmgui-server`、`gui/` | 详情字段与 manifest/metadata 同源；无 GGUF 的目录拒绝注册 |
| C5 | 模型管理功能 | **运行时生命周期**：加载/卸载/切换、加载前显存闸门、失败回滚、JIT 按需加载、空闲 TTL 卸载、GPU lease 协同；单卡默认单活跃模型 | `llmgui-server`、`llmgui-scheduler`、`llmgui-cli`、`gui/` | 无 `--model` 也能 serve；切换不丢正在流式的响应（drain/409 口径明确）；OOM 前拒绝 |
| C6 | 参考 LM Studio | **每模型默认值与别名**：ctx/KV/slots/GPU offload/TTL 可保存并被 CLI 与 GUI 共用；模型别名唯一、可解析；命名预设管理 system prompt 与采样参数 | `llmgui-core`、`llmgui-server`、`gui/` | defaults 保存后 `llmgui load` 等价命令生效；别名冲突 409；预设导入导出往返一致 |
| C7 | 参考 UNSLOTH | **训练/导出产物接入同一库**：训练底座从库中选择（safetensors），adapter 注册，merge/量化/GGUF 导出后自动注册为派生模型并记录 lineage 与评测指标 | `python/llmgui_train`、`llmgui-server`、`gui/` | e2e：下载→训练→导出→注册→加载→对话；lineage 与 base 不匹配即拒绝 |
| C8 | 模型管理 GUI | **信息架构**：模型页拆为「模型库 / 发现 / 下载」三视图，支持搜索、筛选（适配/用途/量化/许可）、排序、批量操作、磁盘占用、空态/加载态/错误态；保持零构建与 `textContent` 安全约束 | `gui/` | `node --check` + jsdom DOM 测试；键盘可达；无 `innerHTML` |
| C9 | 制定完善计划 | **CLI / API 对等**：GUI 的每个动作都有 CLI 与 HTTP 入口，输出支持 `--json`；GUI 不直接写磁盘/注册表，服务端是唯一权威 | `llmgui-cli`、`llmgui-server` | 动作矩阵逐项有 CLI/HTTP 测试；GUI 无直写路径 |
| C10 | 模型管理功能 | **安全与合规**：路径锚定、仅白名单格式、SHA256 校验、删除确认、加载中/被引用模型禁删、admin API 默认 loopback、离线模式不联网、许可随 manifest 展示 | 全域 | 路径穿越/符号链接/哈希不符/删除使用中模型的负例测试；威胁表逐项有控制 |
| C11 | 制定完善计划 | **计划完整性**：ADR 含现状核对、目标/非目标、不变量、数据与接口契约、兼容迁移、分期与退出标准、性能预算、风险、决策台账、落地状态与交接 | `docs/adr/0002-model-management.md` | `make check-docs`；评审通过后状态改 Accepted 方可实现 |

## 现状影响面（候选，待 ADR 核实）

| # | 疑似受影响能力 | 候选位置 | 备注 |
|---|---|---|---|
| 1 | 已安装模型枚举 | `crates/llmgui-cli/src/main.rs:273` | 只打印 manifest，无 API/索引 |
| 2 | 服务端模型列表 | `crates/llmgui-server/src/lib.rs:139` | 固定返回单个 `model_id` |
| 3 | 下载执行 | `crates/llmgui-download/src/lib.rs:63` | 前台单次 `pull`，无队列/进度持久化 |
| 4 | 源能力 | `crates/llmgui-download/src/source.rs:100` | 只有 `list/fetch`，无搜索/解析 |
| 5 | CAS 缓存 | `crates/llmgui-download/src/cache.rs:71` | 无删除、引用计数、GC |
| 6 | manifest | `crates/llmgui-core/src/manifest.rs:9` | schema v1 只描述下载产物 |
| 7 | registry 预留 | `crates/llmgui-core/src/paths.rs:41` | `registry.json` 未被使用 |
| 8 | 加载/卸载 | `crates/llmgui-cli/src/main.rs:419`；`crates/llmgui-backend-api/src/lib.rs:110` | CLI 一次性加载；`unload` 已有但无 manager/端点 |
| 9 | 显存预估 | `crates/llmgui-core/src/estimate.rs:58`；`crates/llmgui-models/src/lib.rs:108` | 仅 CLI `estimate` 使用，未做加载闸门 |
| 10 | GUI 模型页 | `gui/app.js:15`；`gui/index.html:62`；`gui/app.js:540` | 静态清单镜像 + 只读 `/v1/models` |
| 11 | 训练导出 | `python/llmgui_train/src/llmgui_train/export.py:203/433/507` | 有导出能力，无注册/lineage |
| 12 | 预设/别名 | 无 | 无数据结构与接口 |

## 未决问题

- 无（本轮交付为计划与决策提案；实现细节在 ADR 评审时确认，不在实现前开工）。

## 卡点

- [x] 每条需求句都有 C 编号约束（无遗漏、无「待定」）
- [x] 需求方确认计划范围（用户会话直接下达「制定完善计划」指令；本轮不进入实现）
- [x] ADR 已产出并标 Proposed，等待方案评审后转 Accepted
