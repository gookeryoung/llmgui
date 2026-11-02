# llmgui Web GUI

零构建、离线可用的本地 Web 界面，由 `llmgui serve` 作为静态目录托管在 `/`。
全部为原生 HTML / CSS / JavaScript，无框架、无外部 CDN / 字体 / 图片，不需要 npm 或打包步骤。

## 启动

```powershell
# 在项目根目录构建并启动（mock 后端无需模型）
cargo run -p llmgui-cli -- serve --backend mock --port 11435

# 或使用真实后端
cargo run -p llmgui-cli --features llama -- serve --model <model.gguf>
```

然后打开 <http://127.0.0.1:11435/>。CLI 默认会在当前目录存在 `gui/` 时自动托管；
也可以用 `--gui-dir gui` 或环境变量 `LLMGUI_GUI_DIR` 显式指定。

> 不能直接双击 `index.html`：页面使用相对路径调用 `/healthz`、`/v1/models`
> 等接口，必须通过 `llmgui serve` 打开。若页面不是由目标服务托管，可在「设置」页填写
> API base；跨源访问还需要目标服务允许 CORS（服务端默认关闭 CORS）。

## 页面与功能

| 页面 | 功能 |
|---|---|
| 模型 Models | 内置默认模型清单（镜像 `models/v0.toml`，含用途 / 量化 / 许可 / 大小）；每项提供可复制的 `llmgui pull ...` 命令；展示 `GET /v1/models` 返回的服务端已加载模型 |
| 聊天 Chat | 消息列表、发送 / 停止、Enter 发送；`fetch` + `ReadableStream` 解析 SSE（`data:` 行、`[DONE]`）；显示 TTFT、tok/s、token 用量；temperature / max_tokens 可调；错误以气泡 + toast 提示 |
| 训练 Train | QLoRA 表单（base_model、dataset path、method、seq_len、micro_batch、grad_accum、LoRA r/alpha）；实时生成 YAML 预览与 `llmgui train --config train.yaml` 命令，可复制或下载 YAML |
| 设置 Settings | API base（留空 = 同源）、API key、主题（深 / 浅）；「测试连接」调用 `/v1/models` 验证连通性与 key |

顶部状态栏每 15 秒轮询 `GET /healthz`，显示 backend / model / version / uptime / 空闲 slots；
不可达时显示「离线」并给出原因，页面恢复可见或网络恢复时自动重试。

## 存储与隐私

- API base、API key、主题保存在本机 `localStorage`：
  - `llmgui.apiBase`
  - `llmgui.apiKey`
  - `llmgui.theme`
- API key 仅作为 `Authorization: Bearer <key>` 发送到上面配置的 API base，不发送到其他地址。
- 聊天内容只保存在当前页面内存中，刷新即丢失；页面无遥测、无外部请求。
- 所有动态文本（模型清单、聊天内容、错误信息、YAML 预览）均通过 `textContent` 写入，
  不使用 `innerHTML`；聊天内容不做 Markdown / HTML 渲染。

## 已知限制

- 训练页只生成配置与命令，**不会**调用训练 API；实际训练需在终端运行
  `llmgui train --config train.yaml`（由 Python worker 执行）。
- 聊天历史不持久化，刷新页面即清空。
- GUI 仅覆盖当前 API：`/healthz`、`/v1/models`、`/v1/chat/completions`；
  暂无下载进度、adapter 管理、训练曲线（后续 P2b 阶段）。
- tok/s 由最终 chunk 的 `usage.completion_tokens` 与首 token 之后的耗时计算；
  若服务端未返回 usage，则按字符数 / 4 估算并标注 `≈`。

## 自检

```powershell
node --check gui/app.js
```

修改后建议再用浏览器刷新 <http://127.0.0.1:11435/> 冒烟：状态栏在线、模型页复制命令、
聊天流式输出、训练 YAML 联动、设置保存后仍生效。
