# 调研：国内模型下载源、分发与缓存

- 日期：2026-11-02
- 关联需求：C7（国内源下载 + fallback + 分片 + 续传 + SHA256 + 测速 + 离线）、C8（同源同文件 ≥3× 单线程吞吐）、C10（仅 safetensors/GGUF，强制校验）
- 结论适用栈：Rust 下载器 + Python 侧复用官方 SDK
- 验证方式：2026-11-02 实测探活（下文标注「实测」）与官方文档核对；未实测项明确标「待验证」

## 1. 结论摘要

1. **默认源取 ModelScope，fallback 到 hf-mirror，第三顺位 Gitee AI（模力方舟）**。ModelScope 是国内发布者首发地且列表 API 直接返回**每文件 `Sha256` 与 `Size`**，最适合做完整性校验；hf-mirror 覆盖 HF 原生生态（Qwen 官方 GGUF、bartowski 社区量化）；Gitee AI 已提供 **HF 兼容 API**，实测与 HF 镜像同 commit。
2. **不要自研 LFS/Xet 协议**。ModelScope 复用 `modelscope-hub`（官方轻量客户端，自带 Range 续传、并行分片、SHA256、文件锁、离线模式）；HF 若走 Xet 直接回调 Python；git-lfs 型源（WiseModel/OpenI）调 Python/git 子进程。Rust 负责调度、测速、分片、CAS 缓存与校验。
3. **跨源只做「文件级」分工，不做「分片混源」**：不同源同一文件的字节一致性只能靠 SHA256 逐文件证明；实测 Qwen2.5-7B-Instruct-GGUF 在 hf-mirror 与 Gitee AI 的仓库 commit sha 相同（`bb5d59e0…`），但文件级哈希仍须由列表 API 核对后才允许复用。
4. **ModelScope 列表 API 可能「静默截断」**（modelscope-hub 更新日志明确会重复枚举以对抗该行为），Rust 侧必须做条目数/末页自检，否则会漏文件却报成功。
5. **默认模型清单只放 Apache-2.0 / MIT**：GLM-4-9B-0414 已 MIT、Qwen2.5/Qwen3 为 Apache-2.0、Yi-1.5 为 Apache-2.0；InternLM2.5 实测为 `license:other`、Baichuan2 为自定义许可，不做默认项。
6. **OpenI 启智与 WiseModel 不做默认链路**：OpenI 为 Gitea 平台且匿名访问要先过协议/人机校验（实测），程序化拉取不可靠；WiseModel 模型页需登录，仅元数据 API 匿名可读。
7. **限速无公开 SLA**：各免费/公益源均未公布硬性限速数字，工程结论是「可配置并发 + 429/低速退避 + 源级冷却」，实测数据落到 benchmark 而不是写死承诺。

## 2. 源对比表（优先级从高到低）

| 优先级 | 源 | 入口 / 接口形态 | 鉴权 | 覆盖 | 备注（实测） |
|---|---|---|---|---|---|
| P0 | ModelScope 魔搭 | REST：`/api/v1/models/{owner}/{repo}/repo/files?Revision=&Recursive=` + 文件下载；另有 SDK `snapshot_download` | 公开仓库匿名；私有/受限用 token（`read` 权限即可下载） | 国内发布者首发最全（Qwen/Qwen3/GLM/DeepSeek 官方镜像、Qwen 官方 GGUF） | 列表实测返回 `Path/Size/Sha256/Type`；下载端点实测可返回文件字节流 |
| P1 | hf-mirror.com | 完整 HF 兼容（`HF_ENDPOINT=https://hf-mirror.com`），含 `/api/models/...` 元数据与 `hfd`（aria2） | 公开匿名；Gated 需 HF token（**token 会发给第三方**） | HF 全量镜像，社区 GGUF 最全 | 站点自述公益项目、**无可用性 SLA**；`/api/models/Qwen/Qwen2.5-7B-Instruct-GGUF` 实测 200 |
| P2 | Gitee AI / 模力方舟 | HF 兼容：`HF_ENDPOINT=https://hf-api.gitee.com`；另有 git clone + Git LFS | 公开匿名；私有用 Gitee 私人令牌（`HF_TOKEN`） | 精选镜像（Qwen/Qwen2.5-7B-Instruct 等），含 GGUF | `/api/models/Qwen/Qwen2.5-7B-Instruct-GGUF` 实测 200 且 sha 与 HF 一致；注意品牌从 Gitee AI → 模力方舟（moark.com）迁移中 |
| P3 | WiseModel 始智 AI | GitLab：`gitlabcdn.wisemodel.cn` 上的 git 仓库 + LFS；PyPI 有 `wisemodel-hub` | 需注册账号；模型页未登录会跳登录（实测） | 中文社区模型（ChatGLM3、llama 系镜像） | `/api/v4/projects?search=Qwen` 实测匿名可读；权重拉取建议 git-lfs / 官方客户端 |
| P4 | OpenI 启智 | Gitea 仓库（云脑/算力平台一体），模型多为 repo + LFS | 需账号；匿名访问先弹协议与人机校验（实测） | 国产模型与科研项目多，但分发形态非模型中心 | 不作为默认源；仅在用户已有账号/云脑环境时作为显式源 |
| 备选 | 魔乐社区 modelers.cn | 自有 SDK（openmind），非 HF 协议 | 需账号 | 国产模型镜像 | 未深入验证，仅记录为可选扩展 |

