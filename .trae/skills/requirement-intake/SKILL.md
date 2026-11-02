---
name: requirement-intake
description: Use when the user gives a raw product or feature request that has not yet been turned into an implementable spec for the cndb repo. Convert intent into a requirement card (docs/REQUIREMENT_CARD_TEMPLATE.md) with per-sentence implementable constraints, then hand off to adr-writer. Do not write code in this skill.
---

# 需求接入（cndb）

把口语需求转成**可实施约束**，产出需求卡片。此阶段不写代码。

## 输入

- 用户的需求原文（逐字保留）
- 当前仓库上下文：`AGENTS.md`、`CONTEXT.md`、`docs/adr/`

## 步骤

1. 逐字记录需求原句，不要在本阶段改写或「理解化」——此句是后续唯一的原文副本。
2. 逐句拆解为可实施约束并编号 C1..Cn：每条必须**可落库或可校验**（对应到具体 plugin / 表 / 字段 / 端点 / 前端页面）。
3. 用 Grep / Read 粗查疑似受影响的现有能力，按编号列出候选 `文件:行号`（精查留给 adr-writer）。
4. 填写需求卡片（模板 `docs/REQUIREMENT_CARD_TEMPLATE.md`）：约束表、现状影响面、未决问题。
5. 汇总需与需求方确认的决策点，作为「未决问题」列出。

## 产出

- 需求卡片 `docs/requirements/NNNN-<topic>.md`（编号与 ADR 同号）。

## 门禁 / 卡点

- 每条需求句都有 C 编号约束，无遗漏。
- 未决问题清单明确且全部有结论。
- 「决策记录表无待定」属于 ADR 的卡点，不在本阶段判定。
- **人工卡点**：需求方确认后，才交给 `adr-writer`。
