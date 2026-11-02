# AI 自动驱动开发流（cndb）

> 目标：把「用户需求意图 → 方案 → 代码 → 测试 → 部署」全流程交给 AI 编排，由**机器门禁**裁定对错。

## 1. 设计原则

本仓库已具备完整、全部机器可判定的门禁：Makefile 分级门禁、prek/ 提交钩子、双 CI（GitHub Actions + CodeUp）、发布与部署流水线（含健康探测与自动回滚）。因此本开发流的核心不是让 AI「自证正确」，而是让 AI 的每一步都交给门禁裁决 —— **AI 的职责是让门禁变绿**。

| 原则 | 落地 |
|---|---|
| 门禁即裁判 | AI 不声称「做完了」，只看命令退出码：`make check-fast` / `test` / `cov` / `check` / `adr-check` / CI / `/health` |
| 阶段化 + 卡点 | 每阶段：AI 动作 → 产出物 → 客观门禁 →（人类）审批点 |
| 失败自愈闭环 | 门禁失败输出回灌 AI → 定位修复 → 重跑，直到全绿（[AGENTS.md](../AGENTS.md) 已定义） |
| 上下文常驻 | [AGENTS.md](../AGENTS.md)（流程 + 禁令 + 开工/收工交接）、[CONTEXT.md](../CONTEXT.md)（术语）、`docs/adr/`（决策 + 落地状态 + 交接）作为每轮必读 |
| 载体 | `.trae/rules/`（阶段约束）+ `.trae/skills/`（阶段角色）—— [.gitignore](../.gitignore) 已预留这两个目录 |

流水线：

```
需求意图 →[ADR 方案]→ 实现 →[测试]→ 门禁 → 文档/版本 → 推送 → 发布 → 部署(健康探测 + 自动回滚)
       ▲需求确认        ▲方案评审                            ▲上线 tag
```

## 2. 阶段总览

| 阶段 | 输入 / 触发 | AI 动作 | 产出物 | 客观门禁 | 人工卡点 |
|---|---|---|---|---|---|
| A 需求意图 | 用户口语需求 | 需求拆句（C1..Cn）、粗查影响面 | 需求卡片 `docs/requirements/NNNN-*.md` | 每句有 C 编号约束，未决问题清零 | ✅ 需求确认 |
| B 方案制定 | 需求卡片 | 逐条核对代码、定不变量、收决策 D 表、分期 | ADR `docs/adr/NNNN-*.md`（Proposed，结构见模板） | 卡片约束逐条有落点、D 表无「待定」；ADR → Accepted | ✅ 方案评审 |
| C 代码编写 | ADR | 按 P0→P1→P2 小步实现，每期回填 ADR 落地状态 | alembic 迁移 + plugin 分层 + 前端 | `make check-fast`（含 `adr-check`） | — |
| D 测试编写与验证 | 实现同轮 | 判定层 / 接口层 / 迁移 / 备份 / e2e 同步写测试 | `tests/test_*.py`、`*.test.tsx`、`tests/e2e/*` | `make test` → `make cov`(≥95%) → `make e2e` | — |
| E 集成门禁与文档 | 全绿 | 更新 README/CONTEXT/docs、回填 ADR 交接、bump 版本 | `make check`、CHANGELOG | `make check` + 双 CI（含 ADR Landing） | — |
| F 推送与发布 | 评审通过 | 推全部 remote、打 tag | PyPI + fspack 包 + GitHub Release | [release.yml](../.github/workflows/release.yml) | ✅ 上线 tag |
| G 部署 | tag 成功 | 观察部署与健康 | 生产实例 | [deploy-tencent.yml](../.github/workflows/deploy-tencent.yml) + `/health` | 自动回滚兜底 |

## 3. 阶段详解

### A 需求意图接入

把口语需求转成**需求卡片**（可实施约束 C1..Cn）；需求原文与约束编号此后只在卡片维护：

