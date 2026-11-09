# 需求卡片：发现目录在线更新与能力建议标签

- 编号：0005
- 来源 / 提出人：用户会话（2026-11-09）
- 日期：2026-11-09
- 关联 ADR：`docs/adr/0005-catalog-online-tags.md`（Accepted，2026-11-09 用户确认）

> 本卡片是**需求原文与约束编号的唯一副本**：ADR 只引用 C1..Cn，不复制原文；需求澄清后先改这里，再同步 ADR「口径」表。

## 需求原句（逐字）

> 发现的模型能否支持自动从线上更新列表，并且按照编程、多模态等给出建议标签。

## 可实施约束（C1..Cn）

| # | 需求句（原文片段） | 可实施约束 | 涉及模块 | 验收方式 |
|---|---|---|---|---|
| C1 | 自动从线上更新列表 | 发现页目录由**版本化远程目录**提供；服务端启动时与每 24h 检查更新，成功后写入本地缓存；网络失败、超时或离线时按「缓存 → 内置清单」回退，不阻塞发现页；提供手动刷新与关闭自动更新的配置 | `llmgui-core`、`llmgui-server`、`gui/` | fixture 服务模拟成功/304/超时/坏数据；四种状态（远程/缓存/内置/刷新中）在 API 与 GUI 可见 |
| C2 | 更新列表 | 远程目录使用 schema 2：含 `updated_at`、模型条目与 curated 标签；兼容 schema 1；只拉取目录元数据，**不触发模型下载**；未知标签按前向兼容忽略 | `llmgui-core`、`llmgui-server`、`models/` | schema 1/2 解析单测；坏 schema/超大响应/非法 repo 被拒绝且保留旧缓存 |
| C3 | 按照编程、多模态等给出建议标签 | 建立固定标签表：`chat/coding/reasoning/math/multimodal/vision/audio/embedding/rerank/long-context/moe/tool-use/train-base`；每个标签含 `id/label/source/confidence`，发现页与模型库可按标签筛选 | `llmgui-models`、`llmgui-server`、`gui/` | 标签表快照测试；筛选组合（AND）测试；未知标签不导致解析失败 |
| C4 | 给出建议标签 | 标签来源优先级为 `user > curated > metadata > file > name`；推断标签必须带来源与置信度并在 UI 标为「建议」；标签不得用于隐藏模型；用户可确认或隐藏建议标签 | `llmgui-models`、`llmgui-server`、`gui/` | 合并优先级单测；用户确认/隐藏后 API 与 GUI 一致；隐藏后不再出现 |
| C5 | 多模态 | 仓库含 `mmproj*.gguf`、GGUF 视觉/音频元数据或名称特征时可给出 `multimodal/vision/audio` 建议；但当前运行时不支持 mmproj 与图片/音频输入，标签必须标注「运行暂不支持」，不得暗示下载后可直接多模态对话 | `llmgui-server`、`gui/`、`docs/` | 含 mmproj 的 fixture 返回 `runtime_supported=false`；GUI 文案可见；无 mmproj 加载代码变更 |
| C6 | 发现的模型 | 已安装模型与发现页使用同一套标签：`GET /admin/models` 返回合并结果；用户标签存 `registry.prefs.tags`，隐藏项存新增的 `prefs.hidden_tags`；不写回 manifest | `llmgui-models`、`llmgui-server` | registry additive 字段兼容测试；manifest 字节不变；API 字段只增 |
| C7 | 自动从线上 | 安全与离线：仅 https（测试 fixture 例外）、10s 超时、≤2MB、≤1000 条、schema 与 repo/file 格式校验、缓存原子写、单飞刷新；`catalog.auto_update = false` 时不发起任何自动请求；不引入遥测 | `llmgui-core`、`llmgui-server` | 负例：http、超大、超时、坏 TOML、并发刷新；关闭后无外网请求 |
| C8 | 完善计划 | 文档与门禁：ADR/README/CONTEXT/CHANGELOG 同步；代码阶段 `make flow`，文档阶段 `make check-docs`；测试覆盖判定层、接口层与 jsdom | `docs/`、`gui/` | `make check-docs`；实现期 `make flow` 全绿 |

## 现状影响面（候选，待 ADR 核实）

| # | 疑似受影响能力 | 候选位置 | 备注 |
|---|---|---|---|
| 1 | 发现目录接口 | `crates/llmgui-server/src/admin.rs:650` | `include_str!` 编译期静态清单，无联网刷新 |
| 2 | 内置目录文件 | `models/v0.toml` | 只有 `role`，无 tags 与 `updated_at` |
| 3 | GUI 目录镜像 | `gui/app.js:15`、`gui/app.js:1037` | 第二份手写清单；发现页只渲染 role |
| 4 | 源能力 | `crates/llmgui-download/src/source.rs:99` | 只有 `list/fetch`，本需求不做 Hub 搜索 |
| 5 | 配置结构 | `crates/llmgui-core/src/config.rs:10` | 无 `[catalog]` 节 |
| 6 | 数据目录 | `crates/llmgui-core/src/paths.rs:29` | 无 `catalog/` 缓存目录 |
| 7 | 用户偏好 | `crates/llmgui-models/src/library.rs:114` | 已有 `prefs.tags`，未暴露合并逻辑 |
| 8 | 已安装模型输出 | `crates/llmgui-server/src/admin.rs:132` | 只返回 `prefs.tags` 原始字符串 |
| 9 | 多模态运行时 | 无 | 无 mmproj 加载与图片输入，标签只能作信息标注 |
| 10 | GUI 筛选 | `gui/app.js:214` | 只有 fit/loaded/inference 等固定筛选 |

## 未决问题

- [x] 实现范围：远程策展目录 + 建议标签；**不做** Hub 关键词在线搜索（用户 2026-11-09 选择）。
- [x] 远程目录默认地址与自动更新默认值：ADR 采用「可配置 GitHub raw + 默认开启、可关闭」；评审时可调整，不影响 C1..C8 的验收口径。

## 卡点

- [x] 每条需求句都有 C 编号约束（无遗漏、无「待定」）
- [x] 需求方确认卡片（2026-11-09 用户确认，ADR 转 Accepted）