## 3. 客户端与关键接口

### 3.1 ModelScope（默认源）

- **列文件（实测）**：`GET https://modelscope.cn/api/v1/models/{owner}/{repo}/repo/files?Revision=master&Recursive=true`
  返回 `{"Data":{"Files":[{"Path","Size","Type":"blob|tree","Sha256","IsLFS"}]}}`。`Type` 是 `blob`（文件）/`tree`（目录），**不是** `file`。
- **下载单文件（实测可用）**：`GET https://modelscope.cn/api/v1/models/{repo_id}/repo?Revision={rev}&FilePath={path}`（数据集把 `models` 换 `datasets`）。社区另有 `/{owner}/{repo}/resolve/{rev}/{path}` 形态（Rust 实现 `llmman`、`modelhub` 均使用），上线前应以实测校验。
- **SDK / CLI**：`pip install -U modelscope`；`modelscope download --model Qwen/Qwen2.5-7B-Instruct-GGUF --local_dir ./m`（旧 SDK 用下划线参数）；新版拆分为 `modelscope-hub`，CLI 为 `ms-hub download ... --local-dir`（连字符），**两套拼写不同，按锁定的版本调用**。
- **关键环境变量（官方文档）**：`MODELSCOPE_API_TOKEN`、`MODELSCOPE_ENDPOINT`（默认 `https://modelscope.cn`）、`MODELSCOPE_CACHE`（默认 `~/.cache/modelscope`）、下载调优 `MODELSCOPE_DOWNLOAD_PARALLEL_WORKERS`（默认 1）、`MODELSCOPE_DOWNLOAD_PARALLEL_THRESHOLD_MB`（默认 500）、`MODELSCOPE_DOWNLOAD_PART_SIZE_MB`（默认 160）、`MODELSCOPE_DOWNLOAD_MIN_BYTES_PER_SECOND`（默认 1024）等。
- **能力**：HTTP Range 续传、大文件并行 Range 下载、逐文件重试退避、SHA256 校验、多进程文件锁、离线模式、进度回调；token 有 `read/write/admin` 三级。

### 3.2 Hugging Face 兼容入口（hf-mirror / Gitee AI）

- `HF_ENDPOINT` 切换镜像入口，`HF_HOME` 指定缓存；`HF_TOKEN` 传凭证；`HF_HUB_DISABLE_IMPLICIT_TOKEN=1` 避免把已存 token 隐式发给第三方镜像。
- CLI 已换代：`huggingface-cli` 在 `huggingface_hub` 1.0 移除，现为 `hf download` / `hf auth login`；支持 `--dry-run`（只拉元数据，可用来做源探测）与 `--revision <commit>`。
- 加速方式：`hf_xet`（官方高性能传输；官方 Rust crate `hf-xet` 已存在，可作长期研究点，P0 不引入）；第三方 `hfd` 脚本基于 aria2，`-x`（单服务器连接数，标准 aria2 上限 16）、`-j`（并行文件数）。
- Gated 模型：必须先在 HF 官网申请许可再在镜像用 `--token`；把 token 交给第三方镜像等于把凭据交给该服务，**私有/受限仓库应走官方源**。
- Gitee AI 额外路径：`git clone https://gitee.com/<repo>.git` + `git lfs pull -I <file>`，适合少量文件按需拉取。