1. 需求原句逐字保留（唯一副本）；
2. 逐句拆成 C1..Cn 可实施约束表；
3. 粗查疑似受影响的现有能力，按编号给出候选 `file:line`（精查留给 B）。

产出 `docs/requirements/NNNN-<topic>.md`（模板 [REQUIREMENT_CARD_TEMPLATE.md](REQUIREMENT_CARD_TEMPLATE.md)）。

**门禁**：每条需求句都有 C 编号约束，未决问题清零。

**人工卡点【需求确认】**。

### B 方案制定

读代码逐条核实卡片影响面（确认 / 排除 / 新增，补 `file:line`），产出 ADR：

- 口径表沿用 C1..Cn，只补「设计落点」，不复制原文；
- 数据模型 / 判定入口 / API 变更清单 / 边界与例外 / 迁移 / 前端 / 测试清单 / 分期，结构以 [ADR 模板](adr/TEMPLATE.md) 为准；
- 决策结论只写进 D1..Dn 台账（含被否决项与代码落点）；Consequences 只写得失，不重复否决项。

**门禁**：卡片约束逐条有落点、D 表无「待定」；ADR 由 Proposed → Accepted；新术语写入 [CONTEXT.md](../CONTEXT.md)。

**人工卡点【方案评审】**。

### C 代码编写

按分期小步走：先 P0（模型 + 判定入口），再 P1（写路径接入），再 P2（端点 + 前端）。

内环每改完即 `make check-fast`（gitkeep + adr-check + ruff + pyrefly + tsc + eslint，秒级反馈）。

每期完成时回填 ADR 的「落地状态」（期 / 状态 / 代码落点 / 测试 / 提交）与「交接」（当前阶段 / 下一步 / 已知坑）；`make adr-check` 会校验标「已落地」的期确有代码落点与测试文件。

**强制**：不跳 prek、不 `--no-verify`、commit message 用 conventional commits。

### D 测试编写与验证

与实现同轮写测试：判定层单测 / 接口层 / 迁移与备份往返 / 前端 e2e。

命令链：`make test` → `make cov`（阈值 95%）→ 前端 `make frontend-coverage`（双门槛）→ `make e2e`（4 分片）。

覆盖率口径固定于 [pyproject.toml](../pyproject.toml) 的 `[tool.coverage]`（`fail_under=95`，omit 白名单固定），**不得注水**。

### E 集成门禁与文档

`make check`（[Makefile](../Makefile) 并行 7 项：gitkeep + adr-check + lint + typecheck + frontend-static + frontend-coverage + cov）。

文档更新：README / CONTEXT.md / `docs/*`、ADR 的「落地状态 + 交接」、`docs/adr/README.md` 索引，以及 `make bump`（CHANGELOG，条目带 `(ADR NNNN Pn)`）。

**门禁**：双 CI（[GitHub ci.yml](../.github/workflows/ci.yml) + [CodeUp ci.yml](../.codeup/pipelines/ci.yml)）通过、ADR Landing job 无 error（代码推送由 lint 步骤校验，纯 ADR / CHANGELOG 推送单独跑该 job）；bundle budget 不超基线 +5%。

### F 推送与发布

`make push`（推全部 remote + tags）→ 打 tag `vX.Y.Z` → [release.yml](../.github/workflows/release.yml)：PyPI（OIDC）+ Windows/Linux 原生打包（fspack）+ GitHub Release。

**人工卡点【上线 tag】**：打 tag 前必须人工确认。

### G 部署

release 经 pypi job 成功后调用 [deploy-tencent.yml](../.github/workflows/deploy-tencent.yml)（TAT 下发、服务器端构建镜像、预备份 → 重建 → 健康探测 → 失败自动回滚）。

**验证**：`/health` 通过；失败由自动回滚兜底。

## 4. 驱动机制（从文档到可执行）

