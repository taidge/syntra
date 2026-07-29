# 改进审查报告

本轮审查范围：Rust workspace 后端入口、路由、中间件、文件/分片上传、存储服务、配置文件、前端编译警告，以及明显未接入实现。

## 已完成

- [x] `StorageService` 直接 `base_path.join(user_input)`，上传文件名和数据库中的相对路径可触发路径穿越。已新增文件名清洗、相对路径组件校验，并让读/删路径统一走校验。
- [x] 普通上传在校验父目录/房间权限前先落盘，失败时会留下孤儿文件；同时没有按 `storage.max_upload_size_mb` 拦截。已改为先校验大小和目的地权限，再写入存储，并在数据库写入失败时清理文件。
- [x] 分片上传创建会接受未授权的 `parent_id`/`room_id`，完成上传时可把文件挂到无权限目录或房间。已在创建和完成阶段校验父目录与房间编辑权限。
- [x] 分片上传未校验声明大小、已上传大小和配置上限，可能导致超额写盘。已在 session 创建、chunk 写入和 complete 合并阶段增加限制。
- [x] `DELETE /upload/sessions/{id}` 在确认 session 属主前删除 chunk 目录。已改为先按当前用户读取 session，再删除对应目录和记录。
- [x] 下载响应中的 `Content-Disposition` 直接拼接文件名，旧数据中的特殊字符可能造成 header 解析失败。已使用清洗后的附件名。
- [x] 自动清理回收站任务直接拼接数据库路径删除文件。已改为使用 `StorageService::delete_file` 的路径校验。
- [x] `upload.rs` 中的 `session_to_response` 没有任何调用。已删除该死代码。
- [x] `tests/common` 构造 `AppState` 时漏掉 `notifications`，导致后端集成测试不能编译。已补齐测试夹具字段。

## 待处理

- [x] `config/production.toml` 没有被启动逻辑加载，并且写的是 PostgreSQL URL；当前服务只初始化 SQLite 连接。已支持按 `CONTRIX_ENV`/`APP_ENV` 加载 `config/{env}.toml`，并把生产配置改回 SQLite。
- [x] `S3Storage` 已实现并在配置中暴露 `backend = "s3"`，但上传、下载、缩略图、转换等路径仍只接入本地 `StorageService`。已引入统一 `StorageBackend`，本地/S3 均从启动配置初始化，上传、下载、缩略图、转换、WOPI 写入和清理任务统一走后端。
- [x] 迁移执行器吞掉执行错误，可能导致启动时数据库 schema 半成功。已改为读取迁移失败和非幂等 SQL 失败都会中止启动；旧库重放 `ADD COLUMN` 的 duplicate-column 会被显式视为幂等跳过。
- [x] 默认 JWT secret 是固定字符串，启动时没有阻止生产环境使用默认密钥。已增加生产环境强密钥校验，并支持 `${JWT_SECRET}` 从环境变量展开。
- [x] rate limiter 信任任意 `X-Forwarded-For`，且内存 map 没有淘汰策略。已默认只用远端 IP，新增 `trust_forwarded_headers` 显式开关，并清理过期计数项。
- [x] 分享密码使用裸 SHA-256，无盐且无 KDF。已改为 Argon2 带盐哈希。
- [x] `FileResponse` 返回内部存储 `path`，会暴露服务端存储布局。已对 API 序列化隐藏该字段，保留 Rust 内部结构字段以降低改动面。
- [x] WOPI access token 主要通过 URL query/form 传递，容易进入代理日志和浏览器历史。已对 host 页增加 `Cache-Control: no-store`、`Referrer-Policy: no-referrer`，脚本启动时从地址栏移除 `access_token`，并把传入 JWT 兑换为短期、绑定 user/file 的 WOPI session token。
- [x] `cargo check -p contrix-server --lib` 仍报告若干既有 warning，包括未使用 import、未接入的 `IpFilter`/`AuditService`/`EmailService`/`S3Storage` 等。已清理明确未使用 import、接入 `IpFilter`，并对保留但未接线的服务模块做显式 dead-code 标注。
- [x] `cargo check -p contrix-frontend` 报告未使用组件重导出、多余 `mut`、未调用的协作发送函数、未读取字段和过期 helper。已清理可自动修复项，并删除 `send_message`、`RemoteCursor::is_active`、`display_value`、`AuthState::is_authenticated` 等死代码。

## 后续建议