### 3.3 WiseModel / OpenI 的定位

- WiseModel：GitLab 系（`/api/v4/projects` 匿名可读元数据），权重用 Git LFS；模型页与下载需登录，故仅作为「用户已登录环境下的显式源」，不进自动 fallback。
- OpenI：Gitea 系，匿名访问先弹用户协议+人机校验（实测），且模型/数据集与云脑算力绑定；不建议在 Rust 里直连，最多以「用户提供一个已克隆目录」的方式接入。

## 4. 下载器设计要点（Rust 为主，Python 兜底）

### 4.1 默认源 + 自动 fallback + 测速

- **源选择顺序**（满足 C7「默认 ModelScope，fallback hf-mirror」）：
  1. 先探测 ModelScope 是否存在同名仓库/目标文件（列表 API）；存在 → 用它；
  2. 不存在或不可达 → hf-mirror；
  3. 再失败 → Gitee AI（HF 兼容同协议，切换成本最低）；
  4. 仅 GGUF 社区量化且 ModelScope 无同名仓库时，允许把 hf-mirror 提到首位（配置项 `source_order`）。
- **fallback 触发判据**：`404/403`、连续 `5xx`、连接/首包超时、60s 窗口内速率 < 50KB/s、`429`（尊重 `Retry-After`）、SHA256 不符、列表条目数异常。同一源连续失败 2 次进入 10 分钟冷却；每次切换记录 provenance。
- **测速**：对每个候选源做 `Range: bytes=0-1048575` 的 1MiB 探测（3 次取 p50）并叠加列表 API 的 RTT；结果按 host 缓存 30 分钟（可手动刷新）；取前 2 名作为候选，不做「一次性逐源全测」（避免冷启动变慢）。
- **跨源一致性**：跨源只在文件粒度分派；同一文件的分片必须落在同一 URL 上，除非逐文件 SHA256 一致（CAS 命中后自然去重）。

### 4.2 分片并发与断点续传

- 单文件 8 片（4–16 可配），片大小 8–32MB 自适应；每 host 连接上限默认 8，文件级并发默认 2–4；`206` + `Content-Range` 校验，服务端不支 Range 时自动降级单流。
- 续传状态：`<file>.part` + `<key>.state.json`（ETag/Last-Modified/size/分片位图）；ETag 变化则整文件重下；全部完成 `fsync` 后原子 `rename`。
- 校验：流式增量 SHA256（`sha2`），边下边算，避免二次读盘；失败删除 `.part` 并整文件重试 1 次，二次失败标记该源不可用。

### 4.3 内容寻址缓存（CAS）与磁盘

```
<cache>/blobs/sha256/<ab>/<hex>                     # 内容寻址块
<cache>/refs/<source>/<owner>--<name>@<rev>.json    # manifest：文件表/size/sha256/来源/时间
<cache>/snapshots/<owner>--<name>/<rev>/...         # 硬链接或复制（Windows NTFS 硬链接可用）
<cache>/state/<key>.state.json                      # 续传状态（不含权重）
```

- 下载前用 manifest 汇总 size 做**空间预检**（×1.1 余量）；默认缓存目录放用户目录（`%LOCALAPPDATA%\llmgui\models`），**不得默认写 F:**（本机 F: 仅约 24GB），支持 `--cache-dir`。
- 过滤规则：只接受 `.safetensors` / `.gguf` / 小配置文件，拒绝 `.bin`/pickle（C10）。

### 4.4 离线模式、代理与鉴权

- 离线：`--offline` 仅读 CAS + 校验 manifest；语义对齐 `HF_HUB_OFFLINE=1` 与 `local_files_only=True`。离线校验失败直接报错，不联网。
- 代理：`reqwest` 默认读取 `HTTP_PROXY/HTTPS_PROXY/ALL_PROXY/NO_PROXY`；pip/子进程同样继承，需在文档写清。
- 鉴权：每源独立 token（ModelScope token、HF token、Gitee PAT、WiseModel token），只对 `https` 发送、日志脱敏、存 Windows DPAPI/Credential Manager；默认关闭 HF 隐式 token；私有/受限仓库禁止自动切到第三方镜像。

### 4.5 Rust 与 Python 的分工

