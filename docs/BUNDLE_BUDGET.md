# 前端体积预算门禁（Bundle Budget）

关键 chunk 的 gzip 体积是有预算的，超限 CI 直接红。本文说明**为什么要有这道门禁**、
**超限时先做什么**、以及**怎样合法调整基线**。

- 校验脚本：`frontend/scripts/bundle-budget.mjs`
- 基线数据：`frontend/scripts/bundle-budget.json`
- CI 步骤：`.github/workflows/ci.yml` → `Bundle size budget`

## 它防的是什么

体积回归在 code review 里几乎看不见 —— diff 里没人会注意「多了一个 import」，
但产物可能就多了几 KB gzip。历史上 GridPage 是这样一路涨到超预算的：

| 时间 | 改动 | GridPage gzip |
|---|---|---|
| 2025-07 | 分包瘦身基线 | 39.8 KiB 起 |
| 2026-09-26 | 排序三态、列宽估算/拖宽、列序 DnD | +2.5 KB |
| 2026-10-07 | 图表视图接线 | +1.8 KB |
| 2026-10-08 | 数据治理向导（GovernanceDialog） | +1.9 KB |
| 2026-10-10 | 视图筛选/排序规则值校验（385 行新模块） | +2.26 KB → **超限** |

每一步单独看都「合理」，合起来就是每次打开表页都要多下载的一批代码。
所以门禁要防的从来不是某一次增长，而是**没人认领的持续膨胀**。

## 超限时先做什么

脚本失败时会打印超限 chunk 及其内部**体积最大的 Top-10 源文件**（归因表由构建期
产出的 `dist/.vite/chunk-modules.json` 提供）：

```
[bundle-budget] ── GridPage（表页闭包） 超限 1.28 KiB（当前 43.07 KiB，上限 41.79 KiB）
  GridPage-xxxx.js 内体积最大的源文件（压缩前源码字节，分母为该 chunk 源码合计 xx KB）：
     1.   50.6 KB   30.1%  src/pages/grid/GridPage.tsx
     2.   19.3 KB   11.5%  src/pages/grid/view-config/viewOptionSchema.ts
     …
```

按这个顺序处置：

1. **看归因表：体积是不是来自「点了才出现」的 UI？**
   Dialog / Modal / Drawer / Manager 这类组件只有用户操作后才可见，
   却常被图省事静态 import 进页面入口 —— 于是跟着首屏一起下载。
   改成 `React.lazy` 即可（GridPage 里 `FieldManager`、`GovernanceDialog`、
   `ViewConfigDialog`、`TableSettingsModal`、`RowDetailDrawer` 都是这个写法）：

   ```ts
   const XxxDialog = lazy(() => import('@/pages/xxx/XxxDialog'))
   // …
   <Suspense fallback={<ModalFallback />}>{open && <XxxDialog … />}</Suspense>
   ```

   2026-10-10 就是这样把 GridPage 从 43.07 KiB 降到 27.70 KiB 的 **（-36%）**，
   比任何压缩手段都有效。

2. **检查是否误把重型依赖拉进了关键路径**：`echarts`、`jspdf`、
   `html2canvas-pro`、`nunjucks` 等必须走 lazy，绝不静态链入首屏 chunk。

3. **确认增长不可避免**，才更新基线 —— 见下一节。

> 顺带一提：介于基线与上限之间时脚本标记为 `warn` 并在末尾汇总。
> 「慢涨」要在这一步就看见，别等它攒到 FAIL。

## 第三方依赖的隐性膨胀（2026-10-20 案例）

归因表里如果 Top-10 全是 `node_modules/*` 而业务代码只有几 KB，说明问题不在本仓库代码，
而在**依赖的打包声明**。这一轮主入口超限 0.07 KiB（190.10 / 上限 190.03）就是这种：

- 现象：主入口里出现 `rc-field-form` 95.4 KB + `@rc-component/async-validator` 39.7 KB，
  但首屏（App 壳 + 登录页之前的骨架）根本不渲染表单。
- 链路：`Button` 的 style token → `antd/es/color-picker` → `antd/es/form/context`
  → `import { FormProvider } from 'rc-field-form'`。
- 根因两处（都在 rc-field-form 侧，仓库代码无能为力）：
  1. `package.json` 没写 `"sideEffects"`，Rollup 只能保守保留它的每个 import；
  2. `es/index.js` 顶层给默认导出挂静态属性（`RefForm.FormProvider = …`），
     属性赋值被判为副作用，整个 barrel 无法裁剪。
