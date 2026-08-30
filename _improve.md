# 改进审查报告

## 第十三轮推进（2026-08-31，当前结论）

本轮按要求处理下方「产品能力建议」及「死代码、未完成或未接入」两组项目，补齐可用的基础实现、前端入口、持久化和 Office 导出链路。工作直接基于本地 `main@5fdf767`；仓库仍无 remote，无法校验远端最新版本或推送。

本轮增加的重点包括：DOCX 专业排版部件；PPTX 真实媒体、图表、分组、母版/主题与动画；Calc 条件格式、验证、命名区域、冻结窗格、透视汇总和图表定义；Slides/Whiteboard 协作；统一服务状态提取；OpenAPI、Prometheus 和转换/Webhook 重试。

### 实现范围与兼容边界

- 本轮勾选表示基础能力和消费链路已经实现，不表示与 Microsoft Office 的全部格式/编辑功能等价。
- PPTX 导出使用真实 OOXML 对象及关系；XLSX/PPTX 中附带原生模型以保留本产品字段。缓存按各包部件 SHA-256 校验，仅在文件未被外部修改时用于无损回读；外部修改后回退标准解析，复杂第三方 PPTX 对象仍可能降级为文本。
- Calc 透视表会实际生成分组汇总单元格并持久化定义；XLSX 保留汇总结果与本产品定义，不声称已实现 Excel 原生可刷新 pivot cache。命名区域、验证、条件格式、冻结窗格和图表均写入相应 OOXML 部件。
- Writer 专业字段已进入文档模型和 DOCX 导出；目录/页码/邮件合并使用 Word 字段，最终分页和字段刷新由 Office 应用处理。当前浏览器编辑器不是完整的 Word 分页布局引擎。
- 保存兼容性新增跨层验证：Writer 文本编辑保留未编辑的富内容块及 run 样式；Calc 统一坐标/单元格数组的 JSON 编解码、兼容历史空 map 与缺省格式；Whiteboard 保留原图层、旋转和填充属性。四类编辑器保存结果均有文档引擎类型反序列化测试。
- 命名区域可通过地址框按名称定位并选中；Calc 当前前端只提供列表验证、阈值条件格式及分组求和透视的基础编辑入口，其他规则保留在模型/导出层。命名区域公式求值、复杂透视编辑器、第三方 Office 全格式往返不在本轮基础验收范围。
- Slides/Whiteboard 当前采用带冲突提示的整文档实时替换，Calc 单格编辑使用细粒度操作；重连同步持久快照。此版本不宣称具备离线 CRDT 或对象级无冲突合并。
- 运维接口：`GET /api/v1/openapi.json`；管理员 `GET /api/v1/admin/monitoring/metrics`；属主 `POST /api/v1/convert/{id}/retry`；管理员 `POST /api/v1/webhooks/{id}/deliveries/{delivery_id}/retry`。转换任务先写库再后台执行，启动时恢复 pending/processing。

### 验证记录

- [x] `cargo test --workspace --all-targets --quiet`：394 项通过，0 失败（collab 12、doc-engine 84、export 48、frontend 37、server lib/main 各 35、HTTP API 143）。
- [x] `cargo clippy --workspace --all-targets -- -D warnings` 通过；最后的前端追加改动再次通过 `cargo clippy -p contrix-frontend --all-targets -- -D warnings`。
- [x] `cargo check -p contrix-frontend --target wasm32-unknown-unknown` 通过。
- [x] `cargo fmt --all -- --check`、`git diff --check` 通过；原有 handler 状态提取 `unwrap` 搜索无残留。
- [x] `cargo audit` 更新 RustSec 数据后扫描 559 个依赖，0 vulnerability、0 warning。
- [x] Playwright `--list` 发现 87 项既有测试；仅验证测试发现，本轮未执行浏览器交互测试，也未进行桌面 Word/Excel/PowerPoint 的打开及版式验收。

## 第十二轮复审（2026-08-30 至 2026-08-31，历史与本轮接续清单）

本轮从本地 `main@4cf95ef` 开始，并在 `main@887d424` 上继续完成未勾选项，覆盖 Rust workspace、HTTP 路由与认证中间件、配置/迁移、前端四类编辑器、Office 导入导出以及现有测试。仓库没有配置 remote，因此只能确认 `main` 是全部本地引用中的最新提交，不能证明或拉取远端最新版本。