- [x] 如果需要真实 S3 支持，应引入 `StorageBackend` trait 并贯穿上传、下载、缩略图、转换输出和清理任务，而不是只保留单独的 `S3Storage` helper。已在 `_todos.md` 中拆解并完成。
- [x] WOPI 更彻底的 token 方案应使用短期/一次性 token 或服务端 session exchange，当前改动只是减少浏览器侧泄漏面。已在 `_todos.md` 中拆解并完成服务端 session exchange。
- [x] 前端 unused/dead-code warning 应清理到零，避免后续真实未接入功能被噪音覆盖。已在 `_todos.md` 中拆解并完成。

---

# 第三轮审查报告（安全漏洞 / 死代码 / 未完成 / 未接入）

本轮由四个分区评审 agent（handlers / services+middleware / 前端 / core+export+doc-engine+collab）独立扫描，覆盖前两轮之外的新问题。下列「本轮已修复」项已编码并通过 `cargo check`（server/doc-engine/export/frontend）与 doc-engine/export 测试。

## 本轮已修复

### 安全 / 越权（IDOR）
- [x] 文档评论无访问控制：`comments.rs` 的 list/create/resolve 任意登录用户可读取/注入/解决他人文档评论。已新增 `access::load_accessible_document`，三处统一按文档（viewer）授权。
- [x] 在线状态无访问控制：`presence.rs` 的 list/join 可枚举任意文档协作者与光标、把自己注入他人文档在线名单。已加文档 viewer 授权。
- [x] 文档签名无访问控制：`signatures.rs` 的 sign/list/verify 任意用户可对他人文档盖章、读取 `signature_data`。已将 `resolve_document_id` 改为带授权解析（盖章 editor、列举/验证 viewer），forbidden 折叠为 not-found 不泄漏存在性。
- [x] 文件锁越权 + lock_token 泄露：`file_locks.rs` 的 acquire 仅验文件存在（任何人可锁任意文件，DoS），check 向任何人返回 `lock_token`。已对 acquire 加 editor 授权、check 加 viewer 授权，并仅对锁持有者返回 lock_token。
- [x] Webhook 配置越权：`webhooks.rs` 的 list/get/update/list_deliveries 仅验登录，普通用户可读取全部 webhook、把回调 URL 改到攻击者服务器窃取事件。已对四个 handler 补 `require_admin`。
- [x] 邀请提权：`invitations.rs` 的 create 无角色门禁且 `role` 取自请求体，普通用户可创建 admin 邀请。已限定仅 admin 可创建邀请。
- [x] 第三方集成明文回吐密钥：`integrations.rs` 的 list/get 直接返回 `api_key`。已改为 `has_api_key: bool`。

### 安全 / SSRF
- [x] Webhook 投递无 URL 校验（SSRF）且明文回传共享密钥：`services/webhook.rs` 直接 POST 任意 URL、跟随重定向，并在 `X-Webhook-Secret` 头明文发送 secret。已新增 `is_safe_webhook_url`（拒绝非 http(s)、localhost、私有/环回/链路本地 IP 字面量），禁用重定向（`Policy::none()`），并删除明文密钥头仅保留 HMAC 签名。

### 未接入功能
- [x] DOCX/XLSX/PPTX 导出实现完整但服务端从未调用：`export` crate 的 `writer_to_docx`/`workbook_to_xlsx`/`presentation_to_pptx` 仅在自身测试中被引用，`convert.rs` 与 `documents.rs` 的导出 match 缺少对应分支（用户请求即得 400）。已在两处补 `(Writer,Docx)`/`(Calc,Xlsx)`/`(Slides,Pptx)` 分支。

### 逻辑 bug / 健壮性
- [x] `calc.rs` SUMIF 守卫 `args.is_empty()` 放行单参数，随后 `args[1]` 越界 panic。已改为 `args.len() < 2`。
- [x] `calc.rs` LEN 用 `s.len()`（UTF-8 字节数），中文/emoji 结果错误。已改为 `s.chars().count()`（与 LEFT/RIGHT/MID 一致）。

### 前端 未完成 / 假数据
- [x] `ai_assist.rs` 把占位提示文本当作 textarea 初始值，用户须先清空才能输入、否则被当真实 prompt 提交。已改为空初值 + `placeholder` 属性。
- [x] `dashboard.rs` 最近文档 fallback 渲染四张写死假卡片（Project Proposal 等），覆盖加载中与空状态。已改为加载提示 + 诚实空状态（新增 `dashboard.recent_empty` 中英文案）。

