# 冗余梳理与精简：已执行项与待办清单

> 生成时间：2026-10-14（持续更新）。本文件记录「证据驱动」的冗余梳理与精简：
> 已完成的改动见 CHANGELOG「未发布」，下面只列**已分析清楚但尚未执行**的项，
> 每条都附可复现证据与风险判断，便于后续按优先级推进。

## 已完成（本轮）

- 死代码 / 死配置：Sphinx 文档骨架、`.readthedocs.yaml`、`tox.ini`、
  `plugins/{forms,workflows}` 空目录、`.gitignore` 的 `.tox/` 与重复 `.mypy_cache/`
- 未使用依赖：前端 zod / @tanstack/react-table / react-hotkeys-hook / zundo / immer
- 重复实现：后端 CSV 编码探测统一到 `decode_bytes_auto`（并修掉 import_tasks 的
  硬编码 utf-8 缺陷）；前端抽出 `utils/{paletteColor,datetime,formatSize}`
- 测试收敛：删除 12 个与主测试重复的补覆盖率用例、2 个零引用 fixture、1 个空壳测试
- 覆盖率口径：Makefile `cov` 与 CI 统一为 `--cov=cndb`（此前本地裸 `--cov` 虚高到 97.9%）
- 超大文件拆分：`cli/seed.py`、`api/types.ts`、
  `ImportPanel.tsx`、`GridCell.tsx`、`GanttView.tsx`、
  `DocumentsPage.tsx`、`CalendarView.tsx`、`KanbanView.tsx`、`workspaces/routers/workspaces.py`、
  `tables/services/fields/field_ops.py`、`tables/routers/bulk.py`、
  `tables/services/transfer/normalize.py`、`cli/users.py`、`cli/restore.py`、
  `FileImportPreview.tsx`（部分，见下）（详见 CHANGELOG「未发布」）
- 前端死导出清理：`LoginResponse`、`WorkspaceTrashResponse`、`getTagColor`、
  `ANTD_COLOR_NAMES`、`loadTableSettings`/`saveTableSettings`（后者与 zustand persist
  共用同一个 localStorage key `cndb_table_settings`，双写隐患已消除）
- 重复类型收敛：`ViewMode`（store 副本删除）、`WorkspaceRole`（页面内副本删除）
- 配置一致性：`ruff==0.16.8` 精确固定、`ci.yml` 用 YAML 锚点去重 paths、
  `Makefile .PHONY` 补齐、`pytest.ini` 补 `--dist` 生效条件说明
- 配置文件去重：覆盖率阈值收敛到 `pyproject.toml` 一处（`Makefile` / 两套 CI 不再重复传
  `--cov-fail-under`）、`.gitignore` 重复与失效规则（`frontend/node_modules/`、重复 `.idea/`、
  Sphinx 残留）、`.dockerignore` 的 tox 残留、coverage `omit` 里恒无效的 `tests/*`；
  `docker-compose.prod.yml` 修正 `CNDATABASE_URL` → `DATABASE_URL` 拼写

## 一、超大文件拆分：剩余候选

以下为仍超过 500 行的源码文件中**优先处理的候选**（按收益排序）；全量 >500 行清单见
本节末尾的「其余 >500 行文件」。

### 后端

| 文件 | 行数 | 建议拆分 |
|---|---|---|
| `src/cndb/gui/main_window.py` | 1041 | 按 Tab 拆 `gui/tabs/{serve,backup,info}.py`；**`main` 必须留在 `main_window.py`**（`pyproject` 的 `cndbw` entry point 指向它）。coverage 已 omit `gui/**`，无覆盖率风险，也没有 gui 测试（缺少回归网，拆分后务必手工冒烟 `cndbw info`） |
| `src/cndb/plugins/tables/services/core/records.py` | 957 | 拆 `core/{row_values,row_crud,row_bulk}.py`；**本项循环导入风险最高**（与 `query.py`/`ddl.py` 互依，现靠函数内延迟 import 规避，拆分须保留） |
| `src/cndb/plugins/tables/services/importing/importer.py` | 906 | `Importer` 类单类 776 行 / 19 方法，可按方法族拆 mixin |
| `src/cndb/plugins/documents/services/content.py` | 905 | 拆 `content/{crud,trash,links,tags,fts,serialize}.py`；FTS/软删子树/标签计数高耦合，成本较高 |

### 前端