### 本轮已改进

- [x] **API Key 是完整 CRUD 但完全不能用于认证。** `api_keys` 表、创建/更新/吊销接口早已存在，但受保护路由只运行 JWT 认证，生成的密钥没有任何消费端；`permissions`、`expires_at` 也只是写库。现已接入 Bearer API Key 认证，实时校验摘要、启用状态、RFC 3339 过期时间、用户禁用状态和数据库中的实时角色，并更新 `last_used_at`。
- [x] **API Key 权限原来是任意字符串且不执行。** 现只接受 `read`、`write`、`read,write` 或 `*`，规范化大小写/顺序并拒绝未知或空 scope；安全 HTTP 方法要求 `read`，变更方法要求 `write`，不足时返回 403。新密钥使用可识别的 `ctx_` 前缀和 256-bit 随机值，数据库仍只保存 SHA-256 摘要。
- [x] **写权限 API Key 可调用密钥管理接口并给自己扩权。** 现强制 API Key 的创建、更新、吊销与列表必须使用 JWT 会话；API Key 访问该路径固定返回 403，防止 `write` key 铸造 `*` key。
- [x] **API Key 名称和过期时间缺少输入验证。** 现拒绝空名称、超过 100 字符的名称、非法时间和已经过期的时间；吊销和权限变更在下一请求立即生效。新增单元与端到端测试覆盖认证、只读拒绝写入、权限热更新、吊销和非法输入。
- [x] **Webhook “HMAC” 实际是 `SHA256(secret || payload)`。** 旧实现不是 HMAC，既不兼容标准验签器，也有长度扩展类风险。现改为标准 HMAC-SHA256，并用公开标准向量锁定输出格式 `sha256=<hex>`。
- [x] **第三方集成密钥仍会在更新/创建响应中原样回吐。** 旧报告只修复了 list/get，upsert 两个响应仍返回 `api_key`。现四类响应都只返回 `has_api_key`，并增加回归测试保证响应正文不含密钥。
- [x] **Capabilities 返回与实际配置/实现不一致。** `max_upload_size_mb` 固定写成 100（生产配置实际为 500，测试配置为 10），还宣称未实现的 50 MB 文档限制和 LanguageTool/DeepL 集成。现上传限制读取运行时配置、删除虚假的文档限制、未接入集成返回空数组，并公开 `api_key_authentication=true`。
- [x] **最新依赖审计发现 `h2 0.4.15` DoS 漏洞及已撤回的 `chacha20 0.10.1`。** 已将锁文件分别更新到 `h2 0.4.16`、`chacha20 0.10.2`；重新扫描 559 个依赖为 0 vulnerability、0 warning。
- [x] **Webhook DNS SSRF / DNS rebinding。** 投递前及每次重试都会重新解析全部 A/AAAA，拒绝任一私网、环回、链路本地、共享地址、基准测试、组播、未指定和保留地址；请求禁用环境代理与重定向，并把连接固定到本次已校验地址，避免 DNS 二次解析绕过。
- [x] **Webhook 响应体可造成内存/数据库放大。** 响应改为流式读取且最多保留 64 KiB，超限内容写入明确截断标记，不再调用无界 `response.text()`。
- [x] **未接入的第三方集成 CRUD 与明文 secret。** LanguageTool/DeepL 等配置没有消费者或前端入口，却把 `api_key` 明文写入 SQLite。现删除 handler、路由、DTO、模型和 schema，并新增迁移 024 删除 `integration_settings` 表；Capabilities 继续诚实返回空集成列表。
- [x] **API Key 只能做全局读/写授权。** 在保留 `read`、`write`、`*` 兼容性的同时，新增 `<resource>:read` / `<resource>:write`（如 `documents:read,files:write`），按 `/api/v1/<resource>` 路由族执行且拒绝未知资源/动作；端到端测试验证 `folders:write` 可建文件夹但不能写 documents。
- [x] **Writer 图片在 DOCX 中退化为文本占位。** DOCX 导出会把 PNG/JPEG/GIF base64 data URL 写入 `word/media`、关系文件和真实 `w:drawing`，保留尺寸与 alt text；导入会安全解析关系并恢复 `Block::Image`，同时限制单图 20 MiB、总计 50 MiB、拒绝关系路径穿越。远程 URL 不由同步导出器主动抓取，仍以可见占位保留。

