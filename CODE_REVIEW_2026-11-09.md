# Office 项目多 Agent 代码评审与改进报告

评审日期：2026 年 11 月 9 日。基线：本地提交 f18c115（Unify Office UI and migrate database queries to async Horme leases）。开始评审时工作区无未提交变更。

原始评审建议先封闭文件读取与授权缺口，再建立可靠的文档提交和协作恢复协议；性能方面先修正公式重复求值、稀疏导出扫描和协作历史增长。后续多 agent 修复已按此顺序推进，当前结果如下。

原始评审汇总 **27 项有源码依据的问题，其中 P1 17 项、P2 10 项**。当时的“确认”指源码缺陷或可推演路径，其中两项另有内存 SQLite 复现，目录穿越有文件系统验证。原始问题各节保留当时的验收要求；后续修复及真实测试结果以下述执行记录为准。

## 修复执行状态

2026 年 11 月 9 日启动多 agent 修复。11 月 10 日的 `19d7409` 已通过 **Linux 555 项 Rust、20 项定点重复、Linux/Windows 各 136 项联合 HTTP/浏览器回归，以及两个 ZIP 的浏览器与强停重启验收**。联合运行均无重试，真实 Garage 兼容服务的 3 项验收亦通过。该版本的 8 并发测量保留 **0.75% 的 503 拒绝**。11 月 11 日续修完成 D 盘上的 Linux/amd64 容器实测，并加入鉴权容量、启动诊断和本地冷备演练。各轮证据分别列出，macOS 与真实 AWS 桶仍待运行验证。

### 11 月 11 日续修

从已验收的本地提交 `19d7409` 继续。并行推进请求鉴权与数据库容量、启动可靠性、容器与发布门禁，以及配套冷备恢复。此前 Windows/Linux ZIP 和容量数据保留原身份。

