# Syntra Office

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="static/icons/syntra-wordmark-dark.svg">
  <img src="static/icons/syntra-wordmark.svg" width="112" height="112" alt="Syntra Office">
</picture>

[English](README.md) | [中文](README_ZH.md)

一个全功能的开源办公套件，完全使用 Rust 构建。Syntra Office 提供文档编辑、电子表格、演示文稿和白板协作功能——全部由高性能 Rust 技术栈驱动。

## 功能特性

### 文档编辑器

- **Writer（文档编辑器）** — 富文本文档编辑器（类似 Word/Google Docs）
  - 粗体、斜体、下划线、删除线、上标/下标
  - 标题（H1–H6）、段落、引用块
  - 有序和无序列表，支持嵌套
  - 表格，支持合并单元格、行列跨度
  - 图片插入、超链接
  - 代码块，支持语法高亮
  - 页面布局（边距、方向、纸张大小）
  - 字数统计、查找与替换

- **Calc（电子表格）** — 电子表格编辑器（类似 Excel/Google Sheets）
  - 单元格编辑，支持类型：文本、数字、日期、布尔值
  - 公式支持（SUM、AVG、COUNT、IF、VLOOKUP 等）
  - 单元格格式化：字体、颜色、边框、对齐、数字格式
  - 每个工作簿支持多个工作表
  - A1 表示法单元格引用
  - 列/行调整大小、插入、删除
  - 冻结窗格、合并单元格
  - 条件格式化
  - 排序和筛选
  - 图表（柱状图、折线图、饼图、环形图、散点图、面积图）

- **Slides（演示文稿）** — 演示文稿编辑器（类似 PowerPoint/Google Slides）
  - 支持多种布局的幻灯片创建（标题、双栏、空白等）
  - 文本框、图片、形状（12+ 种类型）
  - 图表和数据可视化
  - 幻灯片过渡效果（淡入、滑动、缩放、翻转等）
  - 演讲者备注
  - 演示模式
  - 幻灯片母版/模板
  - 拖放排序

- **Whiteboard（白板）** — 自由绘图画布
  - 自由手绘，支持压感
  - 形状工具（矩形、圆形、直线、箭头）
  - 文字工具
  - 橡皮擦
  - 多图层，支持透明度控制
  - 导出为 PNG/SVG

### 协作功能

- 基于 WebSocket 的实时协作编辑
- 持久化版本快照与操作变换；冲突会保留本地修改并要求显式处理
- 实时光标显示——查看其他用户的编辑位置
- 用户感知（谁在查看/编辑）
- 评论和批注，支持 @提及
- 评论回复线程、解决/取消解决

### 文件管理

- 上传、下载、移动、重命名、删除文件
- 文件夹层级和嵌套导航
- 回收站，支持恢复
- 文件共享（用户间共享和公开链接）
- 权限级别：只读、读写、管理员
- 每用户存储配额

### 版本历史

- 自动保存并创建版本快照
- 浏览和比较版本
- 恢复任意历史版本
- 可选版本标签命名

### 导出与导入

| 格式     | Writer | Calc | Slides | Whiteboard |
|----------|--------|------|--------|------------|
| PDF      | 导出   | 导出 | 导出   | 导出       |
| HTML     | 导出   | —    | —      | —          |
| Markdown | 导出   | —    | —      | —          |
| CSV      | —      | 双向 | —      | —          |
| JSON     | 双向   | 双向 | 双向   | 双向       |
| DOCX     | 导入/导出 | — | — | — |
| XLSX     | — | 导入/导出 | — | — |
| PPTX     | — | — | 元素导入/导出（部分能力） | — |
| PNG      | —      | —    | —      | 导出       |
| SVG      | —      | —    | —      | 导出       |

### PDF 支持