## 第四轮推进（已修复）

本轮继续处理上一节「待处理」的高/中危项，全部通过 `cargo check`、新增单元测试，以及完整的服务端集成测试套件（`api_tests`：130 passed）。

### 高危
- [x] `ws.rs` 协作 WebSocket 越权：upgrade 前已用 `access::load_accessible_document` 校验 editor 权限；接收 Operation/CursorUpdate 时强制 `op.user_id`/`cursor.user_id = 认证用户`，杜绝归属伪造。同时修复 WS 认证可用性——`jwt_auth_middleware` 现支持 `?token=` query 回退（浏览器 WebSocket 无法设 Authorization 头），否则该路由实际不可用。
- [x] `totp.rs` 假 2FA：已用自实现的 SHA-1/HMAC-SHA1（无新依赖）落地真正的 RFC 6238 校验，`verify_totp` 按 secret 计算并带 ±1 时间窗比对；新增 RFC 6238 测试向量与 base32 往返测试。（注：login 流程强制二因子仍属后续工作。）
- [x] `oauth.rs` 回调 stub：已停止回显授权 code，改为诚实返回 501 Not Implemented（与 AI stub→501 一致），并在注释中标注完整实现须持久化校验 `state` 以防 CSRF。
- [x] `collab/session.rs` OT 收敛：`Operation` 新增 `base_version` 字段（serde 默认），`apply_operation` 改用 `operations[base_version..]` 作为转换基准，不再误用 per-client counter；新增 base_version 保持性测试。
- [x] `collab/operation.rs` undo 逆操作丢数据：`Delete`/`SetProperty`/`RemoveElement` 新增可选 `removed_content`/`old_value`/`removed_element`，`inverse()` 在有捕获值时精确还原、无值时退化为安全的空插入/删除而非破坏文档；新增 inverse 还原测试。

### 中危
- [x] `admin.rs` update_user 已加 role 白名单（admin/editor/viewer/disabled）与「最后一个管理员」保护；`login` 拒绝 `disabled` 账户换取新 token。（注：已签发未过期 JWT 仍有效——中间件回查 DB / session 失效属后续工作。）
- [x] `ip_filter.rs` 已实现真正的 CIDR 匹配（IPv4/IPv6，含 `/0` 与非法规则处理），并补单元测试；精确 IP 仍按解析后比较。
- [x] `auth.rs` register/reset 已接入 `PasswordService::validate_password` 强度校验；默认策略放宽为仅强制最小长度（避免破坏既有账户/夹具，管理员可经 policy API 收紧）。（注：`forgot_password` 发信与 forgot/reset 独立限流仍待办。）
- [x] `rooms.rs` get_room/list_members、`groups.rs` get_group/list_members 已加成员/创建者校验（房间 owner 不一定在 room_members，故 member-or-owner 双查）；存在性先判定再鉴权，避免把已删除资源误报为 403。
- [x] `client_log.rs` 公开端点已限制单请求条目数（50）、字段长度截断、level 白名单，list 分页 per_page 钳制。
- [x] 限流退化为纯 IP：新增第二个 `user_rate_limiter` 挂在受保护子路由（JWT 之后），按认证 user_id 限流；边缘 IP 限流器保留覆盖公开路由。同一 NAT 下不同用户不再共享配额。
- [x] 前端多处按钮/控件未接线：`calc.rs` 对齐/填充色/文字色/字体/字号/Sort/f(x) 全部接线（CellStyle 扩展 align/colors/font + 持久化 + 行内样式渲染；合并单元格因需 colspan/rowspan 模型暂移除死按钮）；`admin.rs` 用户角色内联编辑、禁用、搜索/角色过滤、Add User 弹窗（接 register）、Save Settings（接 `/settings/:key`）、活动过滤、图表改为基于真实统计的柱状图（移除占位/死分页）；`navbar.rs` 搜索回车跳转文件管理；`file_manager.rs` 搜索接入 `FileList` 名称过滤；`slides.rs` layout select 绑定并与 layout-rail 取值一致；`comments_pane.rs` 新增 `parent_id`/回复 UI（线程化回复 + 回复指示器）。

## 第五轮推进（已修复 — 原低危 / 数据降级 + 收尾）

本轮处理剩余低危/数据降级项与未接入功能，服务端 `api_tests` 仍为 130 passed，collab/export 单测全绿，前端 `cargo check` 通过。