- JWT/API key 使用一个 SQLite 快照读取凭证、角色和租户活动状态。活跃时间展示按 30 秒节流，写入后在同一租约内复核授权。文档访问合并文档和文件查询，并减少所有者路径的重复租户读取。事务提交与长连接的撤权复核继续有效。503 增加服务端请求 ID，日志分别记录明确响应原因及同期观察到的数据库压力。SQL 耗时来自 SQLite 执行线程的已完成语句，独立业务任务不在该请求统计内。
- 启动首连接使用现有 checkout 和 operation 时限中较大的值，启动日志分别标记连接获取、SQLite 初始化及迁移。请求 checkout 和准入参数沿用既有配置。历史 Windows 失败实例退出前已进入 WAL，尚未写入 schema，两次历史超时的具体阶段仍缺证据。
- 同一源码的 Linux 原生 workspace 复跑 **560 通过、0 失败、4 忽略**，严格 Clippy 通过。新增 3 项鉴权 HTTP 回归实测常规所有者 GET 的 5 次租约与 5 条 SQL、节流写入、即刻撤权和 503 的请求 ID 关联；两项启动回归覆盖真实 SQLite 独占锁、迁移连接复用和迁移错误分类。首轮全量有 16 项服务器单元失败，部分明确停留在 WAL 初始化，其余主要是 checkout 或清理超时。事后 WSL I/O 压力采样及同一 ELF 锁用例 0.278 秒的通过记录均保留，无法将每次失败单独归因。 [全量结果](E:/Works/arkret-org/convert/office/target/review-phase3-validation/linux-rerun-summary.json) · [首轮失败](E:/Works/arkret-org/convert/office/target/review-phase3-validation/linux-workspace-first-failure.log) · [启动调查](E:/Works/arkret-org/convert/office/target/review-phase3-validation/linux-startup-investigation.json) · [Clippy](E:/Works/arkret-org/convert/office/target/review-phase3-validation/linux-clippy.log)
- 新增 CI 容器验收及三平台 ZIP 验收。Intel macOS 的 `x86_64-apple-darwin` ZIP 使用匹配的 `macos-15-intel` runner，同批 ZIP 共用一次 Web 构建。容器和 ZIP 共用静态资源哈希、真实浏览器和强停重启断言。静态工作流检查已通过，GitHub 托管 runner 的实际结果待执行。 [Actionlint 记录](E:/Works/arkret-org/convert/office/target/review-phase3-delivery-actionlint.log) · [GitHub runner 架构说明](https://docs.github.com/en/actions/how-tos/write-workflows/choose-where-workflows-run/choose-the-runner-for-a-job)
- O02/O03 的 Linux/amd64 镜像已在项目专用 D 盘 WSL2 rootless builder 上从干净上下文构建。注册表、VHDX、实际挂载设备、Docker/BuildKit、containerd 数据根和运行时卷均核对为同一 D 盘 ext4。现有 Docker Desktop 数据盘仍在 C。独立原始工作区归档逐项核对 2482 个镜像上下文条目，2478 个字节相同，四个差异均为期间编辑的报告或 README，编译输入全匹配。 [构建存储](E:/Works/arkret-org/convert/office/target/office-container-storage-proof.json) · [运行时存储](E:/Works/arkret-org/convert/office/target/office-container-runtime-storage-proof.json) · [上下文核对](E:/Works/arkret-org/convert/office/target/office-container-context-crosscheck.json) · [构建日志](E:/Works/arkret-org/convert/office/target/office-container-build-phase3.log) · [WSL 导入文档](https://learn.microsoft.com/windows/wsl/reference)
- 镜像 `sha256:4ce2efc2c7c496a34d19c70e5c12a9ee77e463e6132f065ed55623a2fa268ad0` 以生产配置、非 root 用户启动，HTTP 资源与镜像内文件哈希匹配。Chromium 登录保存、REST 下载、SIGKILL 后同卷恢复、WebSocket ACK 持久化和重放幂等均通过。首轮功能断言通过后本地包装器因 PowerShell 变量同名导致清理失败，已按容器和卷身份清理；中间一次包装器作用域校验在启动前失败。修正包装器并将通过报告移到清理之后，最终完整烟测退出码 0，确认无测试容器和卷遗留。镜像已导出，压缩包 SHA-256 为 `5a6b6e87accbdc3c7add4458f2df8552cb9ebd4921b7543bcc88381ea39c59ca`。 [最终烟测](E:/Works/arkret-org/convert/office/target/package-smoke/72406505711549ce8171b27f08136c32/result.json) · [首轮清理失败](E:/Works/arkret-org/convert/office/target/office-container-smoke-phase3.log) · [镜像文件](D:/Works/arkret-org/office-container-validation-20261111/contrix-office-linux-amd64-20261111.tar.gz)
- 使用上一轮 Windows release ZIP 完成真实冷备恢复演练。停止源进程后复制 SQLite、仍有效的 1,623,312 字节 WAL、当前文档对象和上传原件，共四个文件。只复制主数据库时缺少该文档。配套恢复到全新实例后，完整性检查、登录、Writer v3、两条历史记录和两类下载字节全部通过。负向副本删除及同长度修改均被清单检出，原备份哈希保持一致。演练范围为本地存储。 [运行结果](E:/Works/arkret-org/convert/office/target/disaster-recovery/3d63c264cd884b2f9c0d2236580b69a8/result.json) · [独立清单核验](E:/Works/arkret-org/convert/office/target/disaster-recovery/3d63c264cd884b2f9c0d2236580b69a8/independent-verification.json)

当前 Linux 原生 release `f365ce56…a3b102a5b` 另行通过 **136/136** HTTP/Chromium 回归，无失败、跳过或重试。新 ZIP 同样通过浏览器、REST 与 WebSocket 强停恢复、操作重放和 ELF `755` 校验，前一轮 ZIP 保留原路径。新版 ZIP 的本地冷备演练也通过。Linux 演练首轮因 PowerShell 子进程发现 metadata 超时而未启动，脚本改为直接调用 `cargo metadata --no-deps --format-version 1` 后复验。 [原生 release 构建](E:/Works/arkret-org/convert/office/target/review-phase3-validation/phase3-native-release-build.log) · [136 项结果](E:/Works/arkret-org/convert/office/target/review-phase3-validation/phase3-native-e2e-result.json) · [ZIP 烟测](E:/Works/arkret-org/convert/office/target/review-phase3-validation/phase3-native-package-smoke-result.json) · [新 Linux ZIP](E:/Works/arkret-org/convert/office/target/dist/contrix-office-x86_64-unknown-linux-gnu-phase3.zip) · [新 ZIP 冷备结果](E:/Works/arkret-org/convert/office/target/review-phase3-validation/linux-disaster-recovery-result.json)

镜像导出独立验证了配置和全部 6 层摘要，D 盘验证实例在容器及卷清空后停止，保留镜像与项目专用 VHDX。 [镜像校验](E:/Works/arkret-org/convert/office/target/office-container-artifact-proof.json) · [隔离实例清理](E:/Works/arkret-org/convert/office/target/office-container-shutdown-proof.json)

容量复测保持连接池 5、准入 10、每 worker 100 次、90% GET/10% CAS PUT 和零自动重试。普通文档混合流结束后，再以独立大文档运行 30 次串行导出并发冷文档读取，2 MiB 只适用于后一阶段。旧原生 ELF 与新容器 ELF 都在同一 Ubuntu 主机独立运行。2 并发两组均 200/200，吞吐 78.96 与 79.93 req/s；8 并发两轮均 800/800，正序旧/新 79.80/70.46 req/s，反序新/旧 70.86/72.83 req/s。编译工具链分别为 Rust 1.98.1 和 1.99.1，且交换顺序后差距缩小，因果归因仍需同工具链数据。旧 Windows 0.75% 的失败记录保留。 [首轮数据](E:/Works/arkret-org/convert/office/target/review-phase3-validation/performance/summary.json) · [反序新版](E:/Works/arkret-org/convert/office/target/review-phase3-validation/performance/after-8-reverse.json) · [反序旧版](E:/Works/arkret-org/convert/office/target/review-phase3-validation/performance/before-8-reverse.json)

相同 Rust 1.98.1 的两个原生 release 二进制随后按相同参数重新执行。旧版一次启动返回 `database checkout timed out`，测试流未执行。新版该组 200/200。两轮 8 并发均保留实际失败及耗时。

| 8 并发 | 成功/请求 | 成功 req/s | GET / PUT p95 ms | 失败 |
| --- | ---: | ---: | --- | --- |
| 旧版，正序 | 798/800 | 79.86 | 115.89 / 113.18 | GET 2 次 503 |
| 新版，正序 | 799/800 | 41.41 | 115.36 / 121.14 | PUT 1 次 503，操作时限 |
| 新版，反序 | 794/800 | 39.90 | 109.31 / 174.08 | PUT 3 次 429、2 次准入 503、1 次操作时限 503 |
| 旧版，反序 | 800/800 | 72.25 | 125.16 / 127.19 | 0 |

新版两轮 GET 均为 720/720。两次超时请求的服务器耗时约 15 秒，请求 ID 与 `documents / operation_timeout` 日志吻合。HTTP 任务的前置 4 次租约及 4 条语句均完成，实际 SQL 计时各小于 0.3ms。保存进入独立任务后，现有请求统计缺少其内部阶段。两个新版场景全程的 WSL I/O full stall 累计分别约 12.45 和 29.87 秒，两个旧版 8 并发场景分别约 0.07 和 0.10 秒。采样覆盖混合和独立导出阶段，无法分配到单条请求或据此归因吞吐变化。 [同工具链原始数据摘要与请求日志](E:/Works/arkret-org/convert/office/target/review-phase3-validation/performance/matched-native-summary.json)

从原数据库和 WAL 字节只读校验帧校验和，在内存重建后做 SQLite 完整性检查。正序超时的内容实际成为 v4，其后续历史证明元数据提交成功，事务内记录时间早于 503 约 15 秒。现有证据无法确定提交相对 503 的准确先后。反序超时的内容未进入版本历史，残留一条吻合时间和文件 ID 的 `writing` 租约，物理对象不存在。证据支持超时发生在对象登记的数据库操作附近，仍缺具体 SQL 或 worker 完成阶段。收到 503 的客户端应按版本重新读取状态，再用 CAS 决定是否保存。下一步关联保存任务内的凭证复核、预检、对象登记及最终写事务，并区分 worker 排队和实际运行，再验证保存超时与容量。 [只读 WAL 核对](E:/Works/arkret-org/convert/office/target/review-phase3-validation/performance/save-timeout-wal-evidence.json)

修正 metadata 发现方式后，冷备脚本在新版 Linux ZIP 和上一轮 Windows ZIP 上分别完成默认入口复验。Windows workspace Rust 构建期间读到了续修后的前端源码，依赖仍使用启动时的 feature 集合，因缺少 `RequestCache` 编译失败；当前清单已声明该 feature，完整 Windows 复验仍待重跑。上一轮 Windows 包的通过记录保留其原二进制身份。Intel macOS 与 AWS 专用桶仍需真实环境结果。 [Windows 冷备复验](E:/Works/arkret-org/convert/office/target/disaster-recovery/70b08eefdb504a22b3eb2043ea1cb008/result.json) · [本轮 Windows 构建日志](E:/Works/arkret-org/convert/office/target/review-phase3-validation/workspace-tests.log)

### 11 月 11 日保存超时续修

- 文档保存任务继承 HTTP 请求 ID。503 日志区分文档预检、对象登记、对象写入完成及最终元数据提交，也区分数据库 worker 排队、执行和返回阶段。保存前的凭证、权限和版本复核共用一次读取事务，最终写事务再次复核。已就绪对象的附加检查使用只读路径。
- Writer、Slides、Whiteboard 和 Calc 遇到保存响应丢失或网关超时时，重新读取一次文档。只有服务器版本推进且内容匹配已提交草稿时才确认保存，验证期间新输入的草稿继续保留。新增五项浏览器回归，待新 Web 构建实跑。
- Linux 定点运行的服务端单元测试为 88 通过、5 失败，涉及数据库 checkout 超时及测试同步通道提前关闭。鉴权集成测试前三项通过，新增写锁用例的会话字段名错误已修正，待重跑。对象登记超时后仍可能留下无实体文件的 `writing` 记录，过期回收与保存容量均待复验。上述 ZIP 和容器结果对应上一轮产物。

### 第一轮历史摘要

下列 496 项及第一轮发布包结果保留为历史证据，当前代码以第二轮最终验收为准，两轮不累加计数。

- 构建路径已恢复为调用者环境与 Cargo 正常配置；metadata 返回项目内 target。PowerShell 语法与环境不变检查通过，未重定向 Cargo home 或 TEMP/TMP。
- 完整 workspace 的 **496 项 Rust 测试全部通过**：协作 14、引擎 88、导出 50、外部格式回归 3、前端 80、服务端 261。服务端含 68 个单元测试、146 个 API 测试及 47 个新增集成回归；未将多轮重复执行相加。
- 严格 Clippy（workspace/all-targets/no-deps、项目 `-D warnings`）、最新 WASM check 及实际发布资源构建通过；最终 Playwright 联合运行 **136 项全部通过：HTTP 78、浏览器 58**。
- 上传超限专项连续 **30 轮全部通过**。另有 raw TCP 慢体与 1 TiB 长度声明回归，验证排空有界、5 秒内返回 413 JSON。测试客户端显式直连 loopback，避免系统代理掩盖原始传输错误；未改系统代理和应用网络配置。
- 最终服务器与浏览器发布资产已重新组装为完整 ZIP，且再次通过无源码目录启动、包内资源哈希、真实浏览器登录保存、WebSocket ACK 后强停进程重启恢复与相同操作重试不涨版。
- 依赖审计最初发现 25 条漏洞公告及 11 条告警；定向更新 24 个包后，最新 cargo audit --json --no-fetch 为 **0 vulnerabilities、0 warnings**。新依赖已通过上述完整编译与测试。
- 再次核对 Docker context/buildx：只有 default、desktop-linux 两个本机 docker-driver builder，没有远端 endpoint。Docker Desktop 设置中的 WSL 数据根及 DataFolder 均在 C 盘，已注册 Docker/Ubuntu WSL 路径也在 C 盘，不满足用户指定的存储条件，因此未运行镜像构建。已远端查询并确认两份基础镜像 manifest，Dockerfile 固定对应 digest；这不等同实际构建通过。
- 当前实现证据列于下方；原始审计部分的行号仍对应 f18c115 基线。

- SQLite 升为 libsqlite3-sys 0.39.1 bundled，并确认只链接一份 SQLite binding。发布版 Diesel 的依赖范围尚不兼容，临时固定到[官方兼容提交](https://github.com/diesel-rs/diesel/commit/fcd8d71e39bc2ba68b17dff0438b6aadd1869ca0)，不使用未固定分支、不 vendoring。发布兼容版本后应移除此补丁。

| 修复范围 | 第一轮实施状态 | 后续验收范围 |
| --- | --- | --- |
| O01–O03 构建和交付 | 合规路径、CI、服务器与 WASM 发布构建、最终 Windows ZIP 验收通过 | O02 及 O03 容器部分等待合规 builder 的真实构建和启动 |
| S01–S08 安全 | 16 项安全与 12 项消息回归通过，含同类入口与撤权 | 格式/设备边界见下节，无待修的已复现安全回归 |
| D01–D05、O04 持久化 | 14 项持久化集成及服务端取消单元回归通过 | 崩溃对账与历史容量策略见下节 |
| D06–D07 协作 | 有界历史、隔离、幂等、撤权、日志失败与最终发行包实际进程重启通过 | 协议、历史容量与断电边界见下节 |
| F01–F08 前端与格式 | 引擎、导出及前端共 221 项 Rust 测试通过，最新发布资产的 58 项浏览器回归全过 | 格式能力与协议边界见下节 |
| 运维与供应链扩展 | readiness/采集失败等 4 项运维回归、解压预算回归、新依赖全量 Rust 验收通过，audit 0/0 | 无压力、真实 S3 或全面依赖安全认证结论 |

以上是第一轮历史结果，第二轮最终证据如下。修复不等同全面安全认证或所有外部 Office 功能已实现。

## 第二轮验收（Windows，本轮界面修正前）

| 工作 | 已实施内容 | 对应验收范围 |
| --- | --- | --- |
| 历史版本保留 | 可配置最近版本数、最大年龄及批次上限；保护当前版本、原件和当前连续 OT 后缀所需的有界快照 | 事务清理、恢复、配额、并发写入回归 |
| 崩溃对象对账 | 本地与 S3 分页扫描、写入租约、删除 fencing、宽限期和持久 GC | 活跃慢写、崩溃窗口、引用竞态、跨实例及真实 S3 兼容实现回归 |
| 外部 Office 兼容 | PPTX 元素/坐标/样式/媒体/图表，XLSX 标准样式与当前模型可表达的结构 | 无原生缓存的外部 fixture、标准 parts、独立读取和浏览器验证 |
| 上传传输与静态资源 | 保留原始请求流以完成有界拒绝与成功收尾；文档 20 MiB / 分块 8 MiB；读取超时、预留配额、幂等预检、凭据在提交事务内复核；Windows junction 和缺失资产 404 | 未知长度 chunked、慢体/撤权、大小预算、配额并发、junction 和资源加载回归 |
| 供应链维护 | 固定 Glory 最小兼容官方修复提交；继续核实正式 Diesel 兼容版本 | MSRV、默认 feature 树、锁文件、审计及完整构建 |
| 性能与部署验证 | 可重复负载/计数/延迟与容量基线；真实 Garage S3 兼容服务端；可用平台验证 | 带参数和二进制身份的实测结果，明确未具备的远端平台与 AWS 条件 |

第二轮功能与交付验收已经完成。传输专项最初的空响应来自测试客户端提前 TCP 半关闭；fixture 已改为保持写端等待响应，未降低状态码或 JSON 断言。服务端同时补齐成功 multipart 的有界收尾，最终 7 项传输专项全部通过。以下实现边界不能被通过数量替代。

负载验收发现并修复了暂时数据库准入满被返回为 500 的分类缺陷：共享白名单将规范准入/checkout/SQLite 锁冲突映射为 503，真实语法错误或损坏仍为 500；HTTP 与 JSON 错误码一致，原有父目录访问 403 契约保留。新增 1 项分类单测和 2 项实际 HTTP 回归（鉴权后再占满真实池、真实 SQLite 写锁、真实语法错误），最终完整 Rust 回归 555 项通过。最终包与同参负载复测结果见下表。

修正前的 2 并发 200 次请求全部成功；8 并发 800 次中 793 次成功、6 次认证后端 503、1 次明确的数据库准入 500。不能从认证通用 503 文案断言其内部原因。原始失败数据与[中间分发包](E:/Works/arkret-org/convert/office/target/dist/phase2-before-http-backpressure-contrix-office-x86_64-pc-windows-msvc.zip)保留，性能修正前后不混合计数。

- 默认历史策略为普通版本最多最近 100 项且不超过 30 天，由后台按索引分批收敛；当前快照、原件和至多 2048 项当前连续 OT 后缀所需基础版本例外。积压收敛期间允许暂时超量，不宣称每次写入后同步硬裁剪。
- 新对象在物理写入前登记租约；过期写入和删除使用 token fencing，删除墓碑长期保留以阻止延迟写入复活。物理对象和普通历史可回收，不代表墓碑、业务历史或全部数据库表都有固定容量。
- 默认仅回收当前数据库登记的对象。未知旧对象收养需显式启用 `reconcile_legacy_dedicated_namespace`，并确认整个 bucket/base_path 由同一元数据数据库独占。不同数据库可共享桶中的不同物理键；复制数据库后分叉运行且继续共享相同键与 inventory 不在支持范围内。
- 全库实际 GC 待删队列达到 1024 项时，新的对象写入返回背压。仍被引用的冗余 GC hint 会退休；最终引用释放后重新入队。该策略会在存储删除故障时降低写入可用性，以限制持续积压。
- 外部 PPTX 已增加布局/母版继承、真实尺寸、文本样式、基本形状、图片裁剪旋转、缓存图表、备注与基本转换/动画映射；复杂图形、精确段落排版、复杂时间线不保证保真。视频保留媒体及几何信息，画布显示占位。表格近似为定位文本。
- 外部 XLSX 已增加当前模型可表达的样式、尺寸、冻结、命名范围、校验、条件格式、图表缓存及简单透视表。只有同表矩形图表来源保持动态绑定，其他情况使用缓存并告警；Calc 画布没有图表覆盖层。标准透视导出仅支持单行字段与单值字段的 sum/count/average，来源与目标需要前置表头。
- 用户已批准移除 WSL Cargo 的两项旧路径配置，现已完成并保留原配置备份；Linux 原生验证与本轮界面修正见下节。Docker 和 macOS 验证条件仍未具备。

### 第二轮已完成的验证

以下通过数来自单次最终联合运行；专项子集不重复计入总数。

| 验证 | 实际结果 |
| --- | --- |
| 完整 workspace / SSR Rust | [555 项通过，0 失败](E:/Works/arkret-org/convert/office/target/review-phase2-workspace-final.log)；4 项独立环境/发布模式测试默认忽略，另行执行 |
| 严格 Clippy | [workspace/all-targets/no-deps，`-D warnings` 通过](E:/Works/arkret-org/convert/office/target/review-phase2-clippy-accepted-final.log)，包含最终 WS 与 HTTP 分类修正 |
| 默认配置构建 | [workspace 默认 feature check 通过](E:/Works/arkret-org/convert/office/target/review-phase2-default-accepted-final.log)，未只验证 SSR 组合 |
| 浏览器资源 | [WASM check 通过](E:/Works/arkret-org/convert/office/target/review-phase2-wasm-check.log)；[最终 release + wasm-bindgen/CSS 组装通过](E:/Works/arkret-org/convert/office/target/review-phase2-web-accepted-final.log) |
| 联合 HTTP / 浏览器 | [最终复跑 136 项通过](E:/Works/arkret-org/convert/office/target/review-phase2-e2e-accepted-final.log)，HTTP 78、浏览器 58，使用最终代码/浏览器资源与独立实例 |
| 独立 Office 读取 | [openpyxl 3.1.5 / python-pptx 1.0.2 通过](E:/Works/arkret-org/convert/office/target/review-phase2-office-readers.log)，读取实际标准输出，不使用 Contrix 自定义缓存 |
| 依赖审计 | [0 vulnerabilities / 0 warnings](E:/Works/arkret-org/convert/office/target/review-phase2-audit-final.json)，基于已获取公告库 |
| 真实 S3 兼容服务 | [最终 Cargo 独立验收 3 项通过](E:/Works/arkret-org/convert/office/target/review-phase2-s3-accepted.log)：签名/特殊键/二进制读删、分页回收、独立数据库共享桶隔离；使用官方 Garage 2.5.0，不是模拟 HTTP 服务，不代表 AWS 生产桶验收 |
| 优化服务器与分发包 | [最终 release 构建通过](E:/Works/arkret-org/convert/office/target/review-phase2-release-accepted-final.log)；[本次容量基线对应的 Windows ZIP 留档](E:/Works/arkret-org/convert/office/target/dist/before-final-ui-20261110T000855Z-contrix-office-x86_64-pc-windows-msvc.zip)及[当次组包日志](E:/Works/arkret-org/convert/office/target/review-phase2-package-accepted-final.log) |
| 无源码启动、浏览器与恢复 | [最终分发包验收全部为 true](E:/Works/arkret-org/convert/office/target/package-smoke/1b5ef96fb5bd498da3f216788e47221d/result.json)：独立启动、浏览器、REST 重启恢复、WS ACK 后强停重启恢复、相同 operation receipt 幂等重试 |
| 原生 release 容量测量 | [1 项容量测试通过、5 个场景各 30 个样本](E:/Works/arkret-org/convert/office/target/review-phase2-capacity-release.log)；[指标与组件源码/测试二进制哈希](E:/Works/arkret-org/convert/office/tests/benchmark/baselines/windows-engine-release-2026-11-10.json) |

Rust 总数内含：89 项服务端单元、146 项 API、17 项外部高级格式、5 项慢请求/事务内撤权及 15 项消息回归；传输专项 7 项验证 2 MiB 保存、1 MiB 分块、顺序重试不写新对象、预留配额、未知长度与停滞连接。WS 测试使用真实数据库准入饱和和对象 PUT 屏障，验证等待期间不发正文/ACK，并在最终写事务复核凭据。

555 项按组件为核心 1、协作 14、引擎 88、导出 70、前端 82、服务端 300（89 单元、146 API、65 集成）。真实 S3 的 3 项属于默认忽略的独立环境验收，单独列出。Garage 的专用进程、挂载、loop 和临时凭据已[清理并记录](E:/Works/arkret-org/convert/office/target/garage-posix-c372ce82206847c0aee0c36c137be8c3/cleanup.json)，未操作 Docker 数据盘。

真实 S3 验收与引擎 release 测量先于最后的 HTTP 错误分类修正；相应 S3/维护/引擎/导出组件源码在该修正中未改变，最终 workspace 已复验这些组件的单元和集成用例。后续 HTTP 测量固定使用最终验收 ZIP 中的二进制，SHA-256 为 `A42812E282E6BC35EDEB6C9D81A2919E0A1466B4F4492F2E879863194570DD52`，与 ZIP 条目一致。

### 最终性能实测与后续优先级

[最终 HTTP 受控摘要](E:/Works/arkret-org/convert/office/tests/benchmark/baselines/windows-after-backpressure-2026-11-10.json)保留原始数据哈希、实例配置、样本数和全部失败。两个场景顺序运行，连接池均为 **5**、准入上限 **10**，checkout/operation/busy timeout 为 **3000/15000/3000 ms**。每 worker 100 次请求，90% GET、10% CAS PUT；热文档另有 30 次串行 JSON 导出。没有自动重试失败请求，没有为复测调整容量。

| 场景 | 成功/请求 | 成功 req/s | GET p50/p95/p99 ms | PUT p50/p95/p99 ms | 错误率 | RSS 采样峰值 | 冷文档样本：基线/热导出重叠 |
| --- | ---: | ---: | --- | --- | ---: | ---: | ---: |
| 2 workers / 32 KiB 热文档 | 200/200 | 53.51 | 33.19 / 55.80 / 76.83 | 34.42 / 82.90 / 345.19 | 0% | 19.57 MiB（110 样本） | 100/37 |
| 8 workers / 2 MiB 热文档 | 794/800 | 59.52 | 119.90 / 191.34 / 240.54 | 155.90 / 339.51 / 467.41 | 0.75% | 23.56 MiB（185 样本） | 100/80 |

8 workers 的 6 次失败均为 GET 503，80 次 PUT 全部成功；500、429、I/O 错误均为 0。该场景和联合摘要的 `success` 仍为 **false**，不能称为高负载零错验收通过。新增日志实测确认 `active_session` 1 次、`tenant_activity` 3 次均为数据库准入耗尽，另 1 次文档 GET 明确给出同一原因；第 6 次框架默认 503 没有内部日志，IPFilter 读取失败仅是源码推断，不能作为已确认归因。

原生 release 测量中，5000 个公式菱形、20000 个单元格的 p95 为 **29.01 ms**，实际求值 20000 格、解析 15000 个公式、45000 步表达式工作；普通和末行末列两份同为 1001 个非空格的 XLSX 导出 p95 分别 **33.74 / 33.66 ms**。每场景 30 个样本，导出后重导入校验值。此结果是本机原生引擎/导出时间，不是浏览器主线程时延或硬件无关 SLO。

[第一轮旧 release 基线](E:/Works/arkret-org/convert/office/tests/benchmark/baselines/windows-before-maintenance-2026-11-10.json)与[修正前的中间包测量](E:/Works/arkret-org/convert/office/tests/benchmark/baselines/windows-after-maintenance-2026-11-10.json)均留存。它们分别标识不同二进制；旧基线只覆盖 2 workers / 32 KiB，主机背景负载未受控。当前测量在构建、浏览器和组包工作结束后执行，但未固定整台主机的负载、功耗或温度，不作性能变化的因果或峰值吞吐承诺。

后续优先级：

1. **容量与可观测性：** 针对已确认的认证/租户检查准入拒绝，统计每请求的数据库租约数、排队与 SQL 耗时，评估共享读事务或满足撤权一致性要求的缓存；使用相同请求参数建立并发阶梯与明确 SLO。为所有 503 分支增加无凭据的原因与请求关联，确认默认 503 的实际来源。当前 5 连接下的 8 workers 不是零拒绝容量。
2. **运行平台：** Linux 原生与 D 盘 Docker 验收见执行记录。Intel macOS 的真实运行仍待托管 runner，AWS 兼容性仍待专用测试桶。
3. **灾备与产品能力：** 建立数据库与 blob 的一致备份/恢复演练；复杂 Office 排版/播放、更广泛图表和透视功能、离线协作、全面可访问性继续作为产品能力建设，不能以本轮回归替代认证。
4. **启动可靠性：** 两个独立诊断/基线实例曾在测试开始前因数据库 checkout 超时退出，分别保留 Linux 和 Windows 记录；完整验收的其他独立实例成功不能解释这些失败。需在受控负载下区分池初始化、连接创建与调度耗时，再决定是否改启动准入或时限；失败后的资源采样不构成原因证明。

## Linux 补充验证与跨平台界面修正（11 月 10 日）

用户批准后，仅移除了 WSL 全局 Cargo 中旧 `build.target-dir` 和强制 `env.TMPDIR` 两项设置及对应空表，[原配置备份](E:/Works/arkret-org/convert/office/target/linux-validation/cargo-config-before-20261109T222309Z.toml)保留。未访问旧配置指向的目录，未设置新的 Cargo home、target 或 TEMP/TMP 重定向。原 E 盘 workspace 的 Linux metadata 正常返回项目内 target，但该盘的 DrvFS/9p 挂载没有 POSIX metadata，文件映射为 root 所有；`aws-lc-sys` 复制头文件报 EPERM。这个[直接构建失败](E:/Works/arkret-org/convert/office/target/linux-validation/workspace-tests.log)保留，未修改挂载或全局权限。

随后使用独立原生源码工作区 `/home/chris/workspaces/office-linux-validation-20261110-6493d2a140`，其 metadata 返回本工作区的正常 `target`。复制了实际主仓库及 Glory/Sapid 工作树的 2480 个源码文件，包含未提交修正；没有仅归档旧 HEAD。[初始逐文件验证](E:/Works/arkret-org/convert/office/target/linux-validation/source-copy-proof.json)、[三文件修正证明](E:/Works/arkret-org/convert/office/target/linux-validation/source-ui-fix-proof.json)和[CSS 增量证明](E:/Works/arkret-org/convert/office/target/linux-validation/source-final-css-proof.json)保留，键盘增量证明见下文。四次增量分别只重新读取原仓库明确修改的 3/1/2/2 个文件，其余哈希继承先前验证；每次均重新核对全部 2480 个原生副本文件，不能表述为每轮重新读取了全部 E 盘源码。报告后续更新不影响已验证的构建输入。

[实际运行环境](E:/Works/arkret-org/convert/office/target/linux-validation/native-evidence/environment-and-export.json)为 Ubuntu 24.04 x86_64 / WSL2、Rust 1.98.1、Node 22.23.1、Playwright 1.58.2 与真实 Linux Chromium 145.0.7632.6。浏览器及其缺失的三个动态库仅解压到本工作区的验证工具目录；库来自签名验证的 Ubuntu 索引并核对 SHA-256，没有系统安装。浏览器路径和动态库路径仅对子进程生效，Cargo/TEMP 环境保持默认。未使用 Windows 浏览器替代 Linux 验收。

首次完整 Linux 浏览器运行为 **133/136**，失败记录保留，并据实际原因修正：

- Calc Undo/选区更新会替换 DOM，旧拖拽 helper 先解析出的 ElementHandle 在滚动前失效。改为 Playwright [Locator.dragTo](E:/Works/arkret-org/convert/office/e2e/tests/calc.ui.spec.ts:86)，仍执行真实鼠标手势，保留公式、Undo、保存与重载断言；这项失败本身不能证明产品填充逻辑错误。
- 同一浏览器任务内输入后立即 Escape，状态可回到最后渲染值，而绑定因新旧状态相等跳过 DOM 更新，公式框留下未提交文字。增加受作用域管理的[原生输入值同步](E:/Works/arkret-org/convert/office/crates/frontend/src/pages/calc/glory_calc.rs:1398)，并在[原 Escape 用例](E:/Works/arkret-org/convert/office/e2e/tests/calc.ui.spec.ts:839)中覆盖同任务输入/撤销及服务端内容未变。未修改 Glory 源码。
- Linux 字体度量令账户输入框实际高度为 40px，修正[最小高度为 42px](E:/Works/arkret-org/convert/office/crates/frontend/assets/semantic-layout.css:5)。测试继续深入后发现窄屏 Dashboard/Profile 导航栏溢出 7px：账户区不可收缩与品牌单行共同超过可用宽度。通过[窄屏品牌换行与账户宽度约束](E:/Works/arkret-org/convert/office/crates/frontend/assets/semantic-layout.css:62)恢复 390px 视口内完整显示，没有根节点溢出裁剪或降低断言。[布局原因与前后实测](E:/Works/arkret-org/convert/office/target/linux-validation/mobile-overflow/analysis.json)保留。

前三项修正后的 Linux 三组定点 **15/15**、完整 **136/136** 及分发包验收通过，[revision3 记录](E:/Works/arkret-org/convert/office/target/linux-validation/native-evidence/final-acceptance/summary.json)与[独立旧包](E:/Works/arkret-org/convert/office/target/dist/contrix-office-x86_64-unknown-linux-gnu-revision3.zip)保留。随后 Windows 同一资产完整运行为 **135/136**，暴露另一个时序缺陷：Shift+Arrow 同步更新逻辑选区，DOM 焦点延迟 20ms 移动，Delete 落在旧焦点时会重新激活单格并折叠选区。

新增的同任务回归在旧 Linux WASM 上[单次确定性复现](E:/Works/arkret-org/convert/office/target/linux-validation/native-evidence/keyboard-prefx-baseline/result.json)：预期 4 个选中格，实际 1 个，测试已执行且没有重试。修正[Delete/Backspace 的目标判定](E:/Works/arkret-org/convert/office/crates/frontend/src/pages/calc/cell_events.rs:146)，事件目标属于矩形选区或额外选区时保留整体，仅在选区外才重新激活；这一步尚未修改 20ms 延迟聚焦逻辑。[原 Shift/Delete 用例](E:/Works/arkret-org/convert/office/e2e/tests/calc.ui.spec.ts:663)保留真实键盘流程，追加两种清除键的同任务、额外选区和选区外目标验证，完整套件仍为 136 项。[两文件增量证明](E:/Works/arkret-org/convert/office/target/linux-validation/source-keyboard-final-proof.json)记录该轮源码快照。

原始 133/136、定点 10/15、Windows 135/136、旧 WASM 确定性失败和构建中断均保留，没有重写为成功。另一个 Windows 单例基线在测试启动前因数据库 checkout 超时退出，[独立记录](E:/Works/arkret-org/convert/office/target/linux-validation/final-ui-windows-shift-delete-old-web-baseline-result.json)明确 `test_executed=false`；失败后的资源采样不足以归因，未当作产品回归通过或失败。上述专项重复均不累加到完整套件数量。

早先 Linux UI 诊断也有一次[启动阶段 checkout 超时](E:/Works/arkret-org/convert/office/target/linux-validation/ui-diagnostics-before/analysis.json)，在 Node 测试前退出；后续对比使用原验收实例的固定 ELF 副本。该诊断不是完整验收替代，最终全套使用 Linux release ELF 的独立实例。两次启动失败的原因仍待专项核实。

键盘修正后的 revision4 在 Windows 完整 136 项及发行包通过，但 Linux 定点重复为 **19/20**：一次后续公式输入得到 `RetainedA`。[原失败与独立焦点诊断](E:/Works/arkret-org/convert/office/target/linux-validation/native-evidence/final-acceptance-revision4-failed/summary.json)保留，未继续其 Linux 全套或打包。诊断确定旧 20ms `focus_cell` 任务能在用户已进入公式框后夺回焦点；该焦点缺陷有事件及调用栈证据，`RetainedA` 的完整因果链没有确定，不能混为同一已证实结论。修正将延迟任务绑定到安排时的工作表、活动格和焦点，仅在目标与归属仍有效时执行；保留行内输入退出后通过 body 恢复下一格焦点的路径。现有用例增加受控释放单个旧定时器的回归，验证真实点击/输入后焦点及草稿保持，不以 sleep 规避竞态；[该断言在旧 WASM 上单次确定性失败](E:/Works/arkret-org/convert/office/target/linux-validation/native-evidence/keyboard-prefocus-baseline/result.json)。[最终焦点增量证明](E:/Works/arkret-org/convert/office/target/linux-validation/source-focus-final-proof.json)与此前证明形成连续快照链。

最终 revision5 的四组场景各独立运行 5 次，共 **20/20**，保留原正常输入与真实键盘流程，两个受控定时器场景也全部执行；随后 Linux 与 Windows 各 **136/136** 通过，均无失败、跳过、flaky 或自动重试。两份最终发行包亦完成原样 PowerShell 打包及真实浏览器/重启验收。以下是当前交付物，前述旧包与失败记录不覆盖。

| 最终验证 | 实际结果 |
| --- | --- |
| Linux workspace / SSR | [555 通过、0 失败、4 默认忽略](E:/Works/arkret-org/convert/office/target/linux-validation/native-evidence/workspace-tests-final.log)，含 Unix symlink 静态边界与本地扫描/删除专项；后续只改 WASM 专属键盘模块与 E2E，宿主 Rust 编译输入未变 |
| 严格 Clippy、默认配置、格式 | [all-targets / `-D warnings`](E:/Works/arkret-org/convert/office/target/linux-validation/native-evidence/clippy-final.log)、[默认 feature check](E:/Works/arkret-org/convert/office/target/linux-validation/native-evidence/default-check-final.log)、[最终项目 fmt](E:/Works/arkret-org/convert/office/target/linux-validation/native-evidence/focus-final-fmt.log)通过；键盘修正另有[真实 WASM target 严格 Clippy](E:/Works/arkret-org/convert/office/target/linux-validation/native-evidence/focus-final-wasm-clippy.log)通过 |
| WASM 与 Linux release | [最终 WASM/资源组装](E:/Works/arkret-org/convert/office/target/linux-validation/native-evidence/focus-final-web-build.log)、[原生优化服务器](E:/Works/arkret-org/convert/office/target/linux-validation/native-evidence/release-build.log)通过；后续修正仅涉及前端，服务器构建输入未变 |
| 原生真实 S3 兼容服务 | [3 项通过](E:/Works/arkret-org/convert/office/target/linux-validation/native-evidence/s3-acceptance.log)，[返回码与计数](E:/Works/arkret-org/convert/office/target/linux-validation/native-evidence/s3-result.json)另存；专用 Garage 进程及临时凭据已[清理](E:/Works/arkret-org/convert/office/target/linux-validation/garage-resumed-cleanup.json) |
| Linux 定点及完整 HTTP/浏览器 | [20/20、136/136 的最终记录](E:/Works/arkret-org/convert/office/target/linux-validation/native-evidence/final-acceptance-revision5/summary.json)，独立 release ELF 实例，全部原始断言执行，重试/跳过/flaky 为 0 |
| Windows 最终 HTTP/浏览器 | [136/136，重试 0](E:/Works/arkret-org/convert/office/target/linux-validation/final-ui-windows-20261110T003136Z-e2e.log)，使用实际导入的最终便携 Web 资产 |
| Linux 最终发行包 | [完整 ZIP](E:/Works/arkret-org/convert/office/target/dist/contrix-office-x86_64-unknown-linux-gnu.zip)；[无源码启动、浏览器、REST/WS 强停重启与 receipt 幂等全通过](E:/Works/arkret-org/convert/office/target/linux-validation/native-evidence/final-acceptance-revision5/summary.json) |
| Windows 最终发行包 | [完整 ZIP](E:/Works/arkret-org/convert/office/target/dist/contrix-office-x86_64-pc-windows-msvc.zip)；[无源码启动、浏览器及 REST/WS 重启幂等全通过](E:/Works/arkret-org/convert/office/target/package-smoke/37b3a56f89134135a117148bbec4235c/result.json)，[产物映射与日志](E:/Works/arkret-org/convert/office/target/linux-validation/final-ui-windows-revision5-artifact-mapping.json) |
| 独立交付身份核对 | [主 agent 对两份实际 ZIP 再核验](E:/Works/arkret-org/convert/office/target/linux-validation/final-cross-platform-artifact-verification.json)：20 份 Web 资产逐字节哈希一致且等于当前 target/web；服务器分别为 MSVC/ELF 原生 release，Linux 权限 755；旧容量基线 ZIP 身份保留 |

本轮 Web 资产在原生 Linux 构建后按文件 SHA-256 导入 Windows，[导入证明](E:/Works/arkret-org/convert/office/target/linux-validation/portable-web-import-proof.json)与两端 ZIP 核验相互对应；没有以 Windows 二进制替代 Linux 服务器。Windows 服务器仍为前述 `A42812…70DD52`，本轮改动不涉及 HTTP 后端；容量结果继续对应已留存的 `D63138…B1F86FE` 旧 ZIP，不声称重新测量了当前界面包的性能。最终功能验收的进程已退出，导出材料未包含测试数据库、私密运行配置或 JWT。

最终 diff 空白检查通过；[报告链接与 27 个原始编号检查](E:/Works/arkret-org/convert/office/target/linux-validation/report-check.json)及[四个本轮修改源文件/最终 manifest 哈希核对](E:/Works/arkret-org/convert/office/target/linux-validation/final-source-verification.json)通过。

以上验证覆盖当前 Ubuntu/WSL2 原生源码工作区，不代表 E 盘当前 DrvFS 挂载可直接构建，也不推广到其他架构、发行版或裸机环境。Docker、macOS、AWS 生产桶及一致备份恢复的边界继续保留。

## 第一轮最终验收记录

日志及分发物位于 metadata 返回的项目内 target，不属于 Git 提交内容。以下执行次数分别记录，没有将重复回归累加成通过数。

| 验收 | 实际结果与证据 |
| --- | --- |
| `cargo test --locked --workspace --features contrix-frontend/ssr --jobs 3 -- --test-threads=3` | [496 项通过、0 失败](E:/Works/arkret-org/convert/office/target/review-workspace-tests-final.log) |
| `cargo clippy --locked --workspace --all-targets --no-deps --features contrix-frontend/ssr --jobs 3 -- -D warnings` | [通过](E:/Works/arkret-org/convert/office/target/review-clippy-final.log)；上游 Glory 两条弃用告警不属于项目代码 |
| WASM check 与 release build | [最新目标检查](E:/Works/arkret-org/convert/office/target/review-wasm-final.log)、[优化构建](E:/Works/arkret-org/convert/office/target/review-wasm-release.log)及 [wasm-bindgen/CSS 资产组装](E:/Works/arkret-org/convert/office/target/review-web-build-final.log)均通过 |
| `pwsh ./scripts/test-e2e.ps1 -SkipBuild` | [136 项联合运行通过](E:/Works/arkret-org/convert/office/target/review-e2e-final.log)，覆盖 HTTP 78、浏览器 58；实例数据库、存储和端口独立 |
| 上传超限稳定性 | [30 轮严格 413 JSON 回归全部通过](E:/Works/arkret-org/convert/office/target/review-upload-limit-stress.log)；慢体及超大声明另由 146 项 API 套件中的专项验证 |
| 服务器 release 构建 | [真实优化编译通过](E:/Works/arkret-org/convert/office/target/review-release-build-final.log)，未用 debug 二进制代替 |
| 完整分发包与重启 | [第一轮 Windows ZIP 留档](E:/Works/arkret-org/convert/office/target/dist/first-phase-contrix-office-x86_64-pc-windows-msvc.zip)；[实际验收结果](E:/Works/arkret-org/convert/office/target/package-smoke/4ef37d31e77641b2a65eadbc96be31e3/result.json)中的 source-free start、browser、REST restart、WS ACK restart、receipt retry 均为 true |
| 依赖漏洞审计 | [JSON 结果](E:/Works/arkret-org/convert/office/target/review-audit-final.json)：0 vulnerabilities、0 warnings；基于本轮已获取公告库 |
| 格式、脚本、配置及报告 | 项目 `cargo fmt -- --check`、diff 空白检查通过；7 个 PowerShell、4 个 YAML 文件语法通过；当前证据链接与 27 个唯一编号已核对 |
| Docker | 仅完成 [Rust 镜像 manifest](E:/Works/arkret-org/convert/office/target/review-docker-image-manifest.log)、[Debian 镜像 manifest](E:/Works/arkret-org/convert/office/target/review-docker-runtime-manifest.log)及配置核对；未在 C 盘 builder 构建、未迁移 Docker 数据盘 |


## 当前实现与回归证据

以下链接指向修复后的工作区；具体执行结果以修复执行状态为准。后文原始审计保留基线问题、原因与验收要求，不再代表当前代码仍有同一缺陷。

| 编号 | 当前实现 | 关键回归 |
| --- | --- | --- |
| S01 | [真实路径边界校验](E:/Works/arkret-org/convert/office/crates/server/src/handlers/spa.rs:95) | [HTTP 路径穿越与 junction](E:/Works/arkret-org/convert/office/crates/server/src/handlers/spa.rs:113) |
| S02 | [WOPI 转义](E:/Works/arkret-org/convert/office/crates/server/src/handlers/wopi.rs:178)、[CSP](E:/Works/arkret-org/convert/office/crates/server/src/middleware/security_headers.rs:12) | [HTML 与脚本上下文](E:/Works/arkret-org/convert/office/crates/server/tests/security_regressions.rs:75) |
| S03 | [房间事务授权](E:/Works/arkret-org/convert/office/crates/server/src/handlers/rooms.rs:482)、[群组事务授权](E:/Works/arkret-org/convert/office/crates/server/src/handlers/groups.rs:404) | [拒绝无副作用与失败回滚](E:/Works/arkret-org/convert/office/crates/server/tests/security_regressions.rs:161) |
| S04 | [活动会话校验](E:/Works/arkret-org/convert/office/crates/server/src/services/wopi_sessions.rs:25) | [撤销及禁用](E:/Works/arkret-org/convert/office/crates/server/tests/security_regressions.rs:299) |
| S05 | [共享文件 ACL](E:/Works/arkret-org/convert/office/crates/server/src/handlers/access.rs:105) | [撤权后分享失效](E:/Works/arkret-org/convert/office/crates/server/tests/security_regressions.rs:424)、[batch/folder 入口](E:/Works/arkret-org/convert/office/crates/server/tests/security_regressions.rs:623) |
| S06 | [邀请详情授权](E:/Works/arkret-org/convert/office/crates/server/src/handlers/invitations.rs:230) | [不泄露其他用户凭证](E:/Works/arkret-org/convert/office/crates/server/tests/security_regressions.rs:257) |
| S07 | [长连接持续授权](E:/Works/arkret-org/convert/office/crates/server/src/handlers/ws.rs:18) | [只读 key 拒绝写连接](E:/Works/arkret-org/convert/office/crates/server/tests/messaging_regressions.rs:219)、[空闲撤权](E:/Works/arkret-org/convert/office/crates/server/tests/messaging_regressions.rs:775) |
| S08 | [原子重置密码](E:/Works/arkret-org/convert/office/crates/server/src/handlers/auth.rs:972) | [单次消费与事务回滚](E:/Works/arkret-org/convert/office/crates/server/tests/security_regressions.rs:1204) |
| D01 | [CAS 与原子提交](E:/Works/arkret-org/convert/office/crates/server/src/services/persistence.rs:795) | [并发保存及下载一致性](E:/Works/arkret-org/convert/office/crates/server/tests/persistence_regressions.rs:52) |
| D02 | [版本恢复](E:/Works/arkret-org/convert/office/crates/server/src/handlers/documents.rs:968) | [恢复失败保留旧对象](E:/Works/arkret-org/convert/office/crates/server/tests/persistence_regressions.rs:86) |
| D03 | [UUID 对象键](E:/Works/arkret-org/convert/office/crates/server/src/services/storage.rs:280) | [同名对象并发读删隔离](E:/Works/arkret-org/convert/office/crates/server/tests/persistence_regressions.rs:148) |
| D04 | [分块上传](E:/Works/arkret-org/convert/office/crates/server/src/handlers/upload.rs:253)、[幂等完成](E:/Works/arkret-org/convert/office/crates/server/src/handlers/upload.rs:606) | [并发完成只计费一次](E:/Works/arkret-org/convert/office/crates/server/tests/persistence_regressions.rs:207)、[失败可重试](E:/Works/arkret-org/convert/office/crates/server/tests/persistence_regressions.rs:250) |
| D05 | [事务独占锁](E:/Works/arkret-org/convert/office/crates/server/src/services/persistence.rs:1000)、[唯一索引](E:/Works/arkret-org/convert/office/migrations/025_atomic_persistence.sql:3) | [锁竞争及撤权](E:/Works/arkret-org/convert/office/crates/server/tests/persistence_regressions.rs:270) |
| D06 | [操作与 receipt 持久提交](E:/Works/arkret-org/convert/office/crates/server/src/services/persistence.rs:752) | [提交后 ACK](E:/Works/arkret-org/convert/office/crates/server/tests/messaging_regressions.rs:467)、[失败不 ACK](E:/Works/arkret-org/convert/office/crates/server/tests/messaging_regressions.rs:565)、[真实进程重启](E:/Works/arkret-org/convert/office/scripts/test-package.ps1:370) |
| D07 | [按文档隔离](E:/Works/arkret-org/convert/office/crates/collab/src/manager.rs:37)、[有界历史读取](E:/Works/arkret-org/convert/office/crates/server/src/services/collaboration.rs:100) | [跨文档隔离及会话回收](E:/Works/arkret-org/convert/office/crates/collab/src/manager.rs:178)、[裁剪后的旧操作](E:/Works/arkret-org/convert/office/crates/server/tests/messaging_regressions.rs:313) |
| F01 | [快照三方合并](E:/Works/arkret-org/convert/office/crates/frontend/src/pages/calc/snapshot_merge.rs:89) | [dirty 与历史保留](E:/Works/arkret-org/convert/office/crates/frontend/src/pages/calc/glory_calc.rs:3286)、[双浏览器](E:/Works/arkret-org/convert/office/e2e/tests/review.ui.spec.ts:46) |
| F02 | [共享预算与求值缓存](E:/Works/arkret-org/convert/office/crates/doc-engine/src/calc.rs:1385) | [重复依赖计数](E:/Works/arkret-org/convert/office/crates/doc-engine/src/calc.rs:3427)、[预算拒绝](E:/Works/arkret-org/convert/office/crates/doc-engine/src/calc.rs:3456) |
| F03 | [稀疏 XLSX 输出](E:/Works/arkret-org/convert/office/crates/export/src/xlsx.rs:337) | [远端单格及标准 XML](E:/Works/arkret-org/convert/office/crates/export/tests/review_regressions.rs:30) |
| F04 | [外部公式读取](E:/Works/arkret-org/convert/office/crates/export/src/xlsx.rs:928) | [无原生缓存的外部 fixture](E:/Works/arkret-org/convert/office/crates/export/tests/review_regressions.rs:12) |
| F05 | [转换前重算](E:/Works/arkret-org/convert/office/crates/server/src/handlers/convert.rs:77) | [真实 CSV 结果](E:/Works/arkret-org/convert/office/crates/server/src/handlers/convert.rs:899) |
| F06 | [保留原生字段](E:/Works/arkret-org/convert/office/crates/frontend/src/pages/calc/glory_calc.rs:199) | [未编辑类型保留](E:/Works/arkret-org/convert/office/crates/frontend/src/pages/calc/glory_calc.rs:3206)、[新格格式化及重载](E:/Works/arkret-org/convert/office/crates/frontend/src/pages/calc/glory_calc.rs:3451)、[旧格式损坏仍可计算](E:/Works/arkret-org/convert/office/crates/frontend/src/pages/calc/glory_calc.rs:3500) |
| F07 | [标准样式表](E:/Works/arkret-org/convert/office/crates/export/src/xlsx_styles.rs:11) | [标准样式 parts 与索引](E:/Works/arkret-org/convert/office/crates/export/tests/review_regressions.rs:30) |
| F08 | [外部页序、布局与元素](E:/Works/arkret-org/convert/office/crates/export/src/pptx_import.rs:991)、[标准导入入口](E:/Works/arkret-org/convert/office/crates/export/src/pptx.rs:1067) | [外部页序与告警](E:/Works/arkret-org/convert/office/crates/export/tests/review_regressions.rs:54)、[浏览器原件下载](E:/Works/arkret-org/convert/office/e2e/tests/review.ui.spec.ts:75) |
| O01 | [metadata 与路径前置检查](E:/Works/arkret-org/convert/office/scripts/workspace.ps1:2) | 正常环境 metadata、PowerShell 解析、调用者环境不变验证 |
| O02 | [完整容器构建](E:/Works/arkret-org/convert/office/docker/Dockerfile:1) | [D 盘隔离 builder 实际构建](E:/Works/arkret-org/convert/office/target/office-container-build-phase3.log)与[镜像运行验收](E:/Works/arkret-org/convert/office/target/package-smoke/72406505711549ce8171b27f08136c32/result.json) |
| O03 | [完整 ZIP 组装](E:/Works/arkret-org/convert/office/scripts/package.ps1:1) | [ZIP 和镜像共用无源码启动、资源哈希、浏览器登录保存与重启验收](E:/Works/arkret-org/convert/office/scripts/test-package.ps1:1) |
| O04 | [共享事务 purge](E:/Works/arkret-org/convert/office/crates/server/src/services/persistence.rs:1049) | [清理释放配额](E:/Works/arkret-org/convert/office/crates/server/tests/persistence_regressions.rs:343)、[保留对象去重计费](E:/Works/arkret-org/convert/office/crates/server/tests/persistence_regressions.rs:779) |

## 当前剩余边界与后续事项

- Docker Linux/amd64 的真实构建和镜像内浏览器验收已在 D 盘独立 builder 执行。继续构建时应核对实际数据根和挂载设备，现有 Docker Desktop 仍使用 C 盘数据盘。托管 CI 的容器门禁及 Intel macOS 包验收仍待实际运行。
- 第二轮外部 PPTX/XLSX 的实现与能力限制见上文；这些是当前模型可表达的兼容能力，不等同完整 Microsoft Office 兼容性认证或精确排版/播放承诺。
- Calc 可自动合并不同单元格的并发修改；同格不同输入、并发结构或非单元格元数据变化保守提示冲突。离线 CRDT 合并不在当前协议中，历史窗口外操作需要重新同步。
- 历史保留、对象登记与分页回收已有第二轮实现，仍依赖后台最终收敛；保留例外、墓碑增长、未知旧对象及数据库分叉共享键的边界见上文。本轮取消、慢写与服务端故障测试不等同真实断电实验。
- 正常超限上传在返回 413 前按帧排空，累计预算为 multipart 上限加 1 MiB、总计 2 秒，不按声明长度分配内存；成功 multipart 也需要完成 HTTP framing。超出排空预算或超时会关闭连接，不能保证无限输入、认证/准入前拒绝或持续恶意发送者总能收到完整 JSON。
- WOPI 临时 capability 存于进程内，重启后需重新交换。空闲长连接每 2 秒复核凭证，消息处理和推送前另行复核。合法邀请 token 仍是 bearer capability，未新增邮箱所有权验证流程。
- 样式 CSP 保留 inline 以兼容现有控件；脚本使用 hash/nonce 与 WASM 所需策略。Unix symlink 与本地扫描/删除专项已在本次 Linux Rust 套件真实运行通过；Windows junction 已有动态专项。macOS 尚未验收。
- 平台结果分别按 Windows 本机与 Ubuntu 24.04 x86_64 / WSL2 列出，不能推广到全部 Linux 发行版、架构或 macOS。真实 Garage S3 兼容服务及性能工具已有第二轮工作；不将准备好的脚本或旧版本性能结果当作当前版本验收。未连接 AWS 生产桶，未给出硬件无关 SLO、全面 WCAG 或全设备支持结论。
- 管理员备份仅生成 SQLite 快照，不复制或固定 blob；GC 可能已删除旧快照引用的对象，DB-only 快照不能保证文件恢复。部署需外部协调备份/恢复数据库及匹配对象键，并处理并发写入与 GC。发布包重启验收复用实时 DB/storage，不代表全量灾难恢复已验证。
- 依赖漏洞审计为已获取公告库下的扫描结果，不代表全面供应链安全证明。Diesel 临时官方固定提交补丁应在兼容发布版可用后移除；Glory 已固定最小兼容官方修复提交，未直接修改第三方源码，新的 feature/MSRV/构建结果由第二轮验收确认。

## 原始评审范围与方法（f18c115 基线）

由主 agent 和三个独立 agents 完成：

| 评审角色 | 主要范围 | 关注的正交问题 |
| --- | --- | --- |
| 安全 agent | 认证、权限、分享、WOPI、API key、静态文件 | 谁可访问什么，凭证撤销是否有效，输入是否跨越信任边界 |
| 并发 agent | 数据库、文件存储、分块上传、锁、协作 | 操作是否原子、幂等，取消与失败后能否恢复 |
| 前端与引擎 agent | 编辑器、计算、Office 导入导出 | 数据语义是否保真，远程状态是否覆盖本地编辑，算法是否随输入合理增长 |
| 主 agent | 构建、交付、测试、运维及交叉核验 | 能否复现构建、交付是否完整、错误是否可见、清理是否保持不变量 |

覆盖六个 Rust crates、迁移、配置、CI、Docker、静态资源与现有测试。静态统计为 145 个 Rust 文件、约 66,986 行、402 个测试标记；这些不是执行通过数。三方源依赖只在解释构建故障及请求路径处理时定点阅读，未对其全面审计。

除上述四组职责外，报告分别检查安全性、并发、一致性、计算复杂度、吞吐与隔离、内存生命周期、功能正确性、格式兼容性、故障恢复、可测试性、交付可复现性、可观测性和可维护性。一个问题可影响多个维度，但按根因只计一次。

优先级含义：

- **P1**：应优先修复；存在越权、数据丢失、明显资源耗尽或交付阻断路径。
- **P2**：后续计划必须处理；包括条件性竞态、兼容性损坏、长期退化和工程流程问题。
- 未发现可据本次证据判为 P0 的无条件全局事故；未做吞吐、延迟、内存或渗透测试，不给出性能分数或安全通过结论。

## 问题总览

| 编号 | 优先级 | 主要问题 | 建议批次 |
| --- | --- | --- | --- |
| S01 | P1 | 公开静态文件回退可逃逸静态根目录 | A |
| S02 | P1 | WOPI host 页面反射型同源 XSS | A |
| S03 | P1 | 删除他人房间或群组会先清空成员 | A |
| S04 | P1 | WOPI 绕过会话撤销和账户禁用 | A |
| S05 | P1 | 房间 ACL 未覆盖核心文档与分享接口 | A |
| S06 | P1 | 邀请详情泄露可消费的邀请凭证 | A |
| S07 | P1 | 只读 API key 可通过 WebSocket 写入 | A |
| S08 | P2 | 密码重置凭证非原子消费 | B |
| D01 | P1 | 并发保存丢更新并使内容与文件记录分叉 | B |
| D02 | P1 | 恢复版本吞掉 SQL 失败并删除旧 blob | B |
| D03 | P1 | 同名对象先检查后写入导致并发覆盖 | B |
| D04 | P1 | 分块上传覆盖、重复完成及失败后不可重试 | B |
| D05 | P1 | 同一文件可同时取得多把独占锁 | B |
| D06 | P1 | 协作快照、版本、日志与客户端恢复不一致 | C |
| D07 | P2 | 协作历史无限增长且跨文档共用写锁 | D |
| F01 | P1 | Calc 远程消息清除本地未保存状态 | C |
| F02 | P1 | 公式依赖重复展开导致指数级求值 | D |
| F03 | P1 | 稀疏 XLSX 导出扫描整个坐标矩形 | D |
| F04 | P2 | 普通外部 XLSX 导入丢失公式 | E |
| F05 | P2 | Calc 保存后 CSV 公式结果为空 | E |
| F06 | P2 | 修改其他格也会改变未编辑格的数据类型 | E |
| F07 | P2 | XLSX 普通单元格样式未进入标准 OOXML | E |
| F08 | P2 | 外部 PPTX 页序错误且元素静默降级 | E |
| O01 | P2 | 构建与测试配置违反当前路径约束 | 0 |
| O02 | P1 | Docker 工具链与项目不兼容且产物路径不匹配 | 0 |
| O03 | P2 | 发布包与容器缺少可运行的完整前端资产 | 0 |
| O04 | P2 | 回收站自动清理不释放用户配额 | B |

批次 0 是恢复验证与交付的前置条件；A 至 E 的验收与依赖见后文。

## 安全性与授权

### S01 公开静态文件读取缺少目录边界校验

**P1，源码路径确认，文件系统语义已验证，真实 HTTP 待复现。** 位置：[spa.rs:5](E:/Works/arkret-org/convert/office/crates/server/src/handlers/spa.rs:5)、[mod.rs:91](E:/Works/arkret-org/convert/office/crates/server/src/routes/mod.rs:91)。

spa_fallback 把 static_root 与 req.uri().path() 直接拼接后读取。原始路径 /../README.md 对应 static/../README.md，已通过本地非敏感路径 probe 确认会逃逸 static 根目录。锁定版本 Hyper 1.11.0、http 1.5.0 和 Salvo 0.89.3 的源码链保留原始 URI 中的 ..；项目也没有后续路径重写。

触发前提是原始路径到达应用且服务账户可读取目标。公开 fallback 没有身份验证，潜在读取范围包括已知位置的配置、数据库与备份。浏览器和代理可能提前归一化，因此不能用普通浏览器请求失败来证明应用安全，也不能把本次路径 probe 描述为 HTTP 利用成功。

**改进：** 使用安全静态文件组件，或拒绝非普通路径组件，并对真实根目录与真实候选路径校验包含关系；覆盖 symlink/junction。缺失资产返回 404，不能任意落到 index.html。

**验收：** 用 raw TCP 或 curl --path-as-is 请求专门放置的目录外非敏感标记文件，直连应用和经过代理均不可读；覆盖 ..、Windows 路径分隔符、编码变体及链接逃逸。

### S02 WOPI 页面把 URL 参数直接插入 HTML 和脚本

**P1，静态确认。** 位置：[wopi.rs:584](E:/Works/arkret-org/convert/office/crates/server/src/handlers/wopi.rs:584)、[wopi.rs:681](E:/Works/arkret-org/convert/office/crates/server/src/handlers/wopi.rs:681)、[wopi.rs:686](E:/Works/arkret-org/convert/office/crates/server/src/handlers/wopi.rs:686)、[security_headers.rs:20](E:/Works/arkret-org/convert/office/crates/server/src/middleware/security_headers.rs:20)。

公开 host 页将 wopisrc 插入 HTML，并用 Rust Debug 格式插入内联 script。HTML 未转义；Debug 字符串也不能阻止 HTML 解析器处理脚本结束标签。CSP 允许 unsafe-inline，不能阻断该路径。Token 交换失败也仍会返回页面。诱导登录用户打开构造链接即可在 Office 同源执行脚本，影响浏览器可访问凭证和 API 操作。

**改进：** 展示字段使用 textContent；脚本参数改为安全数据容器或外部脚本，并处理 HTML 解析上下文的特殊字符。逐步收紧 CSP。

**验收：** 标签、事件属性、脚本结束片段、引号与中文只能按数据展示，不出现执行标记；分别测试 HTML 与脚本上下文。

### S03 所有者验证发生在删除成员之后

**P1，SQL 顺序已在内存 SQLite 复现。** 位置：[rooms.rs:483](E:/Works/arkret-org/convert/office/crates/server/src/handlers/rooms.rs:483)、[groups.rs:415](E:/Works/arkret-org/convert/office/crates/server/src/handlers/groups.rs:415)。

两个 handler 都先按对象 ID 删除全部成员，再对父对象的 DELETE 添加 owner/created_by 条件。任意登录用户知道他人对象 ID 后，虽收到 404，成员已被删除。本次 SQL probe 的结果是 members_deleted=1、rooms_deleted=0、members_left=0。

**改进：** 事务内先验证所有者，再执行整组删除；如果使用级联删除，先确保实际连接启用外键且所有级联关系符合产品语义。

**验收：** 陌生人、普通成员的失败请求必须完全不改变成员；所有者成功删除；在第二条 SQL 注入失败时整体回滚。

### S04 WOPI 未复用主认证的撤销检查

**P1，静态确认。** 位置：[wopi.rs:42](E:/Works/arkret-org/convert/office/crates/server/src/handlers/wopi.rs:42)、[wopi.rs:86](E:/Works/arkret-org/convert/office/crates/server/src/handlers/wopi.rs:86)、[wopi_sessions.rs:32](E:/Works/arkret-org/convert/office/crates/server/src/services/wopi_sessions.rs:32)；对照 [auth.rs:216](E:/Works/arkret-org/convert/office/crates/server/src/middleware/auth.rs:216)。

WOPI JWT 交换只校验签名和到期时间，未检查 sessions.is_active 与当前账户禁用状态。已被主 API 撤销但尚未到 exp 的 JWT 仍可换取 WOPI token；已经签发的 WOPI token 也只按文件绑定和到期时间校验，继续访问本人文件。

**改进：** 提取共享凭证验证服务；WOPI token 绑定源 session 或撤销版本，并在读写时检查当前撤权状态。

**验收：** 强制下线、改密和禁用账号后，旧 JWT 不能交换，既有 WOPI token 不能继续读写；保留单文件绑定测试。

### S05 房间 ACL 与核心文档接口采用不同授权规则

**P1，静态确认。** 位置：[access.rs:69](E:/Works/arkret-org/convert/office/crates/server/src/handlers/access.rs:69)、[documents.rs:340](E:/Works/arkret-org/convert/office/crates/server/src/handlers/documents.rs:340)、[documents.rs:458](E:/Works/arkret-org/convert/office/crates/server/src/handlers/documents.rs:458)、[shares.rs:65](E:/Works/arkret-org/convert/office/crates/server/src/handlers/shares.rs:65)、[wopi.rs:194](E:/Works/arkret-org/convert/office/crates/server/src/handlers/wopi.rs:194)。

共享 helper 规定房间文件按当前成员权限授权，但文档读取、更新、删除、内容、导出、历史和恢复等路径仅按 file.owner_id 筛选。创建分享和 WOPI 也存在同类判断。已移出房间的文档创建者仍可操作原房间文档并新建公开分享；其他合法 editor 又因非创建者无法调用核心文档接口。

**改进：** 文档、版本、WOPI、分享统一调用 file/document ACL，明确各动作的最小权限以及既有公开链接的撤销策略。租户策略也应在同一授权服务集中执行。

**验收：** owner、editor、viewer、已移除创建者、陌生人，分别测试读取、保存、删除、导出、历史、分享和 WOPI；移除成员后不得通过其他协议继续访问。

### S06 邀请详情泄露 bearer 凭证

**P1，静态确认，提权需要明确条件。** 位置：[invitations.rs:20](E:/Works/arkret-org/convert/office/crates/server/src/handlers/invitations.rs:20)、[invitations.rs:230](E:/Works/arkret-org/convert/office/crates/server/src/handlers/invitations.rs:230)、[invitations.rs:389](E:/Works/arkret-org/convert/office/crates/server/src/handlers/invitations.rs:389)、[invitations.rs:524](E:/Works/arkret-org/convert/office/crates/server/src/handlers/invitations.rs:524)。

get_invitation 只要求登录，查询只按邀请 ID，未使用调用者 ID 授权。返回内容包含邮箱、role 和明文 token；公开接受接口可消费该 token。当邀请有效、未使用、目标邮箱尚未注册时，无关登录用户知道邀请 ID 即可代为接受；若是管理员邀请，可创建管理员账号。现有邮箱分支不替换原账号密码，不能据此声称可接管已有账号。

**改进：** 详情仅允许邀请创建者或管理员访问，普通详情不返回 token；接受邀请绑定可信邮箱验证。

**验收：** 无关账号不能读取 token/email，也不能消费管理员邀请；覆盖已注册邮箱与未注册邮箱两条路径。

### S07 API key 的 HTTP 只读权限未延伸到 WebSocket 消息

**P1，静态确认。** 位置：[auth.rs:103](E:/Works/arkret-org/convert/office/crates/server/src/middleware/auth.rs:103)、[auth.rs:291](E:/Works/arkret-org/convert/office/crates/server/src/middleware/auth.rs:291)、[ws.rs:70](E:/Works/arkret-org/convert/office/crates/server/src/handlers/ws.rs:70)、[ws.rs:176](E:/Works/arkret-org/convert/office/crates/server/src/handlers/ws.rs:176)。

握手 GET 按 read/ws:read 放行后，认证上下文未保留 key scope。WS 只检查用户本身的 editor 资格，随后接受 Operation 并广播、写日志。持有 editor 用户只读 key 的非浏览器客户端可借此执行写操作。

**改进：** 保留 credential/capability 上下文并按消息授权；如果产品不支持只读协作，升级连接就必须要求写权限。

**验收：** read/ws:read key 的 Operation 不得改变版本、日志或广播；write/ws:write 和正常 JWT 编辑路径分别验收。

### S08 密码重置凭证不是原子消费

**P2，静态确认。** 位置：[auth.rs:953](E:/Works/arkret-org/convert/office/crates/server/src/handlers/auth.rs:953)、[auth.rs:1013](E:/Works/arkret-org/convert/office/crates/server/src/handlers/auth.rs:1013)。

先读取 used=false，再异步哈希、改密、撤销会话，最后独立标记 used=true，且标记错误被忽略。有效 token 的并发消费可两次成功；标记失败会留下可重复改密的已用 token。

**改进：** 哈希可在事务外计算，条件消费 token、更新密码与撤销会话必须在同一事务中完成，并校验条件更新恰好影响一行。

**验收：** 同 token 并发请求仅一个成功；任一步失败都不能留下密码已变而 token 未消费的状态。

## 并发与数据一致性

### D01 文档保存缺少版本比较和整体事务

**P1，静态并发序列确认。** 位置：[documents.rs:454](E:/Works/arkret-org/convert/office/crates/server/src/handlers/documents.rs:454)、[documents.rs:512](E:/Works/arkret-org/convert/office/crates/server/src/handlers/documents.rs:512)、[documents.rs:543](E:/Works/arkret-org/convert/office/crates/server/src/handlers/documents.rs:543)。

A/B 同时读到版本 N，均计算 N+1，并只按文档 ID 更新，因此都返回成功而丢失其中一份内容。documents、files、users 分开提交；A 更新 documents、B 更新 documents、B 更新 files、A 更新 files 的合法顺序会使 API 内容是 B、下载 blob 是 A。配额也可能重复累计基于同一旧大小计算的差值。

**改进：** 请求携带 expected_version，使用条件更新实现 CAS，冲突返回 409；版本快照、文档、文件元数据与配额事务提交。Blob 使用不可变独立键，失败后回收未引用对象。

**验收：** 用 barrier 固定两请求读取相同版本，要求仅一个提交；API、下载、历史、配额始终一致，失败方能保留并重试本地编辑。

### D02 恢复版本失败仍报告成功并清理旧文件

**P1，静态失败路径确认。** 位置：[documents.rs:1156](E:/Works/arkret-org/convert/office/crates/server/src/handlers/documents.rs:1156)、[documents.rs:1182](E:/Works/arkret-org/convert/office/crates/server/src/handlers/documents.rs:1182)；普通保存同类问题见 [documents.rs:553](E:/Works/arkret-org/convert/office/crates/server/src/handlers/documents.rs:553)。

恢复路径对所有必要 SQL 使用 let _ 忽略结果，再删除旧 blob 并返回 200。即使文件指针更新失败，旧指针对应的 blob 仍可能被删除。普通保存也只检查 documents UPDATE，忽略后续 files、quota 更新。

**改进：** 必要写入失败必须传播并回滚；只有新指针成功提交后才清理旧对象。此项可与 D01 共用提交服务，但要单独覆盖失败语义。

**验收：** 通过触发器或可控故障分别令 documents/files/quota 写失败；必须返回错误、旧文件仍可下载、元数据和配额不变。

### D03 同名对象存在检查与写入之间有竞态

**P1，静态确认。** 位置：[storage.rs:48](E:/Works/arkret-org/convert/office/crates/server/src/services/storage.rs:48)、[storage.rs:84](E:/Works/arkret-org/convert/office/crates/server/src/services/storage.rs:84)、[storage.rs:291](E:/Works/arkret-org/convert/office/crates/server/src/services/storage.rs:291)。

unique_relative_path 先检查 owner/name 不存在，再执行允许覆盖的本地 write 或 S3 PUT。两个同名并发上传可得到相同键，使两份文件记录引用同一个对象；删除其中一份还会破坏另一份。

**改进：** 对象键直接使用完整 UUID 或内容寻址；需要独占创建时采用原子 create_new/条件写入，不以 exists/HEAD 作为唯一性保证。

**验收：** 并发保存相同 owner/name、不同 payload，所有返回键不同，内容可独立读取、删除；覆盖本地和 S3 适配器。

### D04 分块上传缺少幂等与可恢复的完成状态机

**P1，静态确认。** 位置：[upload.rs:365](E:/Works/arkret-org/convert/office/crates/server/src/handlers/upload.rs:365)、[upload.rs:447](E:/Works/arkret-org/convert/office/crates/server/src/handlers/upload.rs:447)、[upload.rs:552](E:/Works/arkret-org/convert/office/crates/server/src/handlers/upload.rs:552)。

并发 chunk 读取相同计数，各写 k+1 路径，造成覆盖与累计值错误。complete 未原子抢占 pending，可能重复创建和计费。更严重的是完成时先删除 chunks，之后才插入文件记录；数据库写失败会删除最终 blob，使仍 pending 的会话丢失重试材料。

**改进：** chunk index/hash 与唯一约束实现幂等；pending→processing 条件更新抢占完成任务；文件、配额、completed 状态事务提交后再回收 chunks，并提供恢复任务。

**验收：** 并发不同块、重复同块、并发完成、完成提交失败和进程中断，均不覆盖、不重复计费，失败后能够重试。

### D05 独占锁没有文件级唯一性

**P1，schema 层已在内存 SQLite 复现。** 位置：[file_locks.rs:84](E:/Works/arkret-org/convert/office/crates/server/src/handlers/file_locks.rs:84)、[009_wopi_locking_presence.sql:2](E:/Works/arkret-org/convert/office/migrations/009_wopi_locking_presence.sql:2)、[wopi.rs:1040](E:/Works/arkret-org/convert/office/crates/server/src/handlers/wopi.rs:1040)。

先 SELECT 后 INSERT，file_id 只有普通索引。两位编辑者均看到无锁后可插入不同 token，数据库接受同一文件两条独占锁。本次 probe 确认 same_file_rows=2。普通文档 PUT 也没有检查 file_locks，锁协议未覆盖全部写入口。

**改进：** 为 file_id 建立唯一约束，获取、续期、过期接管使用原子 SQL；统一 REST/WOPI 写入口的锁语义。

**验收：** 两用户同时获取锁只有一人成功；数据库始终一条有效锁；覆盖并发过期接管及跨协议写入。

### D06 协作恢复没有一致的快照与日志边界

**P1，跨层静态确认。** 位置：[ws.rs:140](E:/Works/arkret-org/convert/office/crates/server/src/handlers/ws.rs:140)、[ws.rs:180](E:/Works/arkret-org/convert/office/crates/server/src/handlers/ws.rs:180)、[ws.rs:271](E:/Works/arkret-org/convert/office/crates/server/src/handlers/ws.rs:271)、[session.rs:198](E:/Works/arkret-org/convert/office/crates/collab/src/session.rs:198)、[document_editor.rs:120](E:/Works/arkret-org/convert/office/crates/frontend/src/pages/document_editor.rs:120)。

服务端快照版本来自内存 session，内容来自另一次数据库读取；协作操作只进入内存历史、广播和 operation_log，不同步更新 documents。全仓未找到 operation_log 重放路径。自动保存尚未发生时，新加入者会拿到较新版本与旧内容；广播 Lagged 后同样无法靠该快照可靠追上。ACK 又可能先于日志写入成功。

客户端还有独立缺口：共享 EditorSession 没有处理 DocumentState，Writer/Slides/Whiteboard 即使收到重连快照，也不会更新内容。它与服务端边界问题需两端分别修复。

**改进：** 定义权威文档状态和版本；快照绑定明确日志水位，只重放之后的增量，按操作 ID/版本去重。明确 ACK 的持久化语义。客户端 dirty 时保留待合并修改，否则应用快照。

**验收：** 暂停 HTTP 自动保存后加入第二客户端；覆盖断线、超出广播缓冲、快照期间新操作、日志失败、进程重启。客户端内容与版本最终收敛，已确认编辑不丢失。

### D07 全局写锁与无限协作历史导致长期退化

**P2，复杂度和生命周期静态确认。** 位置：[session.rs:183](E:/Works/arkret-org/convert/office/crates/collab/src/session.rs:183)、[manager.rs:74](E:/Works/arkret-org/convert/office/crates/collab/src/manager.rs:74)、[manager.rs:113](E:/Works/arkret-org/convert/office/crates/collab/src/manager.rs:113)。

每次操作线性扫描全部历史去重，再保存操作及 undo 副本；N 次操作的去重累计为 O(N²)。整文档 Replace 还使历史内存随编辑次数乘文档大小增长。各文档共享 manager 写锁，一个热点会阻塞其他文档及游标。cleanup_empty_sessions 有定义但未找到生产调用，离开后的会话不会自动回收。

**改进：** 按文档锁或 actor 隔离；HashSet 去重；快照压缩和有界 undo；空闲 TTL 回收，裁剪须与 D06 的持久恢复共同设计。

**验收：** 长会话 10 万次操作时测历史大小、RSS 和操作成本，同时观察其他文档 p95/p99；所有人离开后会话和内存按策略回落。具体延迟和容量需实测后才能承诺。

## 前端正确性与文档引擎

### F01 Calc 收到远程操作会丢掉本地编辑状态

**P1，静态确认。** 位置：[glory_calc.rs:41](E:/Works/arkret-org/convert/office/crates/frontend/src/pages/calc/glory_calc.rs:41)、[glory_calc.rs:892](E:/Works/arkret-org/convert/office/crates/frontend/src/pages/calc/glory_calc.rs:892)、[glory_calc.rs:949](E:/Works/arkret-org/convert/office/crates/frontend/src/pages/calc/glory_calc.rs:949)。

Operation 无条件调用 hydrate，后者替换模型、清空 undo/redo 并设 dirty=false。远程整本 Replace 直接覆盖本地未保存内容；单格 patch 虽先合并本地 serialize 结果，却也将未持久化修改标成 clean，自动保存随后跳过。

**改进：** 将服务器基线、本地 pending edits 与显示模型分开；合并远程 patch 后保留本地 dirty 和 undo；整本替换遇到本地修改时进入冲突恢复流程。

**验收：** A 编辑 A1 尚未自动保存时 B 保存 B1，A 的 A1 最终仍持久化；再覆盖同格、多格 Replace、保存中与断线恢复。

### F02 无环公式也可指数级重复求值

**P1，算法复杂度静态确认，未运行大输入。** 位置：[f18c115 calc.rs:1338 留档](E:/Works/arkret-org/convert/office/target/review-baseline-f18c115/frontend-calc.rs:1338)、[glory_calc.rs:2264](E:/Works/arkret-org/convert/office/crates/frontend/src/pages/calc/glory_calc.rs:2264)、[calc.rs:1708](E:/Works/arkret-org/convert/office/crates/doc-engine/src/calc.rs:1708)。

前端引用公式时重新 lex/parse/evaluate，引擎 resolve_cell 也递归重算，没有本轮结果缓存。A1=1、A2=A1+A1、A3=A2+A2，递推到 A35，只用 35 格便形成指数级求值树。循环检测不会限制这种无环重复展开；前端同步渲染会占住主线程。

**改进：** 每个 revision 缓存 AST、依赖图与求值结果；按拓扑顺序每格计算一次，并限制深度、范围和执行预算；大计算转 Worker。

**验收：** 对重复引用链和菱形依赖用操作计数验证接近 O(V+E)，同时覆盖循环、深链和取消。不能以结果正确代替复杂度验收。

### F03 稀疏 XLSX 导出扫描最大矩形

**P1，复杂度静态确认。** 位置：[xlsx.rs:377](E:/Works/arkret-org/convert/office/crates/export/src/xlsx.rs:377)、[convert.rs:25](E:/Works/arkret-org/convert/office/crates/server/src/handlers/convert.rs:25)。

非空单元格使用稀疏存储，导出却扫描 0..max_row × 0..max_col。只在 XFD1048576 放一个单元格，也要执行约 171.8 亿次坐标查找。两笔此类导出可占满两个 EXPORT_WORKERS；blocking 任务不会随调用取消立即停止。该次数来自循环边界计算，不是实测耗时。

**改进：** 对实际 cells 排序并按行分组，复杂度随非空格数增长；校验坐标及输出预算。CSV 稠密输出另外设置大小估算和拒绝/分块策略。

**验收：** 同样数量的格分别位于 A1 和远端坐标，XLSX 访问次数不应随空白矩形面积增长；取消与预算超限能释放转换容量。

### F04 外部 XLSX 公式被导入成常量

**P2，静态确认。** 位置：[xlsx.rs:710](E:/Works/arkret-org/convert/office/crates/export/src/xlsx.rs:710)、[xlsx.rs:750](E:/Works/arkret-org/convert/office/crates/export/src/xlsx.rs:750)、[calc.rs:223](E:/Works/arkret-org/convert/office/crates/doc-engine/src/calc.rs:223)。

普通导入只读 worksheet_range 的值并 set_cell，未读取公式。带缓存结果的公式变成常量，无缓存公式还可能丢失。本产品自身往返可能直接命中 customXml 原生模型，掩盖标准解析路径的错误。

**改进：** 同时导入公式与缓存值，统一公式是否带等号的约定。

**验收：** 使用无 Contrix 自定义 part 的外部 fixture；导入 A2=A1+1 后修改 A1，A2 应更新，再导出仍有公式。

### F05 保存后的公式结果在 CSV 中为空

**P2，跨层静态确认。** 位置：[glory_calc.rs:100](E:/Works/arkret-org/convert/office/crates/frontend/src/pages/calc/glory_calc.rs:100)、[convert.rs:93](E:/Works/arkret-org/convert/office/crates/server/src/handlers/convert.rs:93)、[csv.rs:84](E:/Works/arkret-org/convert/office/crates/export/src/csv.rs:84)。

serialize 把公式缓存统一写为 Empty；CSV 只读 cell.value，转换前没有重算。因此前端显示正确计算结果，保存再导出时公式格却为空。

**改进：** 导出通过有预算的统一引擎重算，定义缓存新鲜度；避免前后端各自拥有不同求值语义。

**验收：** A1=1、A2=2、A3=SUM(A1:A2)，经过真实编辑、保存、CSV 下载后 A3 为 3；覆盖文本、错误结果与依赖引用。

### F06 全量字符串重编码改变未编辑格类型

**P2，静态确认。** 位置：[calc.rs:776](E:/Works/arkret-org/convert/office/crates/frontend/src/pages/calc.rs:776)、[calc.rs:898](E:/Works/arkret-org/convert/office/crates/frontend/src/pages/calc.rs:898)、[glory_calc.rs:100](E:/Works/arkret-org/convert/office/crates/frontend/src/pages/calc/glory_calc.rs:100)。

hydrate 把类型化值变为显示字符串，serialize 全量重建时只识别公式、空、数值和文本。修改任意其他格后，Boolean/Date/Error 会变 Text，文本 00123 会变 Number 123。

**改进：** 编辑状态保存类型化 Cell，只对实际输入的格执行解析；未编辑格保留完整原字段。

**验收：** 布尔、日期、错误、带前导零文本和文本 1e3，在修改另一格后应逐字段不变。

### F07 标准 XLSX 输出缺失普通单元格样式

**P2，静态确认。** 位置：[xlsx.rs:237](E:/Works/arkret-org/convert/office/crates/export/src/xlsx.rs:237)、[xlsx.rs:399](E:/Works/arkret-org/convert/office/crates/export/src/xlsx.rs:399)。

styles.xml 只有默认 cellXf，普通单元格没有 s 样式索引。粗体、颜色、边框、对齐和数字格式不会按标准样式呈现。条件格式 dxf 是不同机制；原生模型缓存可保留样式，也不能证明 Excel 可见格式正确。

**改进：** 对 CellFormat 去重生成 fonts/fills/borders/numFmts/cellXfs，并为单元格写样式索引。

**验收：** 直接检查 OOXML 及独立阅读器结果，覆盖数字格式、颜色、边框、对齐，不能只做本产品导出再导入。

### F08 外部 PPTX 导入未按真实页序恢复且静默丢元素

**P2，页序错误静态确认，元素保真有已知实现边界。** 位置：[pptx.rs:971](E:/Works/arkret-org/convert/office/crates/export/src/pptx.rs:971)、[pptx.rs:991](E:/Works/arkret-org/convert/office/crates/export/src/pptx.rs:991)。

导入按 slide 文件名排序，未按 presentation 的 slide IDs 与 relationships 恢复顺序；外部 PowerPoint 重排页面后文件名不必变化，结果页序错误。普通解析随后仅抽文本并放入固定文本框，图片、图表、形状、坐标和动画等降级。既有历史报告已承认复杂外部格式边界，因此该部分重点是防止静默损失和区分兼容级别。

**改进：** 先正确解析 presentation relationships 的页序；逐步恢复元素与布局；对暂不支持部分返回明确警告并保留原件。

**验收：** 独立外部 fixture 的页序与文件名顺序不同，包含图片、图表、形状；验证顺序正确，降级清单明确、原件可恢复。

## 构建与运维

### O01 仓库构建配置仍强制使用被禁止的缓存根目录

**P2，配置确认，本轮因此未执行构建。** 位置：[config.toml:1](E:/Works/arkret-org/convert/office/.cargo/config.toml:1)、[check.ps1:4](E:/Works/arkret-org/convert/office/scripts/check.ps1:4)、[dev.ps1:4](E:/Works/arkret-org/convert/office/scripts/dev.ps1:4)、[build-frontend.ps1:4](E:/Works/arkret-org/convert/office/scripts/build-frontend.ps1:4)、[action.yml:14](E:/Works/arkret-org/convert/office/.github/actions/setup-glory/action.yml:14)、[playwright.config.ts:4](E:/Works/arkret-org/convert/office/e2e/playwright.config.ts:4)。

Cargo metadata 返回禁止的 target_directory；脚本还改写 CARGO_HOME、target、TEMP/TMP 等，并创建目录。开发脚本对其他路径的限制也把机器布局写入项目行为，违反本次 AGENTS 指令，且降低构建可移植性。

**改进：** 后续单独修复配置以遵循调用者环境和项目正常设置，移除强制全局缓存重定向。产物位置只使用实际 workspace 的 metadata 返回值，不增加替代全局根目录。本次报告未自动修改这些设置。

**验收：** 在干净环境和已有调用者环境中检查 metadata 及脚本行为；不访问或创建被禁止目录，不改写调用者 Cargo/TEMP 配置。Docker 验证另须先确认 builder 数据位于 D 盘或远端，Cargo target 位置不能替代该检查。

### O02 Docker 构建工具链及产物位置与当前项目不匹配

**P1，静态确认，未运行 Docker。** 位置：[Dockerfile:2](E:/Works/arkret-org/convert/office/docker/Dockerfile:2)、[Dockerfile:11](E:/Works/arkret-org/convert/office/docker/Dockerfile:11)、[Cargo.toml:2](E:/Works/arkret-org/convert/office/Cargo.toml:2)、[Cargo.toml:15](E:/Works/arkret-org/convert/office/Cargo.toml:15)、[f18c115 config.toml:2 留档](E:/Works/arkret-org/convert/office/target/review-baseline-f18c115/cargo-config.toml:2)。

Docker 使用 Rust/Cargo 1.83，而项目使用 edition 2024 和 resolver 3；frontend 还明确声明 rust-version 1.96。现有构建阶段不能按当前 manifest 完成。即使更新工具链，被复制进容器的 Cargo target 配置也与后续固定 COPY /app/target/release/... 的假设不一致，需一起修复。

**改进：** 选择满足当前项目工具链要求的可复现构建镜像；统一容器内实际产物路径与打包路径；构建使用 locked。版本选择应以修复时验证结果为准。

**验收：** 在满足 builder 数据位置约束后，从干净上下文构建，检查真实产物位置并启动镜像；版本与目录失败必须在构建检查中可见。

### O03 发布资产未组成可运行的完整产品

**P2，打包路径静态确认。** 位置：[release.yml:33](E:/Works/arkret-org/convert/office/.github/workflows/release.yml:33)、[release.yml:48](E:/Works/arkret-org/convert/office/.github/workflows/release.yml:48)、[Dockerfile:14](E:/Works/arkret-org/convert/office/docker/Dockerfile:14)、[bootstrap.js:1](E:/Works/arkret-org/convert/office/static/bootstrap.js:1)、[main.rs:69](E:/Works/arkret-org/convert/office/crates/server/src/main.rs:69)、[main.rs:234](E:/Works/arkret-org/convert/office/crates/server/src/main.rs:234)。

Release 只发布服务器二进制，运行还需要配置、迁移和前端资源；没有组装完整分发包。Docker 只复制 static 源目录，没有调用前端构建或拷入生成的 wasm/js 与样式。bootstrap 引用 /assets/contrix_frontend.js，源 static 本身不包含生成结果。CI 的 frontend artifact 并未接入 release 打包。

**改进：** 定义并组装版本一致的服务器、配置示例、迁移、前端生成资产及启动说明；容器和 release 复用同一资产契约。

**验收：** 将发行包解压到无源码目录启动，实际浏览器能加载 JS/WASM/CSS、登录并保存文档；镜像执行相同验收。缺失资产应明确 404，而不是返回 HTML 伪装成功。

### O04 回收站自动清理未释放配额

**P2，静态确认。** 位置：[main.rs:304](E:/Works/arkret-org/convert/office/crates/server/src/main.rs:304)、[main.rs:323](E:/Works/arkret-org/convert/office/crates/server/src/main.rs:323)；对照手动清理 [trash.rs:318](E:/Works/arkret-org/convert/office/crates/server/src/handlers/trash.rs:318)。

自动任务删除文件和记录，但没有像手动清空一样扣减 users.storage_used。长期使用后，已清理数据仍占逻辑配额。它还忽略存储删除和 SQL 错误，可能在对象删除失败时移除记录，失去后续回收线索。

**改进：** 自动与手动清理使用共享服务，按选定对象事务调整配额；对象删除采用可重试记录并暴露失败数量。关联文档、版本和其他子记录也需明确清理策略。

**验收：** 过期清理后的配额与保留文件大小一致；对象存储失败可重试，不能无痕遗留对象；重复执行不重复扣减。

## 原始修复规划与完成条件

| 批次 | 目标与范围 | 依赖与规模判断 | 完成条件 |
| --- | --- | --- | --- |
| 0 | 修复 O01–O03，恢复合法验证与完整交付 | 独立前置工作，中等 | 路径合规；workspace/wasm 检查可执行；干净包与镜像真实可运行 |
| A | 封闭 S01–S07 的公开入口与越权路径 | 立即推进，中等到较大；ACL 统一较大 | 所有失败授权无副作用；公开输入不能执行代码或逃逸目录；只读凭证不能写 |
| B | 文档提交、恢复、存储、上传、锁及凭证消费 | 先定义事务和对象生命周期，较大 | 并发冲突可见；失败不部分提交；无静默丢失；所有重试幂等 |
| C | D06、F01 协作协议与前端恢复 | 依赖 B 的提交语义，较大 | 快照/增量有边界；双客户端、积压、重连及重启均收敛；dirty 编辑保留 |
| D | F02、F03、D07 算法与资源隔离 | 历史裁剪依赖 C；稀疏导出可独立，中等到较大 | 复杂度有计数证明；大任务可限流和停止；跨文档延迟不被单一热点主导 |
| E | F04–F08 类型与外部兼容性 | 统一模型后推进，中等到较大 | 外部 fixtures 与标准 part 检查通过；不支持内容有显式降级和原件恢复 |

规模为代码影响范围判断，不是已估算人天。建议按可独立验收的 PR 拆分，而不是把全部重构合成一个大提交。S08、O04 与 B 共用事务和生命周期改造，但各自保留独立验收。

### 应建立的核心不变量

1. 未通过授权的操作不会改变任何业务表、文件或成员关系。
2. 每次成功提交的文档版本唯一，数据库内容、下载 blob、版本快照和配额一致。
3. 已确认给客户端的编辑具有明确持久化保证，重连不会把版本推进到未包含的内容。
4. 远程更新不能无提示覆盖本地待保存内容，也不能将其标记为已保存。
5. 对象、上传块、会话与日志都有清晰所有者、回收条件和失败重试路径。
6. 计算与转换消耗由实际数据量及预算限制，不能由空白坐标范围或重复展开无界放大。
7. 原生缓存往返成功与外部 Office 格式兼容成功分别报告、分别测试。

### 测试补齐方向

- **权限矩阵：** 身份、角色、房间关系、凭证类型、接口/消息类型的组合，重点验证拒绝后的数据库和存储均不变。
- **并发与故障注入：** barrier 固定竞态窗口，注入 SQL、blob、日志和提交失败；不要只依赖概率性的高并发跑测。
- **协作恢复：** 双浏览器交错编辑、自动保存暂停、断线重连、广播积压、页面关闭及服务重启。
- **兼容性：** 无本产品 customXml 的外部 DOCX/XLSX/PPTX fixtures；检查标准 XML 与独立阅读器输出，另保留原生模型往返测试。
- **算法与容量：** 公式依赖访问计数、稀疏导出访问计数、长会话历史大小和跨文档隔离；基线形成后再约定 p95/p99 和容量目标。
- **交付：** 从无源码目录启动发行包；容器与发行包用同一浏览器保存/导出冒烟测试。

## 横向代码质量与效率改进

### 把业务不变量集中到共享服务

当前 handler 重复实现授权、配额、版本、对象写入和清理，S05、D01、D02、O04 是同一维护方式在不同路径上的表现。建议抽取授权服务、文档提交服务、上传状态机和清理服务。handler 负责输入、调用和 HTTP 映射；事务与跨存储补偿由共享服务表达。

前端和 doc-engine 分别实现字符串模型与公式求值，F02、F05、F06 说明两套语义已发生偏差。优先共享类型化文档模型、公式解析和求值语义，再优化渲染状态。避免为了“去重”把网络或 UI 状态混入文档核心。

### 区分必要写入与可降级副作用

必要 SQL 不应使用 let _、unwrap_or_default 来伪装成功。文档内容、文件指针、配额和安全凭证属于必须提交的状态；通知等可异步重试，但应留下失败记录。D02 的修复应形成可复用的错误传播和补偿约定，避免逐处打补丁。

### 先测资源边界，再做常规微优化

搜索目前对内容执行包含匹配并读取结果完整 JSON，再生成片段，见 [search.rs:70](E:/Works/arkret-org/convert/office/crates/server/src/handlers/search.rs:70)、[search.rs:127](E:/Works/arkret-org/convert/office/crates/server/src/handlers/search.rs:127)。数据量增长时应测扫描字节和查询时延，再决定全文索引及摘要存储；这里不把未测性能判断算作独立缺陷。

同样，先完成 F02、F03 和 D07，再评估 clone、序列化或 SQL 往返的小幅优化。重点指标应包括编辑主线程阻塞、转换队列等待、跨文档 p99、内存和数据库准入拒绝，而不只看平均响应。

### 增强就绪检测和失败可观测性

[health.rs:6](E:/Works/arkret-org/convert/office/crates/server/src/handlers/health.rs:6) 当前固定返回 ok、uptime=0，可作为存活入口，不能表明数据库或存储可用。建议另外提供有超时的 readiness，避免把昂贵依赖探测放进高频 liveness。监控查询失败不要统一折算为 0；区分真实空值与采集失败，见 [monitoring.rs:46](E:/Works/arkret-org/convert/office/crates/server/src/handlers/monitoring.rs:46)。

补齐事务冲突、上传恢复、对象回收失败、协作 lag/重放、转换队列等待、预算拒绝指标，并以 request/document/job ID 关联。日志不得包含 token、重置凭证或文档正文。

### 缩小文档与实现的差距

README 的数据库栈仍写 SQLx 和 PostgreSQL，当前实现使用 Diesel/Horme 且配置验证只支持 SQLite；部分格式仍列为 Planned，但已存在实现。历史报告的“已完成”也不能代表本次基线再次验收通过。建议以当前能力与兼容级别重写说明，明确协作协议、外部格式边界和运行资产要求。

## 原始评审提出的扩展风险（后续状态见上文）

以下不计入 27 项确认问题：

- **租户隔离：** 迁移添加 tenant_id，但主认证未见租户活动状态与统一租户上下文；本次不足以断言任意跨租户访问。需先定义隔离契约，再按全部资源做负向测试。
- **长连接撤权：** WS 握手后未见持续检查 session、过期和房间撤权。建议与 S04/S05 一起验证既有连接的撤权时效。
- **群组分享可见性：** shares 创建 public token 的条件只看 shared_with_user_id，需核实群组分享是否意外附带公开能力。
- **导入资源预算：** DOCX/PPTX 的部分 XML 解压读取没有硬上限；需按可达入口验证压缩炸弹、部件数量、解压总大小、嵌套及解析时间预算，不能仅限制上传压缩体积。
- **可访问性与交互：** 本轮没有实际浏览器键盘、焦点、读屏和触屏验收，不给出 WCAG 或设备支持结论。
- **供应链：** 未执行本轮最新漏洞库审计，也未对依赖做全面审计；历史 cargo audit 结果不能替代当前扫描。CI 中已有安全审计任务可作为后续入口。

## 原始评审阶段的验证记录与限制

| 检查 | 结果与含义 |
| --- | --- |
| git status --short | 开始时干净；本次只新增本报告 |
| cargo metadata --no-deps --format-version 1 | 可解析本 workspace，target_directory 指向用户禁止目录；没有运行构建或访问该目录 |
| cargo fmt -- --check | 通过；这是项目脚本使用的格式命令 |
| cargo fmt --all -- --check | 失败；递归处理本地 path 依赖时缺少 .dependencies/apace/packages/apace_accesskit/Cargo.toml，不是本项目格式失败 |
| git diff --check | 通过；新增报告另做尾部空白、问题数量和代码链接检查 |
| 24 个迁移的内存 SQLite 执行 | 首次执行无错误；按 main.rs 的重复列容错规则再次执行无错误 |
| 未授权房间删除 SQL 顺序 | 复现成员已删除而父房间未删除；不是完整 HTTP 测试 |
| 独占锁 schema | 复现同 file_id 接受两条不同 token 的记录；不是并发 HTTP 测试 |
| 静态路径拼接 | 使用 README 非敏感目标验证会逃逸 static 根目录；未发送攻击 HTTP 请求 |
| 完整 Rust 测试、Clippy、wasm 检查、E2E | 未执行：当前构建/产物设置违反路径约束；未改写环境或引入替代缓存根目录 |
| Docker 与压力测试 | 未执行 Docker；未查询或使用 C 盘 Docker 数据磁盘；未执行大输入耗尽测试 |

内存 SQLite probe 使用实际迁移 schema 及对应 SQL 顺序，其 foreign_keys=0 仅代表 probe 连接的设置。database.rs 初始化未显式启用外键，生产连接还需核实 Diesel 及 SQLite 编译配置的有效行为；不能假设 schema 中的 REFERENCES/ON DELETE CASCADE 已实际生效。后续启用外键前应先检查存量孤儿数据并核对删除顺序。

原始评审阶段未修改业务代码或构建配置；随后修复阶段的实际状态以上方执行记录为准。历史 _improve.md 保留，未提交或推送改动。原始审计结论针对 f18c115 基线，不能代替当前工作区验收。
