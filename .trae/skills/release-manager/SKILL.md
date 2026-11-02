---
name: release-manager
description: Use when a verified, gate-green change in the cndb repo is ready to publish and deploy. Push remotes, bump version / tag after human approval, then watch the release and Tencent Cloud deploy pipelines. Tagging and deployment are human-gated checkpoints; never tag without explicit approval.
---

# 推送与发布部署（cndb）

门禁全绿 + 文档已更新后的收尾链路。打 tag 与部署均为**人工卡点**。

## 步骤

1. **推送**：`make push`（推全部 remote + tags）。若分支无 upstream，先 `git push -u origin <branch>`。
   推送前确认：ADR「落地状态」与「交接」已回填、CHANGELOG 带 `(ADR NNNN Pn)` 条目、`make adr-check` 无 error。
2. **版本 / 变更记录**：`make bump patch|minor|major`（bump-my-version，自动打 tag 并写 CHANGELOG）。
3. **上线卡点**：打 tag `vX.Y.Z` 前必须**人工确认**（属正式上线动作）。
4. **发布**：tag 触发 `release.yml` → PyPI（OIDC）+ Windows/Linux 原生打包（fspack）+ GitHub Release。
5. **部署**：`release.yml` 的 pypi job 成功后调用 `deploy-tencent.yml`（TAT 下发、服务器端构建镜像、预备份 → 重建 → 健康探测 → 失败自动回滚）。
6. **验证**：健康端点 `/health` 通过；失败由自动回滚兜底（单机部署串行，`concurrency: deploy-tencent-prod`）。

## 参照

- 部署链路与服务器准备：`docs/DEPLOY_TENCENT.md`
- 工作流：`.github/workflows/release.yml`、`.github/workflows/deploy-tencent.yml`

## 铁律

- 未获人工确认，不打 tag、不触发部署。
- 不 force push、不直推 main/master。
- 部署失败先看 TAT 回传日志与 `docker logs --tail 100 cndb`，确认是否已自动回滚到上一版本。