### 仍待处理：安全与可靠性

- [ ] **P2 — CSP 仍允许 `unsafe-inline` 与 `unsafe-eval`。** 这会明显削弱 XSS 防护。需要确认 Dioxus 产物约束后改用 nonce/hash，逐步移除两项放行，并补浏览器端 CSP 回归测试。

### 仍待处理：死代码、未完成或未接入

- [x] **P1 — Slides 媒体/图表/组合对象导出。** Image/Video 写真实关系和媒体对象，Chart 写图表部件，Group 写原生分组；保留尺寸、裁剪、备注和动画，并加入母版/布局/主题关系。原生模型回读和部件内容有测试覆盖。
- [x] **P2 — Calc 条件格式与数据工具接线。** 条件规则可编辑并参与单元格渲染；列表验证在输入提交时执行；命名区域、透视分组汇总、冻结窗格和图表定义已接入保存与 XLSX 导出。前端保存保留规范格式及高级字段。
- [x] **P2 — Slides/Whiteboard 实时协作发送链路。** 两个页面均发送、接收协作操作并处理自回显、错误、断线重连与持久快照；前端显示并发覆盖提示。关闭页面显式终止重连，防止后台重连残留。
- [x] **P2 — 文档高级排版基础能力。** Writer 增加页眉页脚/页码、分页/分节、脚注尾注、目录/参考文献、邮件合并字段和修订模型；菜单接入主要编辑动作，DOCX 输出相应部件/字段/修订标记，并回读页眉页脚、注释与页面设置。
- [x] **P3 — handler 状态提取 panic。** 203 处状态 `unwrap` 全部替换为统一 `require_app_state!`，缺失状态返回结构化 500；编译器清理了不再使用的 Arc/AppState 导入。

### 产品能力建议（按价值排序）

- [x] **Office 保真优先：** 已完成本产品 PPTX 图片/图表/分组对象的真实 OOXML 导出与校验式原生往返，DOCX 图片及专业排版继续保留；外部复杂文件的兼容边界见本轮说明。
- [x] **协作一致性：** Slides/Whiteboard 实时同步已接入；Calc 单格修改改为 SetProperty，批量/结构修改保留 Replace；新增单格补丁、重连退避、显式关闭和消息解析测试，并显示并发覆盖提示。
- [x] **Calc 数据工具：** 条件格式、数据验证、冻结窗格、命名区域、实际透视汇总和图表定义均已有前端/模型/持久化链路及 XLSX 部件覆盖。
- [x] **Writer 专业排版：** 专业排版字段、菜单动作、DOCX 部件及修订持久化已接通；前端不再把文档保存退化成仅含正文的临时 JSON。
- [x] **Slides 演示闭环：** 主题选择、对象/图表插入与预览、动画导出、演讲者当前/下一页和备注视图、计时/重置均已接入。保存升级为规范 Presentation 模型，幻灯片重排保持原对象归属。
- [x] **可运维性：** 提供 OpenAPI 3.1 文档、管理员 Prometheus 指标、持久转换任务后台执行/启动恢复，以及转换和 Webhook 失败重试接口与权限检查。

### 本轮验证记录

- [x] 新增 API Key 认证单元测试 2 项通过。
- [x] 新增 API Key 端到端测试 2 项通过。
- [x] 服务端 lib/main 单元测试各 34 项通过（含新增 Webhook HMAC、URL 与公网地址范围测试）。
- [x] 废弃集成配置的 handler/路由/schema/model/DTO 已无残留引用，完整 API 测试启动链成功执行迁移 024。
- [x] `cargo fmt --all -- --check` 与 `git diff --check` 通过。
- [x] `cargo check --workspace --all-targets` 通过且无 warning。
- [x] `cargo clippy --workspace --all-targets -- -D warnings` 通过。
- [x] `cargo test --workspace --all-targets` 使用更新后的锁文件通过，共 370 项（collab 12、doc-engine 82、export 44、frontend 23、server lib/main 各 34、server API 141）。
- [x] `cargo check -p contrix-frontend --target wasm32-unknown-unknown` 通过。
- [x] `cargo audit` 扫描 559 个依赖，结果为 0 vulnerability、0 warning。
- [x] Playwright `--list` 在 `e2e/` 成功发现 87 项测试。

