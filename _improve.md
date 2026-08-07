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

## 第六轮推进（原功能项 / 已知限制已完成）

- [x] 协作实时编辑的“发送 Operation”链路：Writer 编辑输入现发送带 `client_id`、单调 counter 与 `base_version` 的 `Replace` Operation；客户端消费服务端 Operation、过滤自身回显并实时更新编辑器，服务端在连接后下发版本基线、拒绝未来版本/重复操作，并持久化实际转换后的 Operation。新增消息形状、远端文本还原及 session 边界测试。
- [x] 登录强制二因子：`LoginRequest`/登录页已接入可选 TOTP code；账户启用 TOTP 后，服务端在密码校验通过后强制验证 RFC 6238 code 或一次性备用码，缺失/错误/重复使用均拒绝签发 JWT并计入失败尝试。备用码改为 Argon2 哈希存储并原子消费。新增“缺失/错误拒绝、TOTP/备用码放行、备用码不可复用”集成测试。
- [x] 已签发未过期 JWT 的中间件 DB 回查/即时失效：注册和登录签发 JWT 时同步创建哈希 session；认证中间件每次请求校验 session 存在、active、未过期及用户未禁用，并使用数据库中的实时角色。改密/重置密码会撤销全部活动 session；新增 session 撤销后旧 JWT 立即 401 的集成测试。
- [x] 电子表格“合并单元格”：Calc 已恢复合并/取消合并按钮，完成越界/重叠校验、锚点选择、`rowspan`/`colspan` 渲染、隐藏单元格跳过、undo/redo 与 JSON 持久化，并在插入/删除行列及工作表删除/换序时同步调整合并区域；XLSX 导入/导出保留 `<mergeCells>`。新增水合、非法区域清洗、行列变更与 XLSX 往返测试。
- [x] DOCX 导入 `docx_to_writer` 识别 `word/numbering.xml` 与段落 `<w:numPr>`，按 `numId`/`ilvl` 将连续段落还原为有序或无序 `Block::List`，嵌套项恢复到 `ListItem.children`；表格单元格内列表同样支持。新增有序、无序及嵌套列表往返测试。

## 第七轮复审（本轮新增发现并修复）

- [x] TOTP 备用码原来只有 32 bit 熵、以明文保存且没有接入登录消费。现改为 128 bit 随机码、Argon2 哈希存储、一次性原子消费；兼容旧 8 位备用码并在使用后升级剩余哈希。登录输入和中英文提示已同步支持备用码。
- [x] 已登录用户可在不再次证明身份的情况下覆盖 TOTP 配置或直接关闭二因子。`setup` 现强制校验当前密码，`disable` 现强制校验有效 TOTP/备用码；集成测试覆盖错误密码、无效验证码及备用码不可复用。
- [x] JWT query 参数回退曾对所有受保护 HTTP API 生效，令 bearer token 更易进入访问日志。现仅在 `/api/v1/ws/` 且请求确为 WebSocket Upgrade 时接受 `?token=`；普通 HTTP query token 明确返回 401。
- [x] Session 管理接口在无法取得当前 bearer token 时会回退到“最近一个 session”，可能操作错误会话。现只按当前 bearer token 的哈希精确定位，缺失时失败关闭。
- [x] 注册流程先提交用户、后创建 session；session 写入失败会留下无法正常完成注册的孤儿账户。现将用户和 session 写入放入同一数据库事务。
- [x] Calc 导入的合并区域可能越界、重叠或退化为单格，并且排序可穿过合并区域破坏语义。现统一规范化和过滤非法区域，对与合并区域相交的排序/自动筛选给出阻止提示，并补清洗测试。
- [x] 复核 Calc 合并保护时发现保护逻辑误挂到“插入行”，且把行号当列号检查；真正的列标题排序未受保护。现移除错误的插行拦截，并在升序/降序列排序入口按完整列范围检查合并区域。
- [x] DOCX 列表导出递归可以产生任意 `ilvl`，但 Word 编号定义未覆盖全部合法层级。现定义 OOXML 的 0..8 共九级编号，并将更深应用层级安全压平到第 8 级；测试逐级校验编号定义。
- [x] Calc 已接入与 Writer 相同的协作 WebSocket，以整个 workbook 的 `Replace` Operation 同步本地保存和远端变更。当前是正确性优先的整文档同步基线；细粒度单元格 OT、Slides/Whiteboard 实时编辑仍属于后续架构演进，而非本轮残留死代码。