| 能力 | 建议归属 | 理由 |
|---|---|---|
| 分片/并发/续传/退避/测速/源切换 | **Rust** | 与运行时同栈，`tokio`+`reqwest` 控制力强，性能与 C8 直接相关 |
| SHA256 流式校验、CAS、原子落盘、文件锁 | **Rust** | 需与缓存布局强绑定；`sha2`、`fd-lock`/`fs2` 可用 |
| ModelScope 列表/下载的协议细节 | **Rust 直连 + Python 兜底** | 列表 API 简单可直连；OpenAPI 变更多时回退 `modelscope-hub` 子进程 |
| HF Xet、Gated 授权流程、git-lfs 仓库 | **Python 复用** | Xet 协议复杂且有官方 Rust crate 但未成熟到可替换；不要重写 |
| 训练侧取模型 / 版本解析 | **Python** | 与 `transformers`/PEFT 生态一致 |

- 现成 Rust 参考实现（可借鉴接口细节，注意别复制过期逻辑）：`modelhub` crate 的 `modelscope/download.rs`、`llmman` 的 `sources/modelscope.rs`。
- 验收对齐：C8「≥3× 单线程吞吐」用同源同文件（≥1GB、Q4_K_M 双分片）做基准；C7 用本地 HTTP fixture（支持 Range/断连/429）跑集成测试，避免依赖公网。

## 5. 默认模型清单（v0，优先国内可下载 + 许可友好）

| 场景 | 仓库（源内 ID） | 格式/量化 | 体积 | 许可 | 国内源可用性 |
|---|---|---|---|---|---|
| 冒烟/CI | `Qwen/Qwen2.5-0.5B-Instruct` | safetensors | ~1GB（约） | Apache-2.0 | ModelScope / hf-mirror / Gitee AI 均有 |
| **默认 7B 通用** | `Qwen/Qwen2.5-7B-Instruct-GGUF` | **Q4_K_M**（2 分片） | **4.68GB**（实测 `3993201344`+`689872288`） | Apache-2.0 | 三源实测存在，Gitee 与 HF 同 sha |
| 7B 高质量 | 同上 | Q8_0（3 分片） | 8.10GB（实测） | Apache-2.0 | 同上 |
| 7B 极省内存 | 同上 | Q2_K / Q3_K_M | 3.02GB / 3.81GB（实测） | Apache-2.0 | 同上 |
| 14B 通用 | `Qwen/Qwen2.5-14B-Instruct-GGUF` | Q4_K_M | 约 9GB（待核实） | Apache-2.0 | ModelScope 有 bartowski 镜像仓库 |
| 代码 | `Qwen/Qwen2.5-Coder-7B-Instruct-GGUF` | Q4_K_M | 约 4.7GB（待核实） | Apache-2.0 | 需探测（预期三源） |
| 推理/思维链 | `deepseek-ai/DeepSeek-R1-Distill-Qwen-7B` | safetensors（自量化）/ 社区 GGUF | fp16 约 15GB / Q4 约 4.7GB | MIT | ModelScope、hf-mirror |
| 通用 MIT 备选 | `zai-org/GLM-4-9B-0414` | safetensors（BF16 ~18.8GB 存储） | 约 18GB | MIT（实测 tag `license:mit`） | ModelScope / hf-mirror |
| 微调底座 | `Qwen/Qwen2.5-7B-Instruct`（safetensors，非 GGUF） | BF16 | ~15.2GB | Apache-2.0 | 三源 |
| 候选（需复核许可） | `Qwen/Qwen3-8B` | safetensors / GGUF | ~16GB / Q4 约 5GB | 预期 Apache-2.0 | 上线前读 repo LICENSE |
| 不列入默认 | `internlm/internlm2_5-7b-chat` | safetensors | ~15.5GB | **license:other**（实测） | 商用需阅读自定义协议 |
| 不列入默认 | `baichuan-inc/Baichuan2-7B-Chat` | safetensors | ~15GB | 自定义许可（需登记） | 限制较多 |

说明：体积以列表 API 的 `Size` 为准，表中「实测」项来自 2026-11-02 的 ModelScope 文件列表；标「约/待核实」的为估算或未逐文件核对，实现时应以 manifest 汇总值覆盖。

## 6. 风险与未验证项

