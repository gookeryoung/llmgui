---
name: adr-writer
description: Use when a requirement card or feature intent needs a design decision document in the cndb repo. Produce or update an ADR at docs/adr/NNNN-<topic>.md following docs/adr/TEMPLATE.md and the ADR 0004 house style, verifying the current state against real code with file:line references. Research and design only — do not implement.
---

# ADR 撰写（cndb）

把需求卡片 / 功能意图写成 ADR，作为实现前的唯一方案依据。此阶段只做调研与设计。

## 输入

- 需求卡片 `docs/requirements/NNNN-<topic>.md`（来自 `requirement-intake`）；无卡片时先补卡片
- 模板 `docs/adr/TEMPLATE.md`，范式参考 `docs/adr/0004-row-level-permission-rules.md`

## 步骤

1. 「口径」表沿用卡片的 C1..Cn 编号，只补「设计落点」一列；**不复制原文、不重新拆句**（原文唯一副本在卡片）。
2. **逐条核对现状**：用 Grep / Read 读真实代码，沿用卡片影响面编号逐条标「确认 / 排除 / 新增」并补 `文件:行号`，列出核心缺陷（现模型无法表达的那件事）。
3. 定义不变量 I1..In（安全 / 兼容 / 单一判定等）。
4. 按模板写 Decision 各小节：数据模型、判定入口、API 变更清单、边界与例外、迁移（幂等 + downgrade + 存量零迁移）、前端、测试清单、分期（P0/P1/P2）。
5. 收齐决策记录表 D1..Dn（议题 / 结论 / 依据 / 被否决项 / 代码落点），结论只写这一处；整体方案选型也占一行。
6. 必要时补「行业对照」「与既有 ADR 的关系」两节。
7. 写「落地状态」表（按分期列 P0/P1/P2，全部先填「未开始」）与「交接」节（当前阶段 / 已完成 / 下一步 / 已知坑），并把编号、标题、Status、落地阶段登记到 `docs/adr/README.md` 索引。

## 需求移除 / 变更（修订既有 ADR）

1. **先改 ADR，再动代码**：删或改对应「口径」行，Context、决策记录、测试清单与分期同步收缩，保持 ADR 只描述当前项目。
2. 已失效内容直接删除，不写「曾用方案 / 已废弃」叙述（留痕写 CHANGELOG）。
3. 需求整体作废或被替代：**直接删除**该 ADR 文件、`docs/adr/README.md` 索引行与全仓引用（git 历史可追溯）；不保留 `Superseded` / `Deprecated` 文档，编号不复用。
4. `make adr-check` 通过（索引 Status 一致、不存在失效状态文档、失效路径已清）。

## 产出

- `docs/adr/NNNN-<topic>.md`（Status: Proposed，含「落地状态」与「交接」节）。
- `docs/adr/README.md` 索引新增一行（编号 / 标题 / Status / 落地阶段 / 代码 / 测试 / CHANGELOG）。

## 门禁 / 卡点

- 决策记录表无「待定」项；新术语写入 `CONTEXT.md`。
- `make adr-check` 通过（Related 路径存在、索引覆盖本 ADR、落地表格式合法）。
- **人工卡点**：方案评审通过后，Status 才改 `Accepted`，再交给 `implementer`。
- 分批实现时，按 ADR 的 P0/P1/P2 分期推进。