- 修法：`pnpm patch rc-field-form@2.7.1`（见 `frontend/patches/`）——
  补 `"sideEffects": false` **+ 只删掉 barrel 顶层那几行静态属性赋值**。antd 会在自己的
  `Form` 上重挂 `Item / List / useForm / useWatch / Provider`，默认导出的静态属性无人使用。
- **回归坑（2026-10-20）**：当时把 barrel 顺手写成 `export default FieldForm`（裸组件），
  默认导出就丢了 `React.forwardRef` 包装 —— antd 的 `Form` 靠 ref 取表单实例，于是 dev 环境
  报「Function components cannot be given refs」、点登录报
  「**Cannot add property current, object is not extensible**」，登录页完全不可用。
  正确改法是保留 `var RefForm = React.forwardRef(FieldForm)`，只删静态属性挂载：
  摇树收益不变（主入口仍 175.2 KiB），ref 语义也保住。
  这个坑**单测守不住**——vitest 里 antd 解析到 rc-field-form 的 CJS(`lib`)，而 dev/build 用
  ES(`es`)，坏版本在单测里照样通过（实测）。因此双保险：
  `frontend/scripts/check-patches.mjs` 在 `pnpm dev` / `pnpm build` / `make check-fast` 前校验
  补丁语义（forwardRef 存在、无静态属性赋值、`sideEffects: false`）；
  `make e2e-dev` / `pnpm e2e:dev` 跑在 **vite dev** 上的 Playwright 用例从行为侧兜底
  （真实加载 es barrel，登录页崩溃这类回归直接红）。

结果：主入口 **190.10 → 175.22 KiB（-7.8%）**，表单代码回到按需 chunk
（`useForm-*.js`，21.9 KB，随表单页加载），基线同步下调到 175.22 KiB 锁住净收益。

**排查手法**（下次遇到同形态直接复用）：先看归因表的 `node_modules` 占比，
再沿「谁 import 了它」向上追。Vite 的 `chunk-modules.json` 只到 chunk 粒度，
需要模块级 importers 时临时加一个 `buildEnd` 钩子读 `this.getModuleInfo(id).importers`
（用环境变量开关，别留在提交里），或者用 `pnpm analyze` 的交互式 treemap。

**判据**：首屏 chunk 里出现「只有点了才出现的东西」= 代码问题；出现「整块库的边角
模块」= 依赖声明问题。前者拆 lazy，后者补 sideEffects 或 patch，都不该直接抬基线。

## 怎样调整基线

**不要手改 JSON。** 统一走脚本，理由会落盘到 `reason` / `updatedAt` / `commit`：

```bash
cd frontend && pnpm build

# 全部条目更新到实测值
pnpm bundle:budget:update --reason "feat: xxx，为何这部分代码必须随该 chunk 加载"

# 只更新某一项，并显式指定基线（留合理余量时用）
node scripts/bundle-budget.mjs --update --only GridPage --bytes 32768 --reason "…"
```

规则：

- 没有 `--reason` 直接拒绝执行 —— 这是留给 review 的证据，
  也是本门禁不当橡皮图章的关键；
- 单次上调超过 10% 会额外告警，请在 PR 说明里解释必要性；
- `bundle-budget.json` 的 `history` 字段自动追加每次调整的日期与理由（保留最近 20 条）。

## 第一道防线：ESLint 规则

`frontend/eslint.config.js` 中对 `src/pages/grid/GridPage.tsx`、`gridViewModals.tsx`
启用了 `no-restricted-imports`，禁止静态引入以 `Dialog` / `Modal` / `Drawer` /
`Manager` / `Editor` 结尾的模块（允许 `import type`）。写错的那一刻编辑器就会飘红，
附带怎么改的提示。

它拦的是**最常犯的那一步**；间接依赖（A 引 B，B 静态引面板）和第三方依赖失控，
仍然由预算门禁兜住 —— 两层是互补关系，不是二选一。

## 什么时候该质疑基线本身

如果某次 `--update` 让某 chunk 连续几个迭代单调上涨，
说明该 chunk 承担了太多职责，应该回头做**结构拆分**（把代码挪到 lazy chunk），
而不是继续抬预算。参考：GridPage 基线在 2026-10-10 因懒加载改造被主动下调约 20%。