- 上传 PDF 后可直接在文件列表打开，支持翻页、缩放、文字选择与查找。加密 PDF 可输入密码阅读，保持只读。
- 支持新增中文文字、高亮、旋转、调整页序、删除页面和撤销/重做。保存会创建新版本并保留原件，通过 ETag 检测并发修改；修改已有正文时，可转换成独立的私有 Writer 文档。
- 带文字层的 PDF 使用原生文字提取；扫描件和混合文档由浏览器中打包的中文/英文 OCR 识别，显示进度并可取消。识别结果需要校对，尤其是公式和表格；转换不保证还原 PDF 固定版式或内嵌插图，原件保持不变。
- Writer、Calc、Slides、Whiteboard 均可导出真正的 PDF，包含中文字体、图片和分页。服务端需安装 Chrome、Chromium 或 Edge；自动查找不足时可用 `SYNTRA_PDF_BROWSER` 指向可执行文件，浏览器沙箱保持启用。Docker 镜像包含 Chromium 并使用非 root 用户运行。
- 构建浏览器资源需 Node 22.13 以上。`scripts/build-frontend.ps1` 会打包 PDF.js、PDF-LIB、OCR worker/语言数据和 OFL 中文字体，运行时预览与识别不调用外部文档服务或 CDN。单个源文件上限 32 MiB、500 页，一次 OCR 最多识别 50 个扫描页。

已鉴权 API：`GET/PUT /api/v1/files/{id}/pdf`（保存须提供 `If-Match`）、`POST .../pdf/convert`（`expected_etag`，可选 `ocr_pages`）和 `POST .../pdf/restore`（`expected_etag`、`version_number`）。`GET .../pdf?version=0` 下载原件。现有 `/documents/{id}/export/pdf` 和 `/convert` 任务支持 PDF 输出；文字层 PDF 还可通过转换任务生成 DOCX、HTML、Markdown、纯文本或原生 JSON。

### 搜索

- 搜索文档名称和已保存正文
- 按文档类型筛选
- 基于正文上下文生成结果摘要
- 分页结果

### 通知

- 应用内通知（共享、评论、提及、系统）
- WebSocket 推送通知
- 标记已读/全部已读

### 管理面板

- 用户管理（创建、编辑角色、删除）
- 全系统存储使用面板
- 操作审计日志
- 系统设置配置

## 技术栈

