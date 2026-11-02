---
name: implementer
description: Use when implementing an Accepted ADR or a scoped feature/fix in the cndb repo. Implement in the ADR's phases (P0 -> P2) with small commits, running make check-fast as the inner feedback loop. Follow the plugin architecture and never bypass pre-commit.
---

# 实现（cndb）

按已 Accepted 的 ADR 落地代码。遵循该仓库的插件化架构与小步提交。

## 输入

- 已 Accepted 的 ADR（`docs/adr/NNNN-*.md`）
- 仓库约定：`AGENTS.md`、`.trae/rules/*.md`

## 步骤

1. 通读 ADR 的分期与「决策 → 代码落点」表，按 **P0 → P1 → P2** 顺序实现，每期一个内聚改动。
2. 后端遵循插件分层：`plugins/<name>/` 下 `models.py` / `routers/` / `schemas/` / `services/`；动态表 DDL 走 SQLAlchemy Core。
3. 迁移用 alembic：**幂等 + downgrade 可还原 + 存量零迁移**；新表须纳入 `plugins/workspaces/services/backup_io.py`。
4. 前端改动落在 `frontend/src/`，与后端 API 类型（`api/types/*`）对齐。
5. **内环**：每次改完立即 `make check-fast`（gitkeep + adr-check + ruff + pyrefly + tsc + eslint，秒级），红了先修再继续。
6. 每期完成后回填 ADR 的「落地状态」（期 / 状态 / 代码落点 / 测试 / 提交）与「交接」（当前阶段 / 下一步 / 已知坑），标「已落地」的期必须有真实代码落点与测试文件。
7. 小步提交，message 用 conventional commits。

## 门禁

- `make check-fast` 全绿方可进入下一期。
- 不 `--no-verify`、不跳 prek、不 force push。
- 缓存类收口：涉及记录读/权限收紧时，变更后清相关缓存（见 ADR 缓存条目）。
