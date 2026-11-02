---
name: gate-runner
description: Use to run the cndb repo's objective gates (make flow / make check, local CI) and drive failures to green before a change is considered done. Inspect failing output, fix the root cause, and re-run until all gates pass. Do not use workarounds such as skipping hooks or excluding tests.
---

# 门禁执行与自愈（cndb）

让机器门禁裁定对错：AI 的职责是让门禁变绿，而非声称完成。

## 门禁清单（对照 AGENTS.md）

1. `make check-fast`：gitkeep + adr-check + ruff + pyrefly + 前端 tsc/eslint（内环，秒级）。
2. `make flow`：`check-fast` + `test`（实现/验证快捷入口）。
3. `make test` → `make cov`：后端单测 + 覆盖率门槛 95%。
4. 前端 `make frontend-coverage`（双门槛）、`make e2e`（4 分片）。
5. `make check`：全套门禁（gitkeep + adr-check + lint + typecheck + frontend-static + frontend-coverage + cov，并行）。
6. `make adr-check`：ADR 落地状态表 / 交接节 / Related 链接 / CHANGELOG 关联；已落地的期必须有代码落点与测试文件。
7. CI：GitHub Actions `ci.yml`（含 ADR Landing job）、CodeUp `ci.yml`；bundle budget 不超基线 +5%（见 `docs/BUNDLE_BUDGET.md`）。

## 步骤

1. 逐项运行；捕获失败命令的完整输出。
2. 定位**根因**（不猜、不掩盖），改代码或补测试。
3. 重跑失败项，直到全绿；再跑一次完整链确认无回归。
4. 门禁全绿后，进入「更新项目文档 → `make push`」两步（见 `dev-workflow` 规则）。

## 铁律

- 不 `--no-verify`、不跳 prek、不通过 `-k`/`--ignore` 排除失败用例、不放宽覆盖率门槛。
- 不靠「删掉落地表 / 把状态改回未开始」让 `adr-check` 变绿；实际没做回填要补实现或拆期。
- 迁移必须幂等 + downgrade 可还原。
- 门禁失败的修复要追溯根因，不允许「注释掉断言」式绕过。
