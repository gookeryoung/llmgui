# 开发工作流

## 门禁分级（先定性，再选门禁）

提交前用 `git diff --name-only` + `git status --short` 判定改动类型，**按最高级别执行**；拿不准按 L2 处理（fail-safe）。

| 级别 | 判定范围（只看文件路径） | 必跑 | 跳过 |
|---|---|---|---|
| **L0 文档** | 仅 `*.md`（`docs/`、`CONTEXT.md`、`CHANGELOG.md`、`AGENTS.md`、`.trae/rules/`、`README.md` 等） | `make check-docs`（`gitkeep-check` + `adr-check`） | 所有测试与前端门禁 |
| **L1 配置** | 仅 `Makefile`、`pyproject.toml`、`ruff.toml`、`pyrefly.toml`、`pytest.ini`、`uv.lock`、`frontend/package.json`、`frontend/tsconfig*.json`、`frontend/eslint*`、`docker-compose*.yml`、`deploy/`、`scripts/`、`.github/`、`.codeup/` | `make check-fast`；**触及测试发现 / 依赖 / 打包 / CI 行为**（`pyproject.toml`、`pytest.ini`、lock 文件、workflow、`scripts/` 里的门禁脚本）时加跑 `make test` | `cov` / `frontend-coverage` / `e2e`（除非本次显式改覆盖率或 e2e 口径） |
| **L2 代码** | 命中 `src/`、`tests/`、`frontend/src`、`frontend/tests`、`migrations/` 等 | `make flow`（`check-fast` → `test`）；阶段收尾 / 发版 / 改覆盖率口径时 `make check` | — |

- 只改文档或项目配置时不跑 `make flow`：`test` / `cov` / `e2e` 对 Markdown 与 Makefile 目标增删没有判别力，跑了只是等时间。
- 但**不能因为「主要改了文档」而降级**：同一次提交里既有代码又有文档 / 配置，按 L2 走全链。
- 分级只约束本地内环与提交前自查；CI 仍跑全量门禁，不因分级放宽。
- 禁止 `--no-verify`、禁止跳过 pre-commit（prek）；prek 对 L0 / L1 同样会触发，属预期。
- commit message 用 conventional commits：L0 用 `docs:`，L1 用 `chore:` / `ci:` / `build:`，L2 按改动性质（`feat:` / `fix:` / `refactor:` / `test:`）。

## 方案分级（先定性，再选方案载体）

不是所有需求都写 ADR，按复杂度分三级（判定标准详见 [.trae/rules/design.md](.trae/rules/design.md) 的「方案分级」）：

- **S0 小改**（bug 修复、微调、单点改动）：确认意图后直接实现，无需卡片与 ADR。
- **S1 常规功能**（跨 2+ 模块、多约束但无架构分叉）：写需求卡片（可轻量），方案并入卡片，**不强制 ADR**。
- **S2 重大项**（数据 / 迁移 / 权限口径、架构选型、对外契约、用户明确为重大项、方案有实质分叉）：需求卡片 + ADR，走完整流程。
- 拿不准按上一级处理；实现中发现设计分叉，S1 升级为 S2 补 ADR。

## 代码改动（L2）执行链

- 每次改完代码必须依次执行：`make check-fast` → `make test` → 项目文档更新 → `make push`
- 任一步失败就修，修完重跑，全过才算完成

## 开工前（换会话 / 换 agent 必读）

1. 先读 [docs/AI_DRIVEN_DEV_FLOW.md](docs/AI_DRIVEN_DEV_FLOW.md) 与 [.trae/rules/](.trae/rules/) 下的规则；工具不自动加载时也必须手动读。
2. 找当前任务的 ADR：看 [docs/adr/README.md](docs/adr/README.md) 的「落地阶段」，再读 ADR 的「落地状态」与「交接」。
3. 跑一次基线：代码任务 `make check-fast`，纯文档 / 配置任务 `make check-docs`；不要假设上一轮全绿。

## 收工前

1. 回填 ADR 的「落地状态」与「交接」（做到哪期 / 下一步 / 已知坑）——仅当该需求有 ADR（S2 项）。
2. 更新 CHANGELOG（条目带 `(ADR NNNN Pn)`，S0 / S1 无 ADR 时正常写条目）与相关文档。
3. 按「门禁分级」跑到对应级别全绿再提交（L0 / L1 不跑 `make flow`；L2 至少 `make flow`）；推送按 [.trae/rules/dev-workflow.md](.trae/rules/dev-workflow.md) 执行。

## 需求移除 / 变更

1. 若该需求有 ADR（S2 项），先清理 ADR 再改代码：删去失效的口径行 / 现状条目 / 决策行 / 测试与分期，不留「历史方案」段落，保持 ADR 与当前项目一致；S0 / S1 需求直接改代码与文档。
2. ADR 被新方案替代或需求作废：**直接删除**该 ADR 文件、README 索引行及代码 / 文档中的引用，不在仓库保留 `Superseded` / `Deprecated` 文档；编号不复用，追溯看 git 历史（`git log -- docs/adr/`）。
3. 细则见 [.trae/rules/design.md](.trae/rules/design.md)；`make adr-check` 会拦截残留的失效状态文档，并校验索引 Status 一致。