| 文件 | 行数 | 建议拆分 |
|---|---|---|
| `src/pages/grid/GridPage.tsx` | 1276 | 外置 `gridFallbacks.tsx`、`modeSwitch.ts`、`useGridPageRows.ts`、`useGridPageModals.ts`；主组件 1140 行主要是状态与回调，抽 hook 有一定回归风险 |
| `src/pages/grid/view-config/viewOptionSchema.ts` | 562 | 按视图拆 `options/{kanban,calendar,gantt,wbs,matrix,chart}.ts`；**测试要求保留 7 个具名导出** |
| `src/pages/grid/views/WbsView.tsx` | 588 | 树构建与渲染行两分 |

### 其余 >500 行文件（完整清单）

以上为优先候选。全仓共 34 个 >500 行的源码文件，其余 27 个如下 ——
多数是拆分后落在 500–660 行、且已单一职责的文件，暂不作为优先候选。

**后端（17）**：

- `src/cndb/cli/seed/samples.py` 695
- `src/cndb/plugins/documents/routers/documents.py` 666
- `src/cndb/plugins/tables/services/importing/api_fetch.py` 628
- `src/cndb/plugins/tables/services/transfer/table_create.py` 613
- `src/cndb/cli/backup.py` 601
- `src/cndb/plugins/tables/routers/fields.py` 590
- `src/cndb/plugins/tables/routers/tables.py` 582
- `src/cndb/plugins/tables/services/importing/field_mapping.py` 573
- `src/cndb/gui/checks.py` 560
- `src/cndb/plugins/change_requests/services.py` 550
- `src/cndb/plugins/tables/services/core/lookups.py` 541
- `src/cndb/plugins/tables/routers/views.py` 539
- `src/cndb/plugins/workspaces/routers/workspaces.py` 509
- `src/cndb/plugins/tables/services/fields/select_options.py` 508
- `src/cndb/plugins/tables/services/core/ddl.py` 505
- `src/cndb/plugins/tables/routers/bulk_import.py` 504
- `src/cndb/plugins/tables/services/fields/field_import.py` 502

**前端（10）**：

- `src/pages/fields/FieldManager/index.tsx` 678
- `src/pages/grid/views/GanttView.tsx` 655
- `src/pages/workspace/TablesList.tsx` 617
- `src/pages/import-export/ImportExportDialog/ImportPanel.tsx` 615
- `src/pages/documents/DocumentTree.tsx` 607
- `src/pages/grid/views/ChartView.tsx` 587
- `src/pages/settings/TableSettingsPage.tsx` 586
- `src/pages/documents/DocumentsPage.tsx` 553
- `src/pages/fields/FieldManager/typeConfigPanel.tsx` 544
- `src/pages/settings/WorkspaceSettingsContent.tsx` 525

> 拆分通用的两条约束（多次拆分已验证）：
> 1. **monkeypatch / mock 目标必须指向 canonical 模块**。把实现移出后，若测试仍 patch 原模块的同名属性，patch 会静默失效（seed 的 `_get_datasets_dir`、reports 的 `_run_command` / `_cjk_font_candidates` 都踩过）。
> 2. 测试使用的具名导出要在原文件 `export { ... } from '...'` 或 `__all__` 里保留（GridCell 4 个、DocumentsPage 的 `buildSeriesStats`、DocumentTree 4 个、viewOptionSchema 7 个、FieldManager 2 个、ViewConfigDialog 的 `FilterRule`/`SortRule`）。

## 二、僵尸 Alembic 迁移（4 个，需谨慎）

`forms` / `workflows` 插件源码已删除，但 4 个迁移仍在链上，会在纯迁移链库中建出 6 张没人用的表
（`forms_formdefinition`、`workflows_workflow*`、`workflows_approvalinstance/action`），随后由
`e8f9a0b1c2d3_github_issue_pr_refactor.py` 全部 drop —— 对新库净效果为零，属历史包袱。

- 涉及文件：`62817ea574fa_add_workflows_tables.py`、`b8a7c6d5e4f3_add_forms_formdefinition.py`、`c1d2e3f4a5b6_add_form_approval.py`、`d5c6a7b8e9f0_add_change_request_fields.py`
- **不能直接删文件**：迁移链是单链，删节点会断链。需改 `949b0d8ddf3f` 的 `down_revision` 为 `6399e5f0f61f`、`d5c6a7b8e9f0` 的为 `a9c0d1e2f3b4`，或改为在链尾追加 drop 迁移。
- 建议：**暂不执行**。收益（少量迁移文件）低于风险（已有部署库升级路径），除非确认无存量库依赖该链。

## 三、测试层的剩余重复

