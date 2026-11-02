# 开发工作流（强制）

本规则覆盖 AGENTS.md 的门禁链，任何代码改动都必须遵守。

- **先定性，再选门禁**（详见 [AGENTS.md](../../AGENTS.md) 的「门禁分级」）：
  - L0 仅文档（`*.md`）→ `make check-docs`（`gitkeep-check` + `adr-check`），不跑测试与前端门禁。
  - L1 仅配置（`Makefile` / `pyproject.toml` / `ruff.toml` / lock / workflow / `deploy/` / `scripts/` 等）→ `make check-fast`；触及测试发现、依赖、打包或 CI 行为时加跑 `make test`。
  - L2 代码（`src/` / `tests/` / `frontend/src` / 迁移等）→ `make flow`（`check-fast` → `test`）。
  - 混合改动取最高级；拿不准按 L2。
- L2 每次改完代码依次执行：`make check-fast` → `make test` → 更新项目文档 → `make push`。
- 任一步失败就修，修完重跑，全过才算完成（`make flow` = `check-fast` + `test` 的快捷入口）。
- 每一期（P0/P1/P2）落地后回填 ADR 的「落地状态」表与「交接」节（仅当该需求有 ADR，即 S2 项；见 [.trae/rules/design.md](design.md) 的「方案分级」）；`make adr-check` 校验落地表、交接节、Related 链接与 CHANGELOG 关联（已并入 `make check-docs` / `make check-fast` / `make check` / `make flow`）。
- 中途换会话 / 换 agent 时，先读 `docs/adr/README.md` 的「落地阶段」与目标 ADR 的「落地状态」「交接」，再跑一次基线（代码任务 `make check-fast`，文档 / 配置任务 `make check-docs`）。
- 禁止 `--no-verify`、禁止跳过 pre-commit（prek）。
- commit message 用 conventional commits（`feat:` / `fix:` / `docs:` / `refactor:` / `test:` / `chore:`）。
- 不擅自修改 git config；提交身份缺失时向用户确认，不要自行写入。
- 不在未获明确授权时 force push、不直推 main/master、不打 tag（打 tag 属上线卡点，须人工确认）。
