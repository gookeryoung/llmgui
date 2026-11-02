# 测试与覆盖率规范

- 测试与实现**同轮**编写，不留「以后再补」。
- 分层覆盖：判定层单测、接口层、迁移（幂等 + downgrade）、备份往返（含旧备份兼容）、前端单测、e2e。
- 后端覆盖率门槛 **95%**（口径固定在 `pyproject.toml` 的 `[tool.coverage]`：`fail_under` + omit 白名单），前端双门槛见 `frontend/scripts/check-coverage.mjs`。
- 不得注水覆盖率（不为凑数写无断言用例、不改 omit 白名单放宽门槛）。
- 命令：`make test`（不含覆盖率）→ `make cov`（含门槛）→ 前端 `make frontend-coverage` → `make e2e`（4 分片）。
- e2e 是硬门禁；抖动由 CI retries 兜底，不要通过跳过用例规避失败。