- **公益镜像无 SLA**：hf-mirror 自述公益项目，可能限流或停服；必须保留 ModelScope 主源与 Gitee AI 第三源。
- **端点/协议漂移**：Gitee AI → 模力方舟（域名/命名空间迁移）、ModelScope SDK 拆分为 `modelscope-hub`、CLI 参数从 `--local_dir` 变 `--local-dir`、`huggingface-cli` → `hf`。所有源适配层要版本锁定 + 集成测试。
- **列表截断**：ModelScope `repo/files` 对超大仓库可能静默截断（官方客户端会重复枚举修正）；Rust 侧必须自检条目数并支持分页/重枚举。
- **未验证项**：① ModelScope `/{owner}/{repo}/resolve/{rev}/{path}` 与 `/api/v1/.../repo?FilePath=` 的长期稳定性；② HF tree API 的逐文件 `lfs.oid`（即 sha256）取法在 hf-mirror 上的可用性（Gitee AI 未提供逐文件 sha，只能靠 commit sha + 本地计算兜底）；③ WiseModel 匿名 git clone 是否可行（实测页面需登录）；④ 各源真实限速阈值——只做运行时测量，不写死。
- **许可与合规**：非 OSI 许可（InternLM2.5、DeepSeek-V3/R1 权重协议、Baichuan2）需人工审核；软件只做许可展示与来源记录，不改模型许可（需求 Q5）。国内分发还需关注生成式 AI 服务备案要求。
- **磁盘**：本机 F: 仅约 24GB，默认缓存必须落用户目录并做空间预检，否则 7B fp16 + 训练产物会立刻打满。

## 7. 参考链接

- hf-mirror 首页与用法：https://hf-mirror.com/ ；`/api/models` 实测：https://hf-mirror.com/api/models/Qwen/Qwen2.5-7B-Instruct-GGUF
- HF 下载与镜像加速实战（2026-09 核对，含 `hf` CLI、hfd/aria2、Xet、SHA256SUMS）：https://docs.alayanew.com/cn/docs/tech-blog/2026-05-05-hf-mirror-acceleration
- `hfd` 脚本（aria2 多连接）：https://gist.github.com/padeoe/697678ab8e528b85a2a7bddafea1fa4f
- ModelScope 文件列表 API（实测）：https://modelscope.cn/api/v1/models/Qwen/Qwen2.5-7B-Instruct-GGUF/repo/files?Revision=master&Recursive=true
- ModelScope CLI 文档：https://github.com/modelscope/modelscope/blob/master/docs/source/command.md
- ModelScope SDK `snapshot_download`：https://github.com/modelscope/modelscope/blob/master/modelscope/hub/snapshot_download.py
- ModelScope 哈希校验实现（`file_integrity_validation`）：https://github.com/modelscope/modelscope/blob/master/modelscope/hub/utils/utils.py
- modelscope-hub（官方轻量客户端，含下载/缓存/环境变量全表）：https://github.com/modelscope/modelscope_hub
- ModelScope token 申请：https://modelscope.cn/my/myaccesstoken
- 模力方舟模型下载（`HF_ENDPOINT=https://hf-api.gitee.com`）：https://moark.com/docs/products/models/download
- Gitee AI 兼容 API 实测：https://hf-api.gitee.com/api/models/Qwen/Qwen2.5-7B-Instruct-GGUF
- Gitee AI 模型托管与 git-lfs 下载：https://help.gitee.com/enterprise/code-manage/code-hosting/large-file-manage/ai-model-hosting
- WiseModel GitLab API（实测匿名可读）：https://www.wisemodel.cn/api/v4/projects?search=Qwen
- wisemodel-hub：https://pypi.org/project/wisemodel-hub/
- OpenI 启智模型库（需登录/协议确认）：https://openi.pcl.ac.cn/modelbase
- Rust 参考实现（ModelScope 下载器）：https://docs.rs/modelhub/0.1.4/src/modelhub/modelscope/download.rs.html 、https://docs.rs/llmman/latest/src/llmman/sources/modelscope.rs.html
- Hugging Face 下载文档：https://huggingface.co/docs/huggingface_hub/en/guides/download
- GLM-4-9B-0414 许可（MIT）：https://hf-mirror.com/zai-org/GLM-4-9B-0414
- InternLM2.5 许可（`license:other`）：https://hf-mirror.com/internlm/internlm2_5-7b-chat
- Yi-1.5-9B-Chat 许可（Apache-2.0）：https://hf-mirror.com/01-ai/Yi-1.5-9B-Chat