---

以下为前十一轮历史记录。

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

## 第二轮已完成

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

## 后续建议（已完成）

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
- [x] `totp.rs` 假 2FA：已用自实现的 SHA-1/HMAC-SHA1（无新依赖）落地真正的 RFC 6238 校验，`verify_totp` 按 secret 计算并带 ±1 时间窗比对；新增 RFC 6238 测试向量与 base32 往返测试。登录强制二因子已在第六轮完成。
- [x] `oauth.rs` 回调 stub：已停止回显授权 code，改为诚实返回 501 Not Implemented（与 AI stub→501 一致），并在注释中标注完整实现须持久化校验 `state` 以防 CSRF。
- [x] `collab/session.rs` OT 收敛：`Operation` 新增 `base_version` 字段（serde 默认），`apply_operation` 改用 `operations[base_version..]` 作为转换基准，不再误用 per-client counter；新增 base_version 保持性测试。
- [x] `collab/operation.rs` undo 逆操作丢数据：`Delete`/`SetProperty`/`RemoveElement` 新增可选 `removed_content`/`old_value`/`removed_element`，`inverse()` 在有捕获值时精确还原、无值时退化为安全的空插入/删除而非破坏文档；新增 inverse 还原测试。

### 中危
- [x] `admin.rs` update_user 已加 role 白名单（admin/editor/viewer/disabled）与「最后一个管理员」保护；`login` 拒绝 `disabled` 账户换取新 token。JWT 中间件 DB 回查与 session 即时失效已在第六轮完成。
- [x] `ip_filter.rs` 已实现真正的 CIDR 匹配（IPv4/IPv6，含 `/0` 与非法规则处理），并补单元测试；精确 IP 仍按解析后比较。
- [x] `auth.rs` register/reset 已接入 `PasswordService::validate_password` 强度校验；默认策略放宽为仅强制最小长度（避免破坏既有账户/夹具，管理员可经 policy API 收紧）。`forgot_password` 发信已在第五轮接入，公开认证接口受边缘 IP 限流保护。
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

## 第九轮复审（SQLite 备份 / 恢复）

- [x] 管理员备份原来直接复制运行中的 SQLite 主文件，会漏掉 WAL 中已提交数据；在线恢复又会覆盖连接池仍在使用的数据库文件，可能造成数据丢失或连接状态分裂。现从活连接序列化一致快照并原子落盘，备份目录按数据库隔离；restore 先复制到临时文件、校验 SQLite header 与 `PRAGMA quick_check`，再生成 pending，返回 202 和 `restart_required`。下次启动在连接池创建前原子切换，并保留旧主库及 WAL/SHM 为唯一 recovery 文件；任何移动/激活失败都会回滚，损坏 pending 会中止启动且不触碰当前数据库。新增文件名/路径、损坏 pending、启动切换、WAL/SHM 保留和管理员 API 在线可用性测试。

## 第十轮复审（TOTP 账户安全接线）

- [x] 个人资料页原来没有 TOTP 管理入口，后端能力只能通过手工 API 调用。现接入状态查询、当前密码确认、纯本地二维码生成、手动 secret 展示、备用码一次性展示与本地下载、六位动态码验证启用，以及使用动态码/备用码确认关闭的完整流程；启用成功后立即清除页面内存中的 secret 和备用码。中英文文案保持键集合一致，输入按服务端格式过滤，新增二维码不外传密钥、备用码文件及本地化完整性测试。
- [x] TOTP `otpauth://` 标签原来直接拼接邮箱，`+`、`@` 和非 ASCII 字符可能被身份验证器误解析；现按 RFC 3986 对邮箱组件进行 UTF-8 百分号编码，并以单元/API 测试覆盖特殊邮箱。setup 中原来被静默吞掉的 pending 删除与 enabled 查询数据库错误也改为失败关闭并返回明确错误。
- [x] 复扫发现 Tantivy 0.25 仍锁定同时命中两条 soundness advisory 的 `lru 0.12.5`。现升级 Tantivy 至 0.26.1、适配 `TopDocs` 显式按分数排序 API，并将 `lru` 提升到 0.16.4，已消除 `IterMut` 的 RUSTSEC-2026-0002；搜索接口回归测试覆盖升级后的索引、查询和类型过滤。