- [x] `EmailService` 接入：重构出返回 `Result` 的 `send`，并在 `forgot_password`（发重置链接）、`invitations`（发邀请）、`smtp.test_smtp`（真正尝试投递并回报结果）接线；移除 email 模块的 `#[allow(dead_code)]`。
- [x] `AuditService` 接入：`login` 成功、`admin.update_user`、`admin.disable_user` 写入 `activity_logs`（含 ip/user_agent）；`disable_user` 同时清理该用户 `sessions`。移除 audit 模块 dead-code 标注。（活动日志测试相应更新为校验 `auth.login` 条目。）
- [x] `S3Storage` 死代码：删除完全未用的 `copy_object`/`list_objects`，把 impl 级宽泛 `#[allow(dead_code)]` 收窄到具体 test-only helper；`generate_thumbnail_key` 扩展名修正为 `.jpg` 与 `ThumbnailService` 一致（测试同步）。
- [x] `pptx.rs` 元素丢失：导出新增 `Shape`（按 `ShapeType` 映射 prstGeom + 填充 + 文本）渲染，`Image`/`Chart`/`Video`/`Group` 改为带标签的占位框（保留位置与类型），不再静默丢弃。
- [x] `docx.rs` 列表降级：新增 `word/numbering.xml`（bullet=numId1 / decimal=numId2，各两级缩进）+ Content-Types/rels 声明，列表项改用 `<w:numPr>` + `ListParagraph` 样式，导出为真正的 Word 列表而非字面 `1. `/`• ` 前缀。
- [x] `documents.rs` 文档名消毒：`document_storage_name` 先经 `sanitize_file_name`；`documents.rs`/`convert.rs` 导出 `Content-Disposition` 文件名统一消毒并改用 `if let Ok` 避免 `.parse().unwrap()` panic 与 header 注入。
- [x] `smtp.rs` test_smtp：见上，已真正尝试发送测试邮件并按结果返回 200/502。
- [x] `shared_with_count`：新增 `access::count_shares_for_file`，在 files/folders/recent/favorites/trash/rooms 列表与单项响应中返回真实分享数（新建资源仍为 0，语义正确）。
- [x] `shares.rs` public_token：新增公开端点 `GET /public/shares/{token}`（`access_public_share`），校验存在性/过期/分享密码（Argon2）后流式下载文件，支持文档 JSON blob 回退。

## 最终复核补充（已修复）

- [x] 仪表盘仍残留假数据：近期动态在加载中/空数据时展示三条虚构活动，存储用量固定写死为 12% 和 120 MB。已删除虚构活动，补齐加载/空状态，并将存储用量接入 `/auth/me` 的真实 `storage_used` / `storage_quota`；相关文案全部接入中英文 i18n。

## 待处理（功能项 / 已知限制，非现存缺陷）

以下为新功能或需产品决策的工作，不属于本轮要解决的“现存问题”（安全漏洞 / 死代码 / 未接入 / 假实现）：

- [ ] 协作实时编辑的“发送 Operation”链路：前端目前仅发送光标更新（CursorUpdate），尚未发送文档 Operation，故服务端 OT/undo 修复为正确但暂未被前端触发（新功能）。
- [ ] 登录强制二因子：TOTP 已可校验，但 login 未强制第二步（新功能）。
- [ ] 已签发未过期 JWT 的中间件 DB 回查/即时失效（当前靠 login 拒绝 disabled + 禁用时清 sessions 缓解）。
- [ ] 电子表格“合并单元格”：需 colspan/rowspan 渲染模型，本轮先移除了无功能的死按钮（新功能）。
- [ ] DOCX 导入 `docx_to_writer` 尚不识别 `<w:numPr>` 还原为 `Block::List`（导出已是真列表；导入还原为后续工作）。

## 最终验证

- [x] `cargo fmt --all -- --check` 通过。
- [x] `cargo check --workspace --all-targets` 通过且无编译 warning。
- [x] `cargo test --workspace --all-targets` 全部通过，其中服务端 API 集成测试 130 项通过。
- [x] 前端中英文 i18n JSON 解析通过，Playwright `--list` 成功发现 87 项测试，说明测试源码可加载。
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` 尚未清零：现有失败项为 `collapsible_if`、`derivable_impls`、`unnecessary_sort_by`、`should_implement_trait` 等风格/可维护性 lint；本轮没有用 `allow` 批量压制，留待独立机械清理。
