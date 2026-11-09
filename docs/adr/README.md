# ADR 索引

本项目采用 ADR（Architecture Decision Record）记录 S2 重大项的方案与决策。新增 ADR 前先按 [.trae/rules/design.md](../../.trae/rules/design.md) 定级；模板见 [TEMPLATE.md](TEMPLATE.md)。

状态约定：

- **Proposed**：方案已写出，待人工评审；不得据此开始实现。
- **Accepted**：评审通过，可进入实现；决策记录表不得有「待定」。
- **删除**：被替代或需求作废时直接删除文件与索引行，编号不复用，追溯看 git 历史。

| 编号 | 标题 | Status | 落地阶段 | 代码 | 测试 | CHANGELOG |
|---|---|---|---|---|---|---|
| [0001](0001-local-llm-runtime-finetune.md) | 本地大模型运行与微调框架（Rust + Python） | Accepted | P0 部分 / P1 进行中 / P2a 部分 / P2b 已落地 / P3 进行中 | `crates/`、`python/`、`gui/` | `cargo test --workspace`、`pytest python/tests` | [CHANGELOG](../../CHANGELOG.md) |
| [0002](0002-model-management.md) | 模型管理（模型库 / 发现下载 / 运行时生命周期 / 训练产物） | Accepted | P0/P1 已落地 / P2a 部分 / P2b 部分 / P3 部分 | `crates/llmgui-models`、`crates/llmgui-server`、`crates/llmgui-download`、`gui/` | `cargo test --workspace` | [CHANGELOG](../../CHANGELOG.md) |
| [0004](0004-qwen3-coder-30b-throughput.md) | Qwen3-Coder-30B-A3B 单用户吞吐 90–120 tok/s | Accepted | P0 / P1 / P2a / P2b 全部落地（实测 232.5 tok/s） | `crates/llmgui-core`、`crates/llmgui-backend-api`、`crates/llmgui-backend-llamacpp`、`crates/llmgui-server`、`crates/llmgui-cli`、`models/`、`docs/bench/` | `cargo test`、`make bench-turbo` | [CHANGELOG](../../CHANGELOG.md) |

## 关联文档

- 需求卡片：[0001](../requirements/0001-local-llm-runtime-finetune.md) · [0003](../requirements/0003-model-management.md) · [0004](../requirements/0004-qwen3-coder-30b-throughput.md)
- 方向调研：[runtime](../research/runtime.md) · [finetune](../research/finetune.md) · [download](../research/download.md) · [model-management](../research/model-management.md)
- 术语表：[CONTEXT.md](../../CONTEXT.md)