1. **`.trae/rules/`**：写死每阶段约束（禁止 `--no-verify`、覆盖率不得注水、迁移必须幂等 + downgrade）。
2. **`.trae/skills/`**：把阶段做成角色 Skill —— requirement-intake / adr-writer / implementer / test-writer / gate-runner / release-manager，每个绑定到具体 `make` 命令。
3. **闭环**：门禁失败输出回灌 → 自动修复 → 重跑，直到全绿。
4. **工具化入口（按改动类型分级）**：`make check-docs`（L0 纯文档：`gitkeep-check` + `adr-check`）/ `make check-fast`（L1 纯配置）/ `make flow`（L2 代码：`check-fast` 含 `adr-check` → `test`）。分级规则见 [AGENTS.md](../AGENTS.md)「门禁分级」，混合改动取最高级；文档更新与 `make push` 仍需人工判断，不纳入。
5. **落地与交接门禁**：`make adr-check`（[check_adr_landing.py](../scripts/check_adr_landing.py)）校 ADR 落地状态表 / 交接节 / Related 链接 / CHANGELOG 关联；换会话或换 agent 时按 [AGENTS.md](../AGENTS.md) 的「开工前 / 收工前」回填与恢复上下文。
6. **Hook**：编辑后自动触发 `make check-fast`（提交侧已由 prek 覆盖）。
7. **审批点收敛为 3 个**：需求确认 / ADR 评审 / 上线 tag。

## 5. 项目特定护栏

- 禁 `--no-verify`、禁跳过 prek；conventional commits。
- 门禁按改动类型分级（L0 文档 → `make check-docs`；L1 配置 → `make check-fast`；L2 代码 → `make flow`），只改文档 / 配置不跑全量 `make flow`；混合改动与拿不准的情况按 L2 全链执行。
- ADR 标「已落地」的期必须有代码落点与测试文件；落地状态与交接每期回填（`make adr-check` 校验）。
- 需求移除 / 变更先清理 ADR（删失效段落与决策行），不保留历史方案段；ADR 被替代或需求作废则**直接删除**该文件、`docs/adr/README.md` 索引行与全仓引用（git 历史可追溯），不保留 `Superseded` / `Deprecated` 文档，编号不复用。
- 覆盖率不得注水（阈值 95%，omit 白名单固定）。
- 迁移幂等 + downgrade 可还原 + 存量零迁移。
- 新表须纳入 `backup_io`，旧备份按缺省还原。
- 权限不变量：I1 天花板 / I3 存量不变 / I6 无泄露（见 [ADR 0004](adr/0004-row-level-permission-rules.md)）。
- 规则变更后清记录缓存，防收紧后泄漏。
- 单机部署串行（`concurrency: deploy-tencent-prod`），失败自动回滚。

## 6. 落地清单

- [x] `make flow` 编排目标（固化 AGENTS 四步中的「门禁两步」）—— [Makefile](../Makefile)
- [x] 门禁分级 `make check-docs` / `check-fast` / `flow`（文档 / 配置 / 代码），规则见 [AGENTS.md](../AGENTS.md)「门禁分级」—— [Makefile](../Makefile)
- [x] ADR 模板（从 0004 抽骨架）+ 需求卡片模板 —— [docs/adr/TEMPLATE.md](adr/TEMPLATE.md)、[docs/REQUIREMENT_CARD_TEMPLATE.md](REQUIREMENT_CARD_TEMPLATE.md)
- [x] `.trae/rules/*.md` 阶段约束 —— [dev-workflow](../.trae/rules/dev-workflow.md)、[design](../.trae/rules/design.md)、[testing](../.trae/rules/testing.md)
- [x] `.trae/skills/*` 阶段 Skill —— requirement-intake / adr-writer / implementer / test-writer / gate-runner / release-manager
- [ ] （可选）编辑后自动 `check-fast` 的 hook
- [x] ADR 落地门禁 `make adr-check` + CI 的 ADR Landing job —— [check_adr_landing.py](../scripts/check_adr_landing.py)、[ci.yml](../.github/workflows/ci.yml)
- [x] ADR 索引与开工/收工交接协议 —— [docs/adr/README.md](adr/README.md)、[AGENTS.md](../AGENTS.md)
