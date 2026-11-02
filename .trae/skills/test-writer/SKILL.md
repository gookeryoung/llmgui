---
name: test-writer
description: Use when adding or extending automated tests in the cndb repo alongside an implementation. Cover the ADR test list across judgement-layer unit tests, API tests, migration/backup round-trip, frontend unit tests, and Playwright e2e, then validate against the repo's coverage gates. Do not water down coverage.
---

# 测试编写与验证（cndb）

与实现同轮补测试，并跑覆盖率门槛。

## 输入

- 已实现的改动 + ADR 的「测试清单」
- 约定：`.trae/rules/testing.md`

## 步骤

1. 按 ADR 测试清单分层补用例：
   - **判定层单测** → `tests/test_*.py`
   - **接口层** → 路由 / API 行为
   - **迁移 / 备份** → 幂等 + downgrade + 备份往返 + 旧备份兼容
   - **前端单测** → `frontend/src/**/*.test.tsx`（vitest）
   - **e2e** → `frontend/tests/e2e/{smoke,critical}/*.spec.ts`（Playwright）
2. 断言行为而非实现细节；覆盖边界与例外（ADR 的「边界与例外」章节逐条落到用例）。
3. 运行验证：`make test` → `make cov`（后端门槛 95%）→ 前端 `make frontend-coverage` → `make e2e`。
4. 把新用例文件回填到 ADR「落地状态」表的「测试」列（需与代码落点同行的期匹配），再跑 `make adr-check` 确认路径可达。

## 门禁

- 后端覆盖率 ≥ 95%（`pyproject.toml` 的 `[tool.coverage]`，omit 白名单固定）。
- 前端双门槛通过；e2e 全绿。
- **不得注水**：不写无断言用例、不放宽 omit 白名单。