## 第八轮复审（依赖安全与未完成接线）

- [x] `cargo audit` 初始发现 16 个已知漏洞，覆盖 `lettre` 的 TLS 主机名校验、`quick-xml` 的恶意 XML 解析 DoS、Diesel SQLite 未对齐访问，以及 AWS-LC、rustls-webpki、QUIC 和 crossbeam 等传递依赖。已升级 `lettre`、`quick-xml`、`calamine` 并刷新兼容锁文件，复扫结果为 0 个已知漏洞。
- [x] 唯一没有上游修复版本的 `rsa` Marvin timing advisory 来自并未使用的 Salvo JWT 扩展和 `jsonwebtoken` 默认 PEM/RustCrypto 特性。项目实际只签发/校验 HS256；现移除 Salvo `jwt-auth` 特性，并为 `jsonwebtoken` 关闭默认特性、显式选择已修补的 AWS-LC provider，从依赖图中彻底移除 RustCrypto `rsa` crate，同时避免未选择 provider 时的运行时 panic。
- [x] `quick-xml 0.41` 移除了 `BytesText::unescape`，`calamine 0.36` 废弃了旧合并区域 API。DOCX/PPTX 导入现显式执行“按 XML 编码解码 + 实体反转义”，XLSX 导入改用 `merge_cells_by_sheet_name`，Office 往返测试继续覆盖这些路径。

## 仍待处理（本轮不冒充已完成）

- [ ] AI assistant 前端虽已调用 `/api/v1/ai/assist`，后端仍固定返回 501；完整实现需要文档访问控制、模型提供方配置、额度/超时、审计和输出安全策略。
- [ ] OAuth redirect 会生成但不持久化 `state`，callback 固定返回 501；应在界面隐藏未配置入口，或完整实现 state/PKCE 校验、code exchange、账号绑定与 session 签发。
- [ ] 管理员备份通过文件复制运行中的 SQLite，在线恢复还会覆盖连接池正在使用的数据库文件；应改用 SQLite Online Backup，并采用“校验后暂存、重启时原子切换”的恢复流程。
- [ ] `/api/v1/ws/notifications` 后端 WebSocket 没有前端消费者；当前通知仍只能依赖 HTTP 获取，应接入并补断线重连/去重测试，或删除未使用端点。
- [ ] TOTP setup/verify/disable 后端已完整，但前端账户设置没有管理入口；需补当前密码确认、二维码/secret 展示、备用码一次性展示下载与关闭二因子确认流程。
- [ ] `cargo audit` 仍报告 1 个 informational/allowed warning：Tantivy 0.25 传递依赖的 `lru 0.12.5` 存在 `IterMut` soundness 问题，修复版本要求 `lru >=0.16.3`，超出 Tantivy 当前 semver 约束。项目代码未直接调用该迭代器；仍应跟踪 Tantivy 上游升级，不能把它误报为完全无 advisory。

## 版本基线说明

- 本地 `main` 已包含所有本地分支提交，且没有未合并的本地分支。
- 当前 Git 配置没有任何 remote；`Cargo.toml` 声明的 `https://github.com/contrix-org/office` 返回 Repository not found，因此无法执行远端 `fetch/pull` 或证明远端 `main` 最新。恢复有效 remote 后应补做同步与推送。

## 最终验证

- [x] `cargo fmt --all -- --check` 通过。
- [x] `cargo check --workspace --all-targets` 通过且无编译 warning。
- [x] `cargo test --workspace --all-targets` 全部通过，共执行 326 项（collab 12、doc-engine 82、export 43、frontend 16、server lib/main 各 20、server API 集成测试 133）。
- [x] `cargo check -p contrix-frontend --target wasm32-unknown-unknown` 通过。
- [x] 前端中英文 i18n JSON 解析通过，Playwright `--list` 成功发现 87 项测试，说明测试源码可加载。
- [x] `cargo clippy --workspace --all-targets -- -D warnings` 已清零；逐项修复嵌套条件、可派生 Default、无效 clone、冗余闭包/排序以及易与 `FromStr` 混淆的 helper 命名，未使用全局 `allow` 压制。
- [x] `cargo audit` 复扫为 0 个 vulnerability，另有 1 个已在“仍待处理”记录的 transitive informational warning（未配置 ignore）。