- **跨文件重复的同类 fixture**（16 个文件各自重写 `ws` + `table`，12 个文件各自重写 `owner`/`auth_owner`，7 个文件各自重写 `ws_with_auth` 家族，6 个文件各自重写 `test_session`）。可上提到 `tests/conftest.py`，但会改动大量测试文件的可见 fixture 名，属高风险 churn —— 建议按文件分批做，每批跑一次覆盖率对比。
- **e2e 与单测重叠**：`frontend/tests/e2e/critical/navigation.spec.ts:22-65`（Tab 顺序/可见性/路由）与 `MainLayout.test.tsx:138-208` 覆盖同一行为，且单测更强（还断言激活态）。e2e 独有价值的是「seed 数据下的表行数=8」等用例。可删 navigation 的 3 个重叠用例；e2e 目前 `continue-on-error: true`，删除零风险但也零收益。
- **前端测试样板未收敛**：26 个测试文件各自 `import { http, HttpResponse } from 'msw'` + `server.use(...)`；4 个文件（`api/hooks.test.tsx`、`api/hooks.placeholder.test.tsx`、`WorkspaceList.test.tsx`、`WorkspaceSettingsContent.test.tsx`）自建 Provider 栈而不用 `src/test/render-providers.tsx`。

## 四、已决策并执行：删除 `is_primary` 与 6 个伪系统字段类型

**结论（2026-10-14，用户确认「无存量库使用」）**：这些分支全部是恒假分支，已删除
（提交 `eaf2de0`，19 文件 +51/-100）。

证据链（决策时的完整依据）：

- 后端权威字段类型清单共 19 种（`tables/field_types/registry.py`）：
  text / longtext / number / float / boolean / date / datetime / select / multiselect /
  email / url / phone / percentage / timestamp / link / lookup / attachment / image / json。
  **没有** `auto_id`、`formula`、`created_time`、`updated_time`、`created_by`、`updated_by`。
- `FieldResponse`（`tables/schemas/models.py`）**没有** `is_primary` 字段；
  `grep -r is_primary src/cndb` 命中 0，6 个伪类型作为字段类型同样 0 命中。
- 关键前提：`registry.normalize_field_type` 对**未知类型原样放行**，所以这些分支只可能
  来自存量库的 `tables_datafield.field_type` 行 —— 用户确认无此依赖后即可安全删除。

删除范围：`FieldType` 联合类型中的 6 个类型名、`Field.is_primary`、`fieldTypeMeta` 的
6 条系统类型兜底、`cellValueUtils` 的只读集合（现只剩 `lookup`）、`fieldValueFormat` 的
标题字段优先、`viewOptionSchema` 的伪类型匹配与 **`includePrimary` 选项**（其唯一作用
就是回退到 is_primary 字段）、`FieldList`/`FieldManager` 的 PK 徽标与相关禁用逻辑、
`calendarUtils`/`MatrixView` 的标题回退。测试同步删 3 个前提已消失的用例、改写 2 个、
清理 10 个文件的 mock。

**保留**：`long_text` / `decimal` / `multi_select`（后端 `_FIELD_TYPE_ALIASES` 的真实别名）
与 `json`（registry 真实类型）；`created_by` / `updated_by` 作为**记录元数据**的用法
（`RowResponse` 字段与剥离元数据的解构）逐处核对后原样保留。

> 若将来确需"主键/系统列"能力，应由后端补 `is_primary` 或系统字段类型，而不是让前端空判。

## 五、配置与门禁的剩余项

- **coverage omit 盲区**：`pyproject [tool.coverage.run].omit` 排除了 `cli/main.py`、`cli/seed/**`、`gui/**`、
  `tables/routers/files.py`。其中 seed 包与 gui 共约 3000 行不参与 95% 门禁。当前 `--cov=cndb` 实测 95.46%
  （阈值 95%，余量约 0.46pp），如需提高可信度可先把 `gui/checks.py` 纳入统计。
- **`frontend/scripts/bundle-budget.json`**：三条基线的 `commit` 字段全为 `""`，与其 `_readme` 宣称的
  可追溯性不符；`assetPattern` 依赖 Rollup 自动 chunk 命名，拆包后可能失配（历次拆分后仍匹配）。
- **e2e 不在 tsconfig 项目内**：`tsconfig.app.json` 只 include `src`，`tsconfig.node.json` 只 include
  `vite.config.ts` → `pnpm typecheck` 不覆盖 `frontend/tests/e2e/**` 与 `playwright.config.ts`。
- **`pytest.ini` 的 `--dist worksteal`**：仅在调用方传 `-n` 时生效（Makefile / CI 都传，裸 `pytest` 串行）；
  已在文件内加注释说明，未改动行为。