## 第十一轮推进（原“仍待处理”已清零）

- [x] AI assistant：后端已实现可选 OpenAI-compatible chat-completions 服务；启动时校验 endpoint/key/model/限额，生产环境强制 HTTPS。接口在确认 viewer 文档权限后加载正文，限制 prompt/文档/响应长度，按用户做小时额度与请求超时，抵御正文 prompt injection、过滤控制字符、拒绝空响应，并只把模式、模型和字符计数写入审计，不记录 prompt/正文。前端通过 capabilities 自动隐藏未启用入口；集成测试覆盖越权折叠为 404、禁用态 503、真实 mock provider 调用、敏感内容不进入审计。
- [x] OAuth：项目没有前端入口，原 redirect/callback 也没有 state/PKCE/code exchange/session 的闭环。按本轮“不顾兼容性、不要残留”要求，已彻底删除 OAuth 配置、模型、schema、handler、路由和旧建表 SQL，并新增破坏性迁移 023 删除可能存有明文 access/refresh token 的 `oauth_accounts` 表。
- [x] 实时通知：前端 Navbar 已接入 `/api/v1/ws/notifications`，支持首次 REST 对账、实时 toast/未读数、单条及全部已读、去重、断线指数退避和服务端 lag 后 resync；浏览器回调由连接句柄持有并在销毁时释放，不再依赖 `Closure::forget` 泄漏。集成测试验证数据库落库与 WebSocket 推送同一事件。
- [x] `cargo audit` 的 lru advisory：删除未提供必要一致性且引入 advisory 的 Tantivy 二级索引及其完整依赖链，搜索改为直接查询 SQLite 当前文件名/文档正文，避免索引陈旧；同步删除配置、状态、更新钩子和过时文档。`cargo audit` 现为 0 vulnerability、0 warning。
- [x] 搜索改造复核发现 SQL 裸表达式的 `AND/OR` 优先级会让正文命中绕过 owner、回收站和 document_type 条件，存在跨用户正文泄露风险。现对名称/正文匹配整体加括号，并新增跨属主隔离、类型筛选、正文查询和 LIKE 通配符字面转义测试。
- [x] AI 上游响应同时按 Content-Length 和流式 chunk 累计值限流，避免恶意或异常 provider 让服务端无界缓冲 JSON；输出先清理控制字符和首尾空白再按 Unicode 字符安全截断。

## 清零结论

- [x] 本报告不存在未勾选条目；本轮指定的 AI、OAuth、通知 WebSocket、Tantivy/lru 四条均已实现或彻底删除，不保留 501 stub、未接线路由、死配置或有 advisory 的搜索依赖。

## 版本基线说明

- 本地 `main` 已包含所有本地分支提交，且没有未合并的本地分支。
- 当前 Git 配置没有任何 remote；`Cargo.toml` 声明的 `https://github.com/contrix-org/office` 返回 Repository not found，因此无法执行远端 `fetch/pull` 或证明远端 `main` 最新。恢复有效 remote 后应补做同步与推送。

## 最终验证

- [x] `cargo fmt --all -- --check` 通过。
- [x] `cargo check --workspace --all-targets` 通过且无编译 warning。
- [x] `cargo test --workspace --all-targets` 全部通过，共执行 357 项（collab 12、doc-engine 82、export 43、frontend 23、server lib/main 各 29、server API 集成测试 139）。
- [x] `cargo check -p contrix-frontend --target wasm32-unknown-unknown` 通过。
- [x] 前端中英文 i18n JSON 解析通过，Playwright `--list` 成功发现 87 项测试，说明测试源码可加载。
- [x] `cargo clippy --workspace --all-targets -- -D warnings` 已清零；逐项修复嵌套条件、可派生 Default、无效 clone、冗余闭包/排序以及易与 `FromStr` 混淆的 helper 命名，未使用全局 `allow` 压制。
- [x] `cargo audit` 扫描 559 个依赖，结果为 0 个 vulnerability、0 个 warning，未配置 ignore。
