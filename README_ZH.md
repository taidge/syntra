# Contrix Office

[English](README.md) | [中文](README_ZH.md)

一个全功能的开源办公套件，完全使用 Rust 构建。Contrix Office 提供文档编辑、电子表格、演示文稿和白板协作功能——全部由高性能 Rust 技术栈驱动。

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
- 基于 CRDT 的冲突解决（操作变换）
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
| DOCX     | 计划中 | —    | —      | —          |
| XLSX     | —      | 计划中 | —    | —          |
| PPTX     | —      | —    | 计划中 | —          |
| PNG      | —      | —    | —      | 导出       |
| SVG      | —      | —    | —      | 导出       |

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
| 前端 | [Dioxus](https://dioxuslabs.com) | 全 Rust UI 框架（Web/桌面/移动端） |
| 数据库 | [SQLx](https://github.com/launchbadge/sqlx) + SQLite/PostgreSQL | 异步数据库，编译时安全检查 |
| 认证 | JWT + Argon2 | 无状态认证 + 安全哈希 |
| 实时协作 | WebSocket + CRDT | 低延迟协作 |
| 搜索 | SQLite | 与事务一致的名称/正文搜索 |
| 配置 | [Figment](https://github.com/SergioBenitez/Figment) | 分层配置（TOML + 环境变量） |
| 日志 | [tracing](https://github.com/tokio-rs/tracing) | 结构化异步日志 |

## 项目结构

```
office/
├── Cargo.toml                  # 工作空间根目录
├── crates/
│   ├── core/                   # 共享类型、模型、DTO、配置
│   ├── server/                 # Salvo 后端（REST API + WebSocket）
│   ├── frontend/               # Dioxus Web 前端
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

- [Rust](https://rustup.rs/) 1.83+（2024 版本）
- SQLite 3（通过 sqlx 内置）或 PostgreSQL 15+

### 快速启动

```bash
# 克隆仓库
git clone https://github.com/contrix-org/office.git
cd office

# 运行后端服务器（默认使用 SQLite）
cargo run -p contrix-server

# 在另一个终端，运行前端开发服务器
cd crates/frontend
dx serve --platform web
```

后端将在 `http://localhost:3000` 启动，前端在 `http://localhost:8080` 启动。

### 配置

配置从分层源加载：

1. `config/default.toml` — 基础默认值
2. `config/production.toml` — 生产环境覆盖
3. 环境变量 — 最高优先级（以 `CONTRIX_` 为前缀）

主要配置项：

```toml
[server]
host = "0.0.0.0"
port = 3000

[database]
url = "sqlite://data/contrix.db?mode=rwc"

[auth]
jwt_secret = "请在生产环境中更改此值"
jwt_expiry_hours = 24

[storage]
base_path = "data/files"
max_upload_size_mb = 100
default_quota_mb = 5120        # 每用户 5 GB
```

### Docker 部署

```bash
# 使用 Docker Compose 构建并运行
cd docker
docker-compose up -d

# 或直接构建镜像
docker build -t contrix-office -f docker/Dockerfile .
docker run -p 3000:3000 -v contrix-data:/app/data contrix-office
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
cargo build --release -p contrix-server
```

### 测试

```bash
# 运行所有测试
cargo test --workspace

# 运行特定 crate 的测试
cargo test -p contrix-doc-engine
cargo test -p contrix-collab
cargo test -p contrix-export
```

### 代码检查与格式化

```bash
# 格式化代码
cargo fmt --all

# 运行 clippy 检查
cargo clippy --workspace --all-targets -- -D warnings
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

### 前端（Dioxus）

前端是一个单页应用，包含：

1. **Pages（页面）** — 路由级组件（首页、登录、仪表盘、Writer、Calc、Slides 等）
2. **Components（组件）** — 可复用 UI 组件（导航栏、侧边栏、工具栏、文件列表、模态框）
3. **State（状态）** — 应用状态管理和 API 客户端

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
- [Dioxus](https://dioxuslabs.com) — Rust 跨平台 UI 框架
- [SQLx](https://github.com/launchbadge/sqlx) — Rust 异步 SQL 工具包
- 灵感来源于 [Collabora Online](https://www.collaboraoffice.com/) 和 [ONLYOFFICE](https://www.onlyoffice.com/)