| 层级 | 技术 | 用途 |
|------|------|------|
| 后端 | [Salvo](https://salvo.rs) | 高性能 Rust Web 框架 |
| 前端 | [Glory](https://glory.rs) + [Sapid](https://github.com/sapid-ui/sapid) | Rust 语义控件与 WebAssembly 浏览器前端 |
| 数据库 | Diesel + Horme + SQLite | 短时异步查询租约与事务 |
| 认证 | JWT + Argon2 | 无状态认证 + 安全哈希 |
| 实时协作 | WebSocket + 持久化版本与条件提交 | 在线同步与显式冲突恢复 |
| 搜索 | SQLite | 与事务一致的名称/正文搜索 |
| 配置 | [Figment](https://github.com/SergioBenitez/Figment) | 分层配置（TOML + 环境变量） |
| 日志 | [tracing](https://github.com/tokio-rs/tracing) | 结构化异步日志 |

## 项目结构

```
syntra/
├── Cargo.toml                  # 工作空间根目录
├── crates/
│   ├── core/                   # 共享类型、模型、DTO、配置
│   ├── server/                 # Salvo 后端（REST API + WebSocket）
│   ├── frontend/               # Glory + Sapid Web 前端
│   ├── doc-engine/             # 文档模型（Writer、Calc、Slides、Whiteboard）
│   ├── collab/                 # 实时协作引擎（CRDT/OT）
│   └── export/                 # 导出/导入（HTML、Markdown、CSV 等）
├── migrations/                 # SQL 数据库迁移
├── config/                     # 配置文件（default.toml、production.toml）
├── static/                     # 静态资源
├── docker/                     # Dockerfile + docker-compose.yml
└── .github/workflows/          # CI/CD 流水线
```

## 快速开始

### 前置要求

- [Rust](https://rustup.rs/) 1.94+（2024 版本）
- SQLite 3（通过 Diesel 内置）
- 本地 Glory：`D:/Works/glory-rs/glory`，Sapid：`D:/Works/glory-rs/sapid`
- `wasm32-unknown-unknown` target 和与 `Cargo.lock` 版本一致的 wasm-bindgen-cli

### 快速启动

```bash
# 克隆仓库
git clone --recurse-submodules https://github.com/taidge/syntra.git
cd syntra

# 构建 Glory + Sapid 前端并启动后端
rustup target add wasm32-unknown-unknown
pwsh ./scripts/dev.ps1
```

打开 `http://localhost:3000`，前端和 API 使用同一源。
构建遵循调用者环境和 Cargo 正常配置，不改写 CARGO_HOME、target 或 TEMP/TMP。
脚本在当前 workspace 运行 cargo metadata，使用返回的 target_directory 定位产物，前端资源默认在其中的 syntra-web 子目录。
单独构建前端可运行 pwsh ./scripts/build-frontend.ps1，然后运行 pwsh ./scripts/dev.ps1 -SkipFrontendBuild。
前端资源目录可用 -OutputDirectory 指定；这不改变编译缓存或临时目录。
运行脚本前需初始化固定版本子模块，并安装与 Cargo.lock 相同版本的 wasm-bindgen-cli。

Office 导入导出使用标准格式解析；原生自定义部件的无损往返与外部文件兼容性分别验证。
外部 PPTX 可读取真实页序和页面尺寸、继承的占位符坐标与母版/版式装饰、文本运行段样式和安全超链接，以及带坐标的形状、组合、嵌入图片及其裁剪/旋转、支持的图表缓存、备注、转场和基础动画时序。模型可表达的内容写入标准 Office 部件。嵌入视频的媒体与位置会保留供导出；编辑画布显示视频占位符，不完整播放 PowerPoint 动画时间线。复杂段落排版、自定义路径、镜像、SmartArt/OLE 和不支持的视觉效果可能被近似或省略；导入时会显示具体警告，并保留原件下载。

外部 XLSX 可读取单元格样式与主题颜色、日期格式、行列尺寸、冻结窗格、矩形命名范围、支持的数据验证/条件格式规则、图表缓存和简单透视表。图表可从同一工作表的矩形范围刷新绑定，其余绑定保留缓存系列并告警；当前 Calc 网格不显示图表覆盖层。透视表支持一个行字段、一个值字段及求和/计数/平均值，并写入标准缓存和表定义部件；复杂或带筛选的透视表保留工作表中的缓存值并告警。标准透视表导出要求源数据前有表头行、目标数据前有标题行。自定义验证公式会保留供 Office 导出，但编辑器不执行这些验证公式。遇到不支持的结构化表/筛选/隐藏状态/打印/单元格富文本功能时会提示警告。

外部媒体不会自动加载。嵌入媒体在 base64 编码前限制为每项 2 MiB、每份导入演示文稿合计 8 MiB；文档 JSON 上限为 20 MiB。ZIP/XML 展开、图表点展开和数值几何也有明确预算。超出当前支持范围的内容仍保留在上传原件中。
保存与版本恢复必须携带 expected_version；冲突返回 409。分块上传必须携带从 1 开始的 chunk_index。
这套协作机制不提供离线 CRDT 合并。

使用 pwsh ./scripts/package.ps1 生成包含服务器、配置示例、迁移与浏览器资源的完整 ZIP 分发包，路径位于 metadata 返回的 target_directory/dist。
使用 pwsh ./scripts/test-package.ps1 -BrowserTests 从无源码目录验证 ZIP，包括登录、保存及进程重启后的恢复。
安装 e2e 依赖与 Playwright Chromium 后，使用 pwsh ./scripts/test-e2e.ps1 运行完整 HTTP 和浏览器套件。每次运行在 workspace 的 target 目录下使用独立数据库、存储与回环端口。


### 存储维护

`config/default.toml` 的 `[maintenance]` 控制历史版本保留与对象对账。普通历史默认分批收敛到最近 100 个且不超过 30 天的版本；上传原件、当前快照和有界的活跃协作基础版本受到保护。物理删除异步执行。本数据库的待删除队列达到 1024 个对象时，新的对象写入会被拒绝，直到清理取得进展；该限制统计对象数量，并非磁盘字节数。

新写入会在对象 I/O 前登记所有权，默认保留未知旧对象。只有一个元数据数据库独占整个 S3 桶或本地存储根目录时，才可启用 `reconcile_legacy_dedicated_namespace`。独立数据库必须使用不同的物理键；不支持复制数据库后分叉运行并继续共享原有对象键。上传读取遵循配置中的空闲及总时限；分块会话预留配额，最多 4096 块、每块最多 8 MiB，创建会话时返回相应限制。

### 数据库快照与对象备份

管理员备份接口只生成 SQLite 数据库快照，不复制本地文件或 S3 对象，也不为旧快照保留 blob。单独恢复 SQLite 快照不能保证文件恢复：垃圾回收可能已经删除当前数据库不再引用、但旧快照仍引用的对象。

应配套备份和恢复数据库与对象，保留数据库快照引用的对象键。执行冷备前先停止全部写入实例和垃圾回收，成套复制 SQLite、仍存在的 WAL 和对应对象。恢复到新位置并验证下载，再切换流量。

准备本机发布 ZIP 后，可用 `python scripts/test-disaster-recovery.py` 演练隔离本地存储的冷备恢复。演练在全新实例中核对当前内容、版本历史和上传原件，并用 SHA-256 清单检测对象缺失或篡改。

### 配置

配置从分层源加载：

1. `config/default.toml` — 基础默认值
2. `config/production.toml` — 生产环境覆盖
3. 环境变量 — 最高优先级（以 `SYNTRA_` 为前缀）

升级时，`SYNTRA_` 变量优先于旧版环境变量。若只有旧版默认的
SQLite 文件，Syntra Office 会继续在原位置打开它。替换容器时请挂载原有数据卷。
Prometheus 查询请使用新的 `syntra_` 指标前缀。

限流按 API、`/assets/`、页面与外壳三类分别计算每 IP 的独立配额，
每类均使用 `[rate_limit].max_requests` 和固定的 `window_seconds` 窗口，
生产默认各为 100 次 / 60 秒。已认证 API 请求还受每用户配额限制。
IP 配额耗尽时，请求在查询 IP 规则前返回 429；IP 配额内请求仍执行 IP 规则。
转发 IP 请求头默认不受信任，只有部署可信代理时才应启用 `trust_forwarded_headers`。

主要配置项：

```toml
[server]
host = "0.0.0.0"
port = 3000

[database]
url = "sqlite://data/syntra.db?mode=rwc"

[auth]
jwt_secret = "请在生产环境中更改此值"
jwt_expiry_hours = 24

[storage]
base_path = "data/files"
max_upload_size_mb = 100
default_quota_mb = 5120        # 每用户 5 GB
```

### Docker 部署

构建前必须确认 Docker builder 的实际数据存储在 D 盘或远端；Cargo 产物在其他盘不代表 Docker 缓存位置。

```bash
# 使用 Docker Compose 构建并运行
cd docker
export JWT_SECRET="$(openssl rand -hex 32)"
docker compose up -d

# 或在仓库根目录直接构建镜像
cd ..
docker build -t syntra -f docker/Dockerfile .
docker run -p 3000:8080 -e JWT_SECRET -v syntra-data:/data syntra
```

## API 参考

### 认证接口

| 方法 | 端点 | 描述 |
|------|------|------|
| POST | `/api/v1/auth/register` | 注册新用户 |
| POST | `/api/v1/auth/login` | 登录并获取 JWT |
| GET | `/api/v1/auth/me` | 获取当前用户信息 |

### 文件接口

| 方法 | 端点 | 描述 |
|------|------|------|
| GET | `/api/v1/files` | 列出文件（分页） |
| POST | `/api/v1/files/upload` | 上传文件（multipart） |
| GET | `/api/v1/files/:id` | 获取文件元数据 |
| PUT | `/api/v1/files/:id` | 重命名文件 |
| DELETE | `/api/v1/files/:id` | 将文件移至回收站 |
| POST | `/api/v1/files/:id/move` | 移动文件到其他文件夹 |
| GET | `/api/v1/files/:id/download` | 下载文件内容 |

### 文件夹接口

| 方法 | 端点 | 描述 |
|------|------|------|
| POST | `/api/v1/folders` | 创建文件夹 |
| GET | `/api/v1/folders/:id` | 获取文件夹元数据 |
| GET | `/api/v1/folders/:id/children` | 列出文件夹内容 |
| PUT | `/api/v1/folders/:id` | 重命名文件夹 |
| DELETE | `/api/v1/folders/:id` | 删除文件夹 |

### 文档接口

| 方法 | 端点 | 描述 |
|------|------|------|
| POST | `/api/v1/documents` | 创建文档 |
| GET | `/api/v1/documents/:id` | 获取文档（含内容） |
| PUT | `/api/v1/documents/:id` | 更新文档内容 |
| DELETE | `/api/v1/documents/:id` | 删除文档 |
| GET | `/api/v1/documents/:id/content` | 仅获取文档内容 |
| POST | `/api/v1/documents/:id/export/:format` | 导出文档 |
| GET | `/api/v1/documents/:id/versions` | 列出版本历史 |
| POST | `/api/v1/documents/:id/restore/:version_id` | 恢复版本 |

### 共享接口

| 方法 | 端点 | 描述 |
|------|------|------|
| POST | `/api/v1/shares` | 创建共享 |
| GET | `/api/v1/shares/:id` | 获取共享详情 |
| PUT | `/api/v1/shares/:id` | 更新共享权限 |
| DELETE | `/api/v1/shares/:id` | 移除共享 |
| GET | `/api/v1/shares/file/:file_id` | 列出文件的所有共享 |

### 评论接口

| 方法 | 端点 | 描述 |
|------|------|------|
| GET | `/api/v1/documents/:id/comments` | 列出评论 |
| POST | `/api/v1/documents/:id/comments` | 添加评论 |
| PUT | `/api/v1/comments/:id` | 编辑评论 |
| DELETE | `/api/v1/comments/:id` | 删除评论 |
| POST | `/api/v1/comments/:id/resolve` | 解决评论 |

### 搜索接口

| 方法 | 端点 | 描述 |
|------|------|------|
| GET | `/api/v1/search?q=...` | 搜索文档 |

### 通知接口

| 方法 | 端点 | 描述 |
|------|------|------|
| GET | `/api/v1/notifications` | 列出通知 |
| PUT | `/api/v1/notifications/:id/read` | 标记已读 |
| PUT | `/api/v1/notifications/read-all` | 全部标记已读 |
| DELETE | `/api/v1/notifications/:id` | 删除通知 |

### 管理接口

| 方法 | 端点 | 描述 |
|------|------|------|
| GET | `/api/v1/admin/stats` | 系统统计 |
| GET | `/api/v1/admin/users` | 列出所有用户 |
| GET | `/api/v1/admin/activity` | 操作审计日志 |

### WebSocket

| 端点 | 描述 |
|------|------|
| `WS /api/v1/ws/collab/:document_id` | 实时文档协作 |

## 开发指南

### 构建

```bash
# 检查所有 crate 是否可编译
cargo check --workspace

# 构建所有 crate
cargo build --workspace

# 构建发布版本
cargo build --release -p syntra-server
```

### 测试

```bash
# 运行所有测试
cargo test --workspace

# 运行特定 crate 的测试
cargo test -p syntra-doc-engine
cargo test -p syntra-collab
cargo test -p syntra-export
```

### 代码检查与格式化

```bash
# 格式化代码
cargo fmt

# 运行 clippy 检查
cargo clippy --locked --workspace --all-targets --no-deps --features syntra-frontend/ssr -- -D warnings
```

### 数据库迁移

迁移在服务器启动时自动运行。迁移文件位于 `migrations/` 目录：

- `001_init.sql` — 初始表结构（users、files、documents、versions、shares、comments、notifications、activity_logs）

## 架构设计

### 后端（Salvo）

后端采用分层架构：

1. **Handlers（处理器）** — HTTP 请求处理（提取参数、调用服务、返回响应）
2. **Services（服务层）** — 业务逻辑（存储、搜索等）
3. **Middleware（中间件）** — 横切关注点（JWT 认证、日志、CORS）
4. **State（状态）** — 共享应用状态（数据库连接池、配置、协作管理器）

### 前端（Glory + Sapid）

前端使用手写 Glory Widget、Cage/Bond 响应式状态和 Sapid 控件，原生链接保留文档 URL 与浏览器历史。
每个页面管理自己的异步任务与监听器，包含：

1. **Pages（页面）** — 路由级组件（首页、登录、仪表盘、Writer、Calc、Slides 等）
2. **Components（组件）** — 可复用 UI 组件（导航栏、侧边栏、工具栏、文件列表、模态框）
3. **State（状态）** — 应用状态管理和 API 客户端

本分支跟随当前本地语义 API。Calc 用选择模式、范围输入和 Fill 操作替代宿主尚未暴露的部分鼠标手势。
Writer 富文本编辑支持粗体、斜体、代码和链接；其他原生格式会保留，表格、列表、图片、代码块通过独立控件编辑。
复杂幻灯片对象也可通过演示文稿 JSON 面板调整。

### 协作引擎

实时协作使用：

1. **WebSocket** — 低延迟双向通信
2. **操作变换（OT）** — 并发编辑的冲突解决
3. **会话管理器** — 管理活跃编辑会话和参与者感知
4. **广播通道** — 高效的消息分发给所有参与者

### 文档引擎

每种文档类型都有丰富的结构化模型：

- **WriterDocument** — 基于块（段落、标题、列表、表格、图片、代码块）
- **Workbook** — 基于工作表，稀疏单元格存储（HashMap）、公式、格式化
- **Presentation** — 基于幻灯片，包含元素（文本框、形状、图片、图表）
- **Whiteboard** — 基于图层，包含绘图元素（路径、形状、文字、图片）

## 参与贡献

1. Fork 本仓库
2. 创建功能分支（`git checkout -b feature/amazing-feature`）
3. 提交更改（`git commit -m '添加精彩功能'`）
4. 推送到分支（`git push origin feature/amazing-feature`）
5. 创建 Pull Request

请确保：
- 代码通过 `cargo fmt --check` 和 `cargo clippy`
- 新功能有对应测试
- 文档已更新

## 许可证

本项目采用双重许可：

- [MIT 许可证](LICENSE-MIT)
- [Apache 许可证 2.0](LICENSE-APACHE)

您可以自行选择任一许可证。

## 致谢

- [Salvo](https://salvo.rs) — 快速灵活的 Rust Web 框架
- [Glory](https://glory.rs) + [Sapid](https://github.com/sapid-ui/sapid) — Rust 跨平台 UI 框架
- [Diesel](https://diesel.rs/) — Rust 类型化 SQL；Horme 提供异步查询租约
- 灵感来源于 [Collabora Online](https://www.collaboraoffice.com/) 和 [ONLYOFFICE](https://www.onlyoffice.com/)
