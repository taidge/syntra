# 数据结构激进整理任务

## API 字段命名

- [x] 将文档类型字段从 `doc_type` 统一改为 `document_type`，覆盖核心 DTO、服务端 handler、前端请求/响应和测试。
- [x] 将分享相关字段从 `shared_by` / `shared_with` / `shared_with_group` 改为 `shared_by_user_id` / `shared_with_user_id` / `shared_with_group_id`，删除旧字段入口。
- [x] 将配额更新请求从 `storage_quota_mb` 改为 `storage_quota`，API 统一使用字节数，不保留 MB 入参。
- [x] 将备份响应里的 `file_path` 改为 `file_name`，避免把文件名伪装成路径字段。

## 字段暴露与历史兼容

- [x] 删除 `FileResponse.path` 公开字段，避免把内部存储路径混入文件/文件夹元数据响应。
- [x] 删除 `/documents/:id` 同时接受 `file_id` 的历史兼容逻辑，文档接口只接受 `document_id`。

## 验证

- [x] 更新 Rust 集成测试和 Playwright API 测试里的字段名。
- [x] 运行格式化和核心检查，确认整理后的结构可以编译。

# 全项目审查修复（第二轮）

## 服务端权限与数据

- [x] 新增共享访问控制模块 `handlers/access.rs`，删除 files/folders/documents 三处重复的 `check_room_access`/`load_accessible_folder` 实现。
- [x] `batch_tag`：校验标签归属与文件可编辑权限（原先任何登录用户可给任意文件打标签）。
- [x] `batch_move`：校验目标文件夹存在且可写，并像单文件 move 一样传播 `room_id`。
- [x] `batch_duplicate`：副本保留原文件的 `room_id`。
- [x] `tags.rs` 的 assign/unassign/list 三个 handler 补齐文件与标签归属校验。
- [x] WOPI PutRelativeFile 与模板建文档不再丢失 `room_id`；模板建文档校验并继承父文件夹房间。
- [x] 房间 `storage_quota` 从 i32 统一为 i64 字节（与用户配额一致；SQLite INTEGER 原生支持 64 位，无需迁移）。
- [x] AI assist 假实现（返回 "Stub:" 文本）改为诚实的 501 未配置错误。

## 前端

- [x] 删除整条无功能的装饰性 Toolbar 组件（仅 slides 页使用，按钮全部无事件）。
- [x] `export_document` 重写：原先 window.open GET 无认证打不开 POST 导出端点，改为带 token 的 fetch + Blob 下载。
- [x] `FileList` 的 `refresh_key`/`current_parent_id` 改为 ReadOnlySignal，修复上传后列表不刷新。
- [x] Sidebar 重写：删除假目录树/假共享/假回收站，存储条改为 /auth/me 真实数据。
- [x] DocumentEditor 实现真正的自动跳转（原先只显示 "Redirecting..." 不跳）。
- [x] 白板背景色真正实现（加载/应用/持久化/导出），并新增背景色选择器；删除 `let _ = bg` 等死代码。
- [x] Writer 页码估算实现（原先永远显示 1/1）。
- [x] 评论 @提及候选改为真实数据（文档评论参与者 + 当前用户），删除 alice/bob/charlie 假名单。
- [x] collab WebSocket onerror 不再静默吞错。
- [x] i18n 补漏：file_list、sidebar、document_editor、profile、login、register、version_history 的硬编码英文全部接入字典（EN/ZH 同步新增约 50 个键）。

## 导出

- [x] DOCX 导出补 `docProps/core.xml`（标题可往返）并注册 Content-Type 与关系。
- [x] styles.xml 补 Heading4-6 定义（原先引用未定义样式）。
- [x] 删除列表项里无 numId 的非法 `<w:numPr>`。

## 验证

- [x] `cargo check --workspace --all-targets` 通过。
- [x] `cargo test -p syntra-export` 40 个测试通过。
- [x] `cargo test -p syntra-server` 集成测试通过。
- [x] 前端 wasm32 目标编译通过；EN/ZH 字典 JSON 校验通过。
