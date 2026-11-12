# Office 项目多 Agent 代码评审与改进报告

评审日期：2026 年 11 月 9 日。基线：本地提交 f18c115（Unify Office UI and migrate database queries to async Horme leases）。开始评审时工作区无未提交变更。

原始评审建议先封闭文件读取与授权缺口，再建立可靠的文档提交和协作恢复协议；性能方面先修正公式重复求值、稀疏导出扫描和协作历史增长。后续多 agent 修复已按此顺序推进，当前结果如下。

原始评审汇总 **27 项有源码依据的问题，其中 P1 17 项、P2 10 项**。当时的“确认”指源码缺陷或可推演路径，其中两项另有内存 SQLite 复现，目录穿越有文件系统验证。原始问题各节保留当时的验收要求；后续修复及真实测试结果以下述执行记录为准。

## 修复执行状态

| 维度 | 当前结论 | 仍需闭环 |
| --- | --- | --- |
| 安全与授权 | 原27项中的路径、凭证撤销、跨协议ACL及单次消费有实现与当前双端完整回归；native144已通过 | 不等同全面安全认证；当前remote安全任务未分配runner |
| 数据一致性 | 当前CAS、补偿、持久ACK、receipt、D04与GC回归通过；两平台ZIP真实强停与冷备恢复、各自027三库均通过，Garage4独立通过 | AWS、多实例共享桶及真实断电另行验收 |
| 并发与生命周期 | 当前保留CleanupOwner保护，并实施query-only固定线程实验；原取消写锁回归及三项新增真实回归在双端完整套件中通过 | 不承诺物理连接所有Drop都同线程，不证明RSS原因或回收目标已完成 |
| 效率与性能 | 当前公式预算/复杂度、稀疏导出、历史裁剪与GC索引回归通过 | 当前instrumented与normal各100k功能/策略/恢复通过，但离开后均未观察到RSS下降；原内存要求开放，不承诺通用延迟或零拒绝 |
| 工程质量与可观测性 | 当前2495/source bdc/freeze24a：Linux与Windows各624/0/6/0、27个测试可执行+6doc；strict full Clippy0、正常release及独立canonical齐备 | 旧602/614/620及c04/f5诊断仍按旧输入保留；远端runner0不是已执行CI |
| 交付与恢复 | 原脚本正式构建21Web资产；native144、两平台ZIP/冷备/027三库、Garage4及两次100k功能均为当前输入的真实结果 | RSS原目标未完成；当前Docker/macOS/AWS未验 |

当前查询 worker 候选已实现并完成本轮验收：Linux/Windows 各 **624 通过、0 失败、6 忽略、0 过滤**，两端 strict full Clippy、正常 release、正式 21 项 Web 资产、native 144、两个 ZIP/冷备/027 及 Garage4 均有独立结果。两次各 100,000 操作的仪表与正常长测均完成协议、持久化、历史/GC 收敛和同进程重开；**RSS 回落目标仍未完成**，终态分别较基线高 16,211,968 / 15,958,016 字节。验收运行时为 `22ab8a4` 加 worker 改动，完整清单 SHA `bdc8c25e…71fae`；报告整理发生在全部运行捕获结束后，不改产品源码，历史身份与当前结果分别保留。当前源码 Docker 镜像、Intel macOS 实际 CI 及 AWS 专用环境仍未验收。

phase9 阶段已补齐安全与格式的直接验收组合，并修复会话退出与重新加入的竞态、延迟退休缺少自动重试的问题。Windows/Linux 完整 workspace 各 **602 通过、0 失败、5 忽略**，两平台严格 Clippy、正常 release，以及 Linux **144 项**浏览器/API 验收通过；当时两平台 phase9 ZIP 的完整烟测与冷备恢复均通过，Garage **4/4** 属于全套之外的独立环境验收。六个提交已按用户授权推送至 `origin/main` 的 `2c9a249`。该次远端 CI 因账户付款/支出额度启动门禁未分配 runner，不能记为远端构建或测试已执行；旧失败 attempt 的状态不说明当前账户是否已恢复。**D07 该阶段正常 release 已完成 100,000 次 ACK、21 个持久化检查点、历史/GC 收敛及同进程退休重开与最终 REST；RSS 充分回收仍未证明**，收敛后仍高于基线 104,259,584 字节，完整内存验收未完成。历史失败、运行时源码快照与提交后的内容等价桥接分别记录如下，不把旧 HEAD 的测试改称新 HEAD 重跑。

2026 年 11 月 9 日启动多 agent 修复。11 月 10 日的 `19d7409` 已通过 **Linux 555 项 Rust、20 项定点重复、Linux/Windows 各 136 项联合 HTTP/浏览器回归，以及两个 ZIP 的浏览器与强停重启验收**。联合运行均无重试，真实 Garage 兼容服务的 3 项验收亦通过。该版本的 8 并发测量保留 **0.75% 的 503 拒绝**。11 月 11 日续修完成 D 盘上的 Linux/amd64 容器实测，并加入鉴权容量、启动诊断和本地冷备演练。各轮证据分别列出，macOS 与真实 AWS 桶仍待运行验证。

### Phase12：固定查询 worker、双端交付与两次 100k（11 月 12 日）

**验收运行身份与历史区别。** 运行时 HEAD 为 `22ab8a427d8a319d180a591228e172f2b9607a41`，运行时工作区包含修改后的 `database.rs` 和新增 `database/worker.rs`。完整清单2495文件，source SHA `bdc8c25e1860a366bf4e008a17080bd0c6ddfc298aa330456d37edcb97d71fae`；critical freeze SHA `24a151e8413f6121ab9505d6a53fded299bde54b3d70710cd0c0ad06b0a3601f`，其中 database 为 `7652272c823ddd67cbf7ff01463d473df2b07c88cb0fc94dee8fde80b67873b7`，worker 为 `232d6d7fe17ee281bd817f8fbdb92a7967024cb52542d091bd399b7e3b6e31cb`。之前 `1210805` 窄修、c04/f5完整620、旧正常/仪表100k分别保留各自源码身份，不改称本轮执行。本节整理及后续提交仅将已验证代码与报告归档，不把整理后的报告字节冒称原运行输入。查询 worker 仍是待完成内存验收的候选。

**具体机制与限制。** 在原SQL/checkout/busy时限、池容量、Admission、事务和CleanupOwner语义下，为每个物理连接安排专属native查询worker，保持已声明工作的FIFO。调用方取消不会静默撤销已准入SQL；不再持有物理wrapper后，Admission仍需等实际opaque callback/inner Arc退休，释放回调在mailbox锁外执行。mailbox的一个普通等待槽不代表全部prepared callbacks/前驱waiters全局O(1)。query-only变体保留空闲物理连接同步Drop的原调用者语义；繁忙任务可活过Tokio runtime teardown，原生worker需以真实Stop信号确认退出，不声称所有SQLite allocation/free或物理Drop都有同线程亲和性。此机制也改变线程存续与调度，不能单凭本轮RSS差异就归因为迁移。

**三项新增真实回归，均包含在当前624中，不另累加。**

| 实际测试名 | 验证边界 |
| --- | --- |
| `database::query_worker_regressions::every_physical_database_connection_keeps_one_query_thread` | 实际Database容量1/5、真实startup迁移、TEMP连接标记及精确SQL计数；每次查询断言每物理连接唯一ThreadId，另持有八个全局blocking正控；释放/等待所有blocker与原生Stop，不改业务时限或runtime worker数 |
| `database::query_worker_regressions::declared_fifo_survives_a_cancelled_queued_caller` | 实际SqliteRuntime控制首callback、取消第二调用方并等待第三，已声明工作仍准确执行一次且顺序为1/2/3；不使用镜像队列、sleep或重试 |
| `database::query_worker_regressions::queued_sql_survives_runtime_teardown_and_holds_admission_until_owner_destruction` | 真实mailbox queued witness、取消调用方及销毁current-thread runtime后，真实SQL才开始；Admission保持1，外部busy_timeout0写锁正控失败；释放未提交事务后只在opaque owner真实销毁后Admission归零、回滚及peer写锁一次成功，并观察Stop |

原 `shutdown_before_cleanup_owner_is_polled_retires_the_sqlite_writer` 断言未放宽，本轮完整套件真实通过。线程迁移ignored probe仍为独立诊断，不加进默认套件通过数，也不以旧probe结果代替新设计的RSS因果。

**当前完整矩阵。** Linux和Windows各实际完成 **624 passed / 0 failed / 6 ignored / 0 filtered**；各为27个metadata匹配的真实测试ELF/EXE和6个doc-test，逐目标没有遗漏、重复或过滤。6项ignored为四项Garage环境用例、容量基准及独立迁移probe。两平台严格workspace SSR/all-targets Clippy实际0；Compile/Test/Clippy使用完整workspace，正常Release为server-only且无SSR/test特征。四阶段源码与调用者环境前后一致；当前normal ELF为 `525b5c20…83c104`，Windows PE为 `91310458…c67330`。独立canonical summary/identity仅派生自原始证明，原raw release的 `raw_release_build_only=true`、`normal_release_acceptance=false`、`full_workspace_accepted=false` 原样保留。

[Linux canonical](E:/Works/arkret-org/convert/office/target/review-phase12-validation/native-functional-delivery-export-4051c5bc7c764514848a594748c1b3bb/canonical/workspace-summary.json) · [Windows canonical](E:/Works/arkret-org/convert/office/target/review-phase12-validation/phase12-canonical-9d4f5afa41dd454dbc098a920eef1a37/workspace-summary.json) · [Windows当前原始Test](E:/Works/arkret-org/convert/office/target/review-phase12-validation/windows-full-test-b47c045cf41a4f13b02994a53e196567/result.json)

**正式Web与native144。** 使用原 `build-frontend.ps1 -Release` 正式构建21项Web资产，WASM SHA `b30ef73f…26a4ad`、Worker SHA `610bf41d…0fd7e8`。当前native正常ELF和相同源码/资产随后完成全部144项浏览器/API测试，零失败、跳过、flaky和重试；这是当前功能验收，不是RSS或Windows包验收。

[正式前端构建](E:/Works/arkret-org/convert/office/target/review-phase12-validation/native-functional-delivery-export-4051c5bc7c764514848a594748c1b3bb/frontend/result.json) · [144实际结果](E:/Works/arkret-org/convert/office/target/review-phase12-validation/native-functional-delivery-export-4051c5bc7c764514848a594748c1b3bb/inner/result.json)

**Linux ZIP、冷备及027。** 当前ZIP SHA为 `43f01b023e4a6153152317c45d30685b735ef80ebea09deffb029c60342792d3`，56文件，ELF普通可执行权限100755，服务器/21Web/config/migrations/docs逐项绑定。实际无源码启动、Worker浏览器、REST/WS强停、持久ACK/receipt重放与D04续传/冲突/完成幂等均通过；停写后DB+有效WAL+匹配blob/上传原件冷备、全新路径恢复、缺失/篡改对象负例通过。三个本次owned数据库均实际验证027索引owner及 `(document_id ASC, version DESC, id DESC)`，只读检查DB/WAL前后哈希相等。原canonical ZIP/Web恢复，25个既有档案及原DB哈希未变，自有contrix/chrome/e2e Node退出。本地冷备不是管理员DB-only备份接口已有全量blob能力，也不是AWS或真实断电保证。

[Linux包与恢复终态](E:/Works/arkret-org/convert/office/target/review-phase12-validation/linux-package/phase12-package-c65c8820f98c44f5a7054b3e64fffa3c/delivery-summary.json)

**当前Garage4。** 另开资源窗口，在原b77五个连续样本准入后，用本次FullCompile的实际三ELF哈希及当前full624/canonical/source绑定运行原scope适配wrapper。Garage v2.5.0为真实loopback S3兼容服务；四个exact names全部通过，两组实际4.20秒和0.84秒，原12秒readiness与各180秒上限、串行、零重试保留。maintenance的 `--ignored` 自然排除一个非ignored用例，S3组filtered0，没有增加grep或补发。source-after仍为bdc，三ELF前后相同；launcher/child身份与退出已证，凭据删除、RPC secret脱敏，owned进程为空，本次meta/objects限定路径清理，原始诊断仅留native。此4项不加进624，不代表真实AWS桶验收。

[Garage实际结果](E:/Works/arkret-org/convert/office/target/review-phase12-validation/garage-current-prep-1a918f8c102948c5987e60a22b63d504/runtime-authorization-072d5400f3fb49cca5f6ab37a7ee7b75/safe/result.json) · [独立source/owned/data清理](E:/Works/arkret-org/convert/office/target/review-phase12-validation/garage-current-prep-1a918f8c102948c5987e60a22b63d504/runtime-authorization-072d5400f3fb49cca5f6ab37a7ee7b75/independent-cleanup.json)

**Windows当前ZIP真实验收。** 原guarded适配器使用当前自然canonical PE91310458与接受的immutable PE逐项相等，调用原 `scripts/package.ps1 -SkipBuild`，本轮没有Cargo build。ZIP SHA `4d4188cea0596f7d6a50dddd660b7e6db49f6161953425bbb70e4e56fe320f3f`，绑定当前21Web、native144来源及Windows四阶段完整manifest。实际source-free启动、Worker浏览器、REST/WS强停与ACK/receipt幂等、D04首块强停续传/异内容冲突拒绝/完成与重放只计20,209字节均通过。独立冷备恢复证明停写后有效WAL（1,606,832字节）与2个blob/原件匹配、WAL replay必需、quick_check、备份不变及缺失/篡改负例；三个本轮owned DB均实际核对027索引owner/ASC-DESC键和DB/WAL前后哈希相等。七次受约束移动后canonical ZIP/Web按原哈希或原absence恢复，legacy archives未变。

原脚本finally及独立CIM复查确认无本包server、package.ui.spec.ts Node及本包创建时间窗口内Chrome/headless/Edge遗留。窗口内另一个Node被核为旧Codex的require-browser-access.mjs，不属于本包，未终止；不声称全机器没有Node/Chrome。

[Windows outer终态e2e98ba3](E:/Works/arkret-org/convert/office/target/review-phase12-validation/windows-package-outer-26297c24f23b42b1bf8d68374c25bd4f/result.json) · [inner终态0b0c2bed](E:/Works/arkret-org/convert/office/target/review-phase12-validation/windows-package-076e40ce74f144a185c711a829fedab8/result.json) · [027三库实际proof a86e2812](E:/Works/arkret-org/convert/office/target/review-phase12-validation/windows-package-076e40ce74f144a185c711a829fedab8/database-index-proof.json) · [烟测9083ee2c](E:/Works/arkret-org/convert/office/target/package-smoke/96aecf23ef524bf19a9f22763e05fe47/result.json) · [冷备71d88397](E:/Works/arkret-org/convert/office/target/disaster-recovery/6fba00cf52bf474baf0d2ee0db750401/result.json)

**两次100k实际终态，功能与RSS分开。** 当前候选先完成四边界仪表run `20261112T014015Z-3d718fd5`，再完成原35ca正常无仪表run `20261112T021436Z-a6c71468`。两者各100,000次attempt/ACK、21个持久化checkpoint、单inflight、零retry/resume，均完成历史/GC收敛、原60秒策略窗口、同进程重开与最终REST、owned退出。两次动态预算分别436.5秒与335.5秒，依各自初始candidate页数按原公式计算，未固定成旧375秒或添加余量。仪表convergence阶段55.207秒、距close110.013秒；normal历史/GC首次ready0.008449秒，等待原策略窗口后overall10.036890秒、距close64.841345秒。log/receipt/snapshot最终各2,048，GC pending/eligible/expired/writing/deleting均0，版本100001和最终内容一致。经过窗口不单独证明某次reaper执行或最后Arc释放；重开黑盒验证与内存要求分开。

| 当前run | baseline RSS bytes | before leave | after30s | final | final−baseline | final−leave |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| instrumented923b | 28,254,208 | 44,441,600 | 44,462,080 | 44,466,176 | +16,211,968 | +24,576 |
| normal35ca | 28,413,952 | 44,371,968 | 44,371,968 | 44,371,968 | +15,958,016 | 0 |

**当前两次协议/持久化/策略/恢复功能通过，RSS原回落要求仍未满足。** 两次离开后都没有下降；normal采集证明的normal/RSS acceptance仍为null，仪表的normal_d07_acceptance仍为false。旧c04/f5仪表残留+96,043,008字节仅作历史对照，较小残留不能替代验收或证明线程迁移因果。

仪表S从6,852,608增至22,179,840后不降、arena34→47后不降，线程34→47→35→35；F终态15,818,166，U终态6,361,674。F不是RSS，U不是Rust live heap，/proc与顺序arena采样非同一瞬间，不用RSS−F推导存活堆；normal没有allocator XML，不反造S/F/U。normal线程34→45→34→34而RSS不降。两次原result/DB/WAL/SHM采集前后哈希相等，source-after2495仍bdc，harness/server已gone；仪表六采集stage全0，normal采集/native/transport全true，normal27迁移hash匹配当前源、027只打开新字节副本核对。

normal全100,000行operation和100/2940/100行cold延迟以原nearest-rank重新计算与safe结果一致：send→ACK p50/p95/p99=5.440/12.617/35.407ms，during冷文档p95/p99=3.139/5.360ms；期间PSI full avg10最高51.61%。运行中外部Windows编译有记录，夹具HTTP限流关闭；这些不是独占性能基准、生产100/60吞吐或观察者开销的受控对照。

[仪表实际终态](E:/Works/arkret-org/convert/office/target/review-phase12-validation/d07-terminal-safe-2a52da5bc065483c96c30356d4743138/result.json) · [仪表独立算术](E:/Works/arkret-org/convert/office/target/review-phase12-validation/allocator-phase-analysis-8e1e3a0297474acfbca0361c4e20c38f/numeric-result.json) · [normal实际终态7653078c](E:/Works/arkret-org/convert/office/target/review-phase12-validation/phase12-normal-terminal-e9ca5b14505f6116dfffa02d881f8fd2bbe9c0daf0224665a36e145541f53696/safe/result.json) · [normal采集bf5a6ec5](E:/Works/arkret-org/convert/office/target/review-phase12-validation/phase12-normal-terminal-e9ca5b14505f6116dfffa02d881f8fd2bbe9c0daf0224665a36e145541f53696/post-evidence-result.json) · [normal独立审计](E:/Works/arkret-org/convert/office/target/review-phase12-validation/normal-d07-independent-audit-f13d8c145710487090cbfb9fcf12f813/CONCLUSION.md)

**剩余目标。** RSS充分回落及其直接归因仍开放；当前Docker镜像没有本轮build/验收，需合法动作范围和实际D/远端storage证明；macOS/真实远端runner、AWS专用桶/多实例协调及真实断电边界保持。两平台包、当前full/144/Garage与两次100k功能不再列为未运行。下一步先补实际连接缓存用量和所有权计数；剩余驻留的归因尚未证实。

**F07与历史独立阅读器。** 当前完整套件实际执行标准OOXML/styles回归；独立openpyxl/python-pptx reader实跑仍属于phase9生成输出。只读逐项比较生成时清单与当前bdc，export/doc-engine/core全部源、fixture、example、reader及根Cargo.toml/Cargo.lock共44文件SHA全部一致，历史reader输入文件仍匹配原哈希。因此原F07独立reader要求保留已有实际证据并以组件内容桥接，不冒称reader在本次ELF重跑。

[44文件组件内容桥接](E:/Works/arkret-org/convert/office/target/review-phase12-validation/completion-audit-51b305be3abc4d41ad63437f784bbdb6/F07-component-content-bridge.json)

**remote、Docker及环境边界。** 缓存的当前HEAD22a [CI43122967873](https://github.com/arkret-org/office/actions/runs/43122967873) 有7个failed job，均runner0/steps0，另2个skipped；具体启动原因unknown。本轮只读其本地证据，未联网补查；不把旧账户账单注解用作本次原因，也不称dirty候选已获远端验收。历史phase3镜像的D盘builder构建/运行证据仍有效于旧输入。`Diagnostic-A-create-rejection.txt`的具体拒绝是固定镜像的volume/create/start诊断流程，命令未执行、未产生session，审批未提供具体规则，不改称一次已经执行的Docker失败或单独builder创建。该被拒流程不可等价重试或换入口绕过。只读存储发现表明旧D盘WSL实例仍注册、VHDX仍在且Stopped；这不是本轮运行态Docker storage验证。现有Desktop数据盘仍在C，当前源码镜像尚未验收；任何后续build仍须先确认实际builder/BuildKit/containerd/overlay/volume在D或远端。Intel macOS实际CI、AWS专用桶及多实例/断电边界继续保留。

[22a CI缓存](E:/Works/arkret-org/convert/office/target/review-phase12-validation/remote-ci-22ab8a4-43122967873.json) · [具体Docker自动审批拒绝](E:/Works/arkret-org/convert/office/target/review-phase5-container/Diagnostic-A-create-rejection.txt)

**拒绝记录更正。** 原tracked报告中“本地新Docker builder创建曾被自动审批拒绝”的表述，目前未找到该第二次动作的独立原tool/file证据，不能继续列为已证事实。可证的Docker拒绝仅为上述Diagnostic-A额外诊断volume/container创建与启动组合；GitHub annotation CLI拒绝属于另一服务动作。当前源码镜像build本轮没有实际尝试。此更正不授权重试Diagnostic-A或等价变体，也不取消build前实际D/远端storage证明的要求。

### Phase12 前置记录：CleanupOwner 取消保护与迁移观察（11 月 12 日，22ab8a4）

**已实现与边界。** `DatabaseLease::drop` 现在在构造 async owner 前创建 [CleanupOwner](E:/Works/arkret-org/convert/office/crates/server/src/database.rs:358)：只有清理 barrier 成功才普通归还池；错误、未轮询即取消或 shutdown 默认 `Object::take` 退休连接，无 runtime 分支沿用直接退休，Admission 仍由原清理 SQL 闭包持有。当前 `database.rs` SHA 为 `1210805fc711bf99b990bafd7b32c4b915ae1589eab3fbd617c29a37cd290d9b`，绿例完整源清单为 `fb01f959…cabe1a`。该修复封闭的是取消后未认证连接仍 idle 持有 SQLite 写锁的路径，不是已证实的 D07 RSS 原因。

**实际红绿与验证范围。** [确定性回归](E:/Works/arkret-org/convert/office/crates/server/src/database.rs:1295) 使用真实 Diesel 事务和外部 busy_timeout0 写锁正控，在 current-thread runtime 不再 poll 清理任务时 shutdown；不依赖 sleep、专属 worker 或后续 checkout。旧生产逻辑实际退出101，外部 BEGIN 仍报 `database is locked`；窄修后精确 case **1/0/0/110filtered、退出0**，验证立即释放写锁、回滚未提交行及池/Admission 退休。随后数据库模块实际 **14/0/1ignored/96filtered**（已包含该 case，非另加14），原源/环境/ELF 前后相等，绿例与数据库模块均在原双资源门禁3d271通过后执行。仅6库 no-run compile0，未执行新全套、Windows、strict Clippy 或 normal release。 [红例原结果](E:/Works/arkret-org/convert/office/target/review-phase11-validation/cleanup-owner-green-a2a443af8af246aeb3997cff8db770d7/runtime-evidence-e0f31dcab76f4c65b084ac8ac7381bb5/cleanup-owner-red-a54a95fd0c154bd5b9a2b19cf0c28d44-result.json) · [精确绿例](E:/Works/arkret-org/convert/office/target/review-phase11-validation/cleanup-owner-green-a2a443af8af246aeb3997cff8db770d7/runtime-evidence-e0f31dcab76f4c65b084ac8ac7381bb5/cleanup-owner-green-6bce446f85d4464f949760e646f9ead9-result.json) · [数据库模块原日志](E:/Works/arkret-org/convert/office/target/review-phase11-validation/cleanup-owner-green-a2a443af8af246aeb3997cff8db770d7/runtime-evidence-e0f31dcab76f4c65b084ac8ac7381bb5/cleanup-owner-green-930e5fb317d1435792c99b2b0ec26349-test.log)

旧编译包装器 `51359774…4be9d` 在 Cargo0 后因 library kind 判断遗漏 rlib/cdylib 而 false，原件保留；修复包装器后，红例源码 compile `850c1500…097f1`、绿例源码 compile `5c504d72…4065` 均真实0，不能把包装器失败改写成测试失败或覆盖旧 proof。9个原始证明/日志通过 byte-only 导出，源/副本/源after SHA 一致。 [旧 wrapper false](E:/Works/arkret-org/convert/office/target/review-phase11-validation/cleanup-owner-green-a2a443af8af246aeb3997cff8db770d7/runtime-evidence-e0f31dcab76f4c65b084ac8ac7381bb5/phase11-native-compile-libs-6e6c4b2f1fea4bbcb957557ca6534c72-result.json) · [修后红源码编译](E:/Works/arkret-org/convert/office/target/review-phase11-validation/cleanup-owner-green-a2a443af8af246aeb3997cff8db770d7/runtime-evidence-e0f31dcab76f4c65b084ac8ac7381bb5/phase11-native-compile-libs-b1cbbc769c36442e86ae39624113d1c7-result.json) · [绿源码编译](E:/Works/arkret-org/convert/office/target/review-phase11-validation/cleanup-owner-green-a2a443af8af246aeb3997cff8db770d7/runtime-evidence-e0f31dcab76f4c65b084ac8ac7381bb5/phase11-native-compile-libs-bb8ce8ca37eb4e349753ea6eaf753497-result.json) · [字节副本清单](E:/Works/arkret-org/convert/office/target/review-phase11-validation/cleanup-owner-green-a2a443af8af246aeb3997cff8db770d7/runtime-evidence-e0f31dcab76f4c65b084ac8ac7381bb5/byte-copy-proof.json)

**迁移观察不等于内存归因。** 独立 ignored 迁移基线实际退出0、结果 SHA `9f89bd4c…8129cf`，TEMP 标记和计数验证的6/6连接都观察到跨线程。capacity1 普通段只有1个 worker，8-blocker 正控后扩到2；capacity5 普通段每连接涉及9个 worker，但它在同一 PID/运行时内继 capacity1 的8-blocker 正控之后执行，不能称未干预拓扑。该结果证明迁移可发生，未证明正常 D07 的 allocator/RSS 因果；`rss_or_d07_causation_proved=false` 和 `full_suite_acceptance=false` 原样保留。query-only 候选仍只在 target，尚未应用。原 RSS 充分回落要求和当前待补的完整验收保持开放。 [迁移基线原结果](E:/Works/arkret-org/convert/office/target/review-phase11-validation/thread-migration-run-8036b279effb4ca1a03aa8112826218d/actual-migration-fbe724/result.json)

### Phase11：有界数据库观测、历史流式读取与完整四边界诊断（11 月 12 日）

**本阶段范围与身份。** 继续原始 27 项评审及 D07“所有人离开后会话和内存按策略回落”的验收，不以小幅 RSS 下降、会话退休或 GC 清零替代内存要求。本轮完整源码为 `c04ba1cdaa81f824f6c19ddefa4b3b518248bc6321c068904424d30982a3f28c`，关键冻结 `6cc4ac1c…d2c3bd`，新增 telemetry 模块为 `f5a1a082…ca0b00`；源码计数增加到 2,494。该清单保留测试期间 HEAD/index/status 和实际逐文件字节，下面结果没有仅凭提交号推定源码一致。报告本次更新发生在所有运行与源码捕获结束之后。 [本轮冻结](E:/Works/arkret-org/convert/office/target/review-phase11-validation/source-freeze-1300c993959449cdb15203b7c9bfd546.json) · [完整同步证据](E:/Works/arkret-org/convert/office/target/review-phase11-validation/phase11-native-sync-53428a79347a4e588efb13c61213df57/byte-export-a0587e58b28540e6b4d18b3ac0c4443a/result.json)

**实现与回归。** 七个产品/测试文件的改动保持既有授权、事务和资源上限：

| 文件与当前入口 | 本轮行为与直接回归 |
| --- | --- |
| [database.rs instrumentation](E:/Works/arkret-org/convert/office/crates/server/src/database.rs:79) | 接入连接/事务状态；请求与后台工作保留各自观测上下文，观测记录不替代 SQL 成功结果。 |
| [typed IMMEDIATE helper](E:/Works/arkret-org/convert/office/crates/server/src/database/telemetry.rs:124)、[每池有界 monitor](E:/Works/arkret-org/convert/office/crates/server/src/database/telemetry.rs:342) | 调用点给出固定事务意图，区分 begin_wait/held/commit/rollback 和实际耗时；三项事务耗时在同一采样时刻冻结，日志排队不再抬高 begin 等待时长。观测容量有界，快 GC 结束释放记录，慢日志在锁外处理，未记录 SQL 正文或凭据。 |
| [请求观测接收事务快照](E:/Works/arkret-org/convert/office/crates/server/src/middleware/request_observability.rs:160) | HTTP/WS 的原有阶段、原因及 worker 统计与事务快照关联；缺失观测不伪装成全覆盖。 |
| [GC backlog 门禁](E:/Works/arkret-org/convert/office/crates/server/src/services/maintenance.rs:29)、[claim 事务](E:/Works/arkret-org/convert/office/crates/server/src/services/maintenance.rs:150) | 维持 1,024 backlog 上限及原租约/引用语义，加入 typed 事务阶段。 |
| [GC 调度](E:/Works/arkret-org/convert/office/crates/server/src/services/persistence.rs:439)、[批次执行](E:/Works/arkret-org/convert/office/crates/server/src/services/persistence.rs:526) | 观测已选/已 claim、并发 unlink、失败及 finalize 数，日志工作不占用这些状态锁；原发布事务、失败回滚及后台重试规则继续有效。 |
| [历史策略流式检查](E:/Works/arkret-org/convert/office/crates/server/src/services/operation_history.rs:72) | `load_iter` 逐条解码长度并及时释放，仅另取一个 cutoff 键；保持 2,048 行/32 MiB、version/id 排序、未来版本保护、前台回滚与有界后台清理，不为每条保留记录构造 id String。SQLite 读取 overflow 页等成本未被此计数覆盖。 |
| [实际分配回归](E:/Works/arkret-org/convert/office/crates/server/tests/operation_history_regressions.rs:379)、[负版本/同版本排序与 UTF-8/NUL](E:/Works/arkret-org/convert/office/crates/server/tests/operation_history_regressions.rs:465) | 与四项 telemetry 回归共同组成本轮六个新增 case；队列时长的扩展断言位于既有 case，不另计测试数。 |

四项 telemetry 回归覆盖真实 IMMEDIATE 等待者与持锁事务、慢日志不持观测 mutex 且不泄漏错误正文、GC claim/阻塞 unlink/失败/finalize 计数，以及快 GC 不累积观测或日志。它们与两项历史回归在最终完整日志中各有唯一 `ok`（Linux 行 317–320、697、700）。 [真实等待者](E:/Works/arkret-org/convert/office/crates/server/src/database/telemetry.rs:752) · [慢日志边界](E:/Works/arkret-org/convert/office/crates/server/src/database/telemetry.rs:953) · [GC 计数](E:/Works/arkret-org/convert/office/crates/server/src/database/telemetry.rs:1076) · [快 GC 退休](E:/Works/arkret-org/convert/office/crates/server/src/database/telemetry.rs:1235) · [逐目标与六 case 日志核对](E:/Works/arkret-org/convert/office/target/review-phase11-validation/phase11-native-test-full-ae6af485af32450f9ed8f0e5189240c8/byte-export-c7597f00791d48109f96e437ccba1587/runtime-log-audit.json)

较早 cad9 组件快照的一次独立分配 probe 记录：短输入 18 次 / 1,561 requested bytes，2,049 条输入 27 次 / 2,449 bytes，旧 eager 正控 2,066 次 / 414,250 bytes。计数来自线程局部 Rust alloc/realloc 请求与累计请求字节，排除 SQLite C heap，不是 live heap、进程 RSS 或全应用性能；该独立 exact case 也不累加到完整套件通过数。最终 f5/c04 的同一回归已在完整 620 中执行，但没有将旧 probe 数值改称该 ELF 的新测量。 [组件 probe 原始结果](E:/Works/arkret-org/convert/office/target/review-phase11-validation/phase11-allocation-observation-e8c979c2a0cf48188e8ab43cbd00437c/result.json) · [原始输出](E:/Works/arkret-org/convert/office/target/review-phase11-validation/phase11-allocation-observation-e8c979c2a0cf48188e8ab43cbd00437c/probe.log)

**完整矩阵与失败保留。** 中间删除仍在使用的 `TransactionSnapshot::begin_wait_us` getter 后，Linux 和 Windows full Compile 各实际退出 101/E0599，尚未进入该次 runtime；原日志和 false 结果保留。恢复 getter 并完成 f5 冻结后重新取得下面完整矩阵；旧 cad9 Compile0/27 targets 和旧 156 项定点结果均不代替这轮最终源码验收。 [Linux 原失败](E:/Works/arkret-org/convert/office/target/review-phase11-validation/phase11-native-compile-full-c39a9a881e134327bec831d6c443c9be/byte-export-a0587e58b28540e6b4d18b3ac0c4443a/result.json) · [Windows 原失败](E:/Works/arkret-org/convert/office/target/review-phase11-validation/windows-full-compile-68cb705cb23046268e3ca7017d08ef45/result.json)

| 当前源码实际阶段 | Linux | Windows |
| --- | --- | --- |
| Full Compile / Test | [Compile0](E:/Works/arkret-org/convert/office/target/review-phase11-validation/phase11-native-compile-full-e5e427495bde451aa3a4d1adac191973/byte-export-e2f059ab2594415eb3816c88800b2487/result.json)；[620/0/5/0](E:/Works/arkret-org/convert/office/target/review-phase11-validation/phase11-native-test-full-ae6af485af32450f9ed8f0e5189240c8/byte-export-c7597f00791d48109f96e437ccba1587/result.json) | [Compile0](E:/Works/arkret-org/convert/office/target/review-phase11-validation/windows-full-compile-2c21569599904b8aa69ff532353600ce/result.json)；[620/0/5/0](E:/Works/arkret-org/convert/office/target/review-phase11-validation/windows-full-test-4c18b39c7c144604b1f534484aafeb91/result.json) |
| 完整执行目标 | 27 个测试 ELF + 6 个 metadata doctest，逐目标无过滤/遗漏/重复 | 27 个测试 EXE + 6 个 metadata doctest，逐目标无过滤/遗漏/重复 |
| strict full Clippy | [实际0](E:/Works/arkret-org/convert/office/target/review-phase11-validation/phase11-native-clippy-full-db6033da0fc3477488b2e86efe9ef874/byte-export-e2f059ab2594415eb3816c88800b2487/result.json) | [实际0](E:/Works/arkret-org/convert/office/target/review-phase11-validation/windows-full-clippy-b8665c833ed144d09923d27c3486d05a/result.json) |
| 正常 server release 原始构建 | [raw0；ELF f4bef446…612c91](E:/Works/arkret-org/convert/office/target/review-phase11-validation/phase11-native-release-raw-a7d6ebd57ec142e78bf10946d044490a/byte-export-e2f059ab2594415eb3816c88800b2487/result.json) | [raw0；PE 2292ad90…1256a9](E:/Works/arkret-org/convert/office/target/review-phase11-validation/windows-full-release-d478b67f870f43dc844a3615188a6fa9/result.json) |
| 独立规范化身份 | [summary b7bd / identity 154e](E:/Works/arkret-org/convert/office/target/review-phase11-validation/native-canonical-byte-export-590a27a1ebb84b3fbb60d78949bb6c3a/release-identity.json) | [summary 57d9 / identity b328](E:/Works/arkret-org/convert/office/target/review-phase11-validation/phase11-canonical-1a639fbead964044a9bff8a90b53c70f/release-identity.json) |

规范化 summary/identity 是引用原始全套、Clippy、源码、环境及 normal artifact 哈希的独立派生证明；原 raw release 的 `raw_release_build_only=true`、`normal_release_acceptance=false`、`full_workspace_accepted=false` 没有改写。两平台源/环境前后相等，完整测试 raw 哈希分别为 Linux `5158610d…2d775a`、Windows `3ab45865…b214df`。 [Linux summary](E:/Works/arkret-org/convert/office/target/review-phase11-validation/native-canonical-byte-export-590a27a1ebb84b3fbb60d78949bb6c3a/workspace-summary.json) · [Linux 原件字节导出](E:/Works/arkret-org/convert/office/target/review-phase11-validation/native-canonical-byte-export-590a27a1ebb84b3fbb60d78949bb6c3a/byte-export-proof.json) · [Windows summary](E:/Works/arkret-org/convert/office/target/review-phase11-validation/phase11-canonical-1a639fbead964044a9bff8a90b53c70f/workspace-summary.json)

**本轮 instrumented D07。** 固定正常 ELF f4 和同一 c04 源码，`20261111T205620Z-f389e576` 实际完成 100,000 次尝试/ACK、21 个持久化检查点、同一 client/document、1 in-flight、零重试。编辑阶段 601.204 秒；原有限 375 秒收敛预算内，在约 210.657 秒观察到历史计数候选、GC pending 与 writing/deleting 均归零，操作日志与 receipt 各保留 2,048。原 30 秒 RSS/冷文档读取、60 秒策略窗口、同进程会话退休重开与最终 REST 均完成。四个 boundary 请求/ACK、同 PID/start、时限与 XML 捕获完整，baseline 在 ready/建文档后；编辑期无周期 malloc_info。这次是加载 observer 的 **instrumented 诊断**，不当作当前源码的无仪表正常 100k 或全应用性能对照。HTTP rate_limit 在该容量夹具中仍禁用，不代表默认生产限流配额吞吐。 [安全规范化结果](E:/Works/arkret-org/convert/office/target/review-phase11-validation/d07-terminal-safe-79932d091ff640e4877e6776aa9fc8be/result.json) · [严格四边界映射](E:/Works/arkret-org/convert/office/target/review-phase11-validation/d07-terminal-safe-79932d091ff640e4877e6776aa9fc8be/boundary-coverage.json)

| /proc RSS 原始阶段 | 字节 |
| --- | ---: |
| 稳定 baseline | 29,794,304 |
| before_leave | 126,152,704 |
| 离开后 30 秒 | 125,837,312 |
| 历史/GC 收敛后 | 125,837,312 |

收敛后较离开前只下降 **315,392 字节**，仍较基线高 **96,043,008 字节**。因此原 D07 内存回落验收仍未满足；没有增设更容易达到的阈值，也没有用少量下降或会话/Weak/GC 结论代替进程内存。原始 result 哈希 `55326798…acf7f8`、execution `f3ee7fe2…192b4d` 保留，安全规范化结果是独立派生文件。终态 collector 的 source/provider、旧 safe exporter、telemetry extractor、allocator parser、coverage、byte transport 六个实际 exit 均为 0，源码仍为 c04，所属 harness/server 已退出，原 result/DB/WAL/SHM 三阶段哈希相等；027 核验只打开新字节副本。采集成功不等于内存验收成功。 [收尾证明 4478d26f…a2119a](E:/Works/arkret-org/convert/office/target/review-phase11-validation/d07-terminal-safe-79932d091ff640e4877e6776aa9fc8be/post-evidence-result.json) · [副本索引核验](E:/Works/arkret-org/convert/office/target/review-phase11-validation/d07-terminal-safe-79932d091ff640e4877e6776aa9fc8be/database-index-proof.json) · [固定字段 telemetry](E:/Works/arkret-org/convert/office/target/review-phase11-validation/d07-terminal-safe-79932d091ff640e4877e6776aa9fc8be/database-telemetry-events.json)

**独立 allocator 判读及下一步。** 四个精确边界的 arena 数为 34→60→60→60，线程数为 34→60→30→31；最后较 baseline 的 allocator-reported `F=fast+rest` 增加 89,868,144 字节，`U=system-current−F` 增加 3,831,952 字节。离开后 U 的部分减少对应 F 增加，而 arena 系统区未回落，支持“已释放给 allocator 的空间仍由 arena 保留”的方向。F 不等于其驻留页数，不能从 RSS 扣除；U 还可含 tcache、元数据、native 分配和碎片，不能称 live Rust heap。XML 与 /proc 的采样不是同步瞬间，也不能排除真实 owner 仍保留一部分内存。数值派生 `225a7892…378b33`、独立结论 `05975435…bef1a` 和原输入哈希均保留。下一候选是检查 SQLite 连接跨 blocking 线程迁移与 arena 数的关系；源码路径可行，但尚未证明它是本轮 RSS 原因，未据此修改 allocator、强制 trim 或宣称修复。 [数值与时序原件](E:/Works/arkret-org/convert/office/target/review-phase11-validation/allocator-phase-analysis-4f53a13bff6240189588fe0b85507555/numeric-provenance.json) · [独立判读](E:/Works/arkret-org/convert/office/target/review-phase11-validation/allocator-phase-analysis-4f53a13bff6240189588fe0b85507555/CONCLUSION.md) · [输入未变证明](E:/Works/arkret-org/convert/office/target/review-phase11-validation/allocator-phase-analysis-4f53a13bff6240189588fe0b85507555/freeze-proof.json)

**当前仍待完成。** 本轮 c04/f5 的 normal 100k、144 浏览器/API、两个 ZIP/冷备和 Garage4 尚未补验；旧 phase10 通过记录继续有效于其旧输入。当前 8b3 远端运行未获 runner，具体启动失败原因未知；取得 annotations 的代理 CLI 查询被自动审批以 `blocked by policy` 拒绝，没有换工具绕过，旧账单 annotation 不能推断当前原因或 readiness。本地 Docker 拒绝范围按上文已核对原记录更正，现有 Desktop 数据盘仍在 C，本轮没有在其上构建；macOS 实际 CI 与 AWS 专用桶仍待环境。这些边界不扩大或缩减原 27 项范围，目标继续保持未完成。 [本轮 annotation 查询拒绝记录](E:/Works/arkret-org/convert/office/target/review-phase11-validation/ci-annotation-query-policy-rejection.json)

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
- Writer、Slides、Whiteboard 和 Calc 遇到保存响应丢失或网关超时时，重新读取一次文档。只有服务器版本推进且内容匹配已提交草稿时才确认保存，验证期间新输入的草稿继续保留。新 Linux WASM 搭配上一轮验收的服务器 ELF，五项新增 Chromium 回归全部通过，无跳过或重试。旧 WASM 的断线负对照显示保存失败。新版首轮 Writer 测试代理重排 JSON 时改变了小数表示，保留原数值格式后通过。 [构建与浏览器记录](E:/Works/arkret-org/convert/office/target/review-phase4-validation/save-recovery-browser/summary.json)
- `24e2b0e` 的 Linux 定点运行先有 88 项通过和 5 项失败，涉及数据库 checkout 超时及测试同步通道提前关闭。用同一测试 ELF 串行复跑失败用例，5 项全部通过，未改测试时限。修正新回归的会话字段名后，鉴权集成回归 4 项全部通过。真实 SQLite 写锁下的 503 记录对象登记阶段，文档内容和版本保持原值。上述 ZIP 和容器结果对应上一轮产物。 [首轮单元结果](E:/Works/arkret-org/convert/office/target/review-phase4-validation/phase4-server-targeted.log) · [同 ELF 串行复跑](E:/Works/arkret-org/convert/office/target/review-phase4-validation/phase4-server-targeted-serial-existing-elf.log) · [鉴权与写锁回归](E:/Works/arkret-org/convert/office/target/review-phase4-validation/phase4-auth-capacity-rerun.log)
- 对象预留过期后，维护任务按 SQLite 租约索引分页查找登记项，包括物理存储中不存在的对象。空页只读，候选在写事务中重新检查租约、引用及 GC 队列容量，再原子标记删除并入队。Linux 定点测试验证了无实体预留、活动租约保护，以及 S3 协议夹具中迟到 PUT 经物理扫描和宽限期再次删除。 [回收实现](E:/Works/arkret-org/convert/office/crates/server/src/services/maintenance.rs:184) · [分页查询计划](E:/Works/arkret-org/convert/office/target/review-phase3-validation/expired-write-sql-probe.json)
- REST 保存的 WebSocket 快照和更新 webhook 由最终事务的提交信号触发。HTTP 返回数据库操作超时后，保存任务仍等待可能在执行的事务。提交成功才发布，协作会话仍拒绝较旧的版本。该机制在进程内运行，进程中断后的即时广播和 webhook 投递没有新增持久队列。Linux 定点用例在调用方先超时的情况下验证了实际事务提交后才发出信号。 [提交时序回归](E:/Works/arkret-org/convert/office/crates/server/src/services/persistence.rs:1146) · [定点运行](E:/Works/arkret-org/convert/office/target/review-phase4-validation/phase4-server-final-targeted.log)
- 同次定点运行单元测试 94 通过、1 失败，鉴权集成 4 项全部通过。唯一失败发生在容量测试冷连接预热，尚未进入队列断言。测试单独用 5 秒建立空闲连接，池满时仍验证原有 1 秒生产 getter 的 checkout 超时及拒绝行为。后续 Linux 全量执行时，服务端单测 87 通过、8 失败，其余测试目标通过。失败期间 WSL 同期采样记录了持续 I/O 停滞，一项启动断言明确停在 WAL 初始化。I/O 回稳后，同一服务端单测 ELF 串行 95 项全部通过。保留两次结果，采样无法确定每项失败的原因。Windows 全目标检查、容量复测和新版发布产物仍待验收。 [首轮全量结果](E:/Works/arkret-org/convert/office/target/review-phase4-validation/phase4-workspace.log) · [I/O 采样](E:/Works/arkret-org/convert/office/target/review-phase4-validation/phase4-workspace-io-observation.log) · [同 ELF 复跑](E:/Works/arkret-org/convert/office/target/review-phase4-validation/phase4-workspace-server-elf-rerun.log)
- 中文 README 的格式化命令与英文说明和 CI 对齐。项目 workspace 的 `cargo fmt -- --check` 已通过，第三方源码工作区的 `--all` 会尝试解析本项目未使用的 Apace 清单。
- 同一构建产物在 I/O 回稳后完成整套 Linux workspace 复跑，**565 通过、0 失败、4 忽略**，首轮为 557 通过、8 失败、4 忽略。忽略项包括显式容量基准和需要独立 S3 服务的用例。严格 Clippy、最新 release 与 141 项联合 HTTP/浏览器验收随后执行。 [Linux 全量复跑](E:/Works/arkret-org/convert/office/target/review-phase4-validation/phase4-workspace-quiet-rerun.log)
- Linux 原生 workspace 全目标和 `wasm32-unknown-unknown` 前端的严格 Clippy 均已通过。Windows 本机以当前源码重新执行 workspace 全目标 `cargo check`，退出码 0，覆盖测试目标的编译检查。Windows 运行测试仍以已验收的上一轮产物为历史证据。 [Linux Clippy](E:/Works/arkret-org/convert/office/target/review-phase4-validation/phase4-clippy-workspace.log) · [WASM Clippy](E:/Works/arkret-org/convert/office/target/review-phase4-validation/phase4-clippy-wasm.log) · [Windows 全目标检查](E:/Works/arkret-org/convert/office/target/review-phase4-validation/windows-workspace-check.log)
- 最新 Linux 原生 release 由 Rust 1.98.1 构建，服务端 SHA-256 为 `69b0cbdd797057c2a82e30ca1686bfdace094d8789624ccc544166a3b4bbbd57`。使用该 ELF 和新版 WASM 启动独立实例，**141 项 HTTP 与 Chromium 用例全部通过**，无失败、跳过、重试或不稳定用例。二进制与源码清单身份均已记录。 [构建与哈希](E:/Works/arkret-org/convert/office/target/review-phase4-validation/phase4-release-identity.log) · [联合运行](E:/Works/arkret-org/convert/office/target/review-phase4-validation/phase4-native-e2e-result.json)
- 上述 ELF 对应的 Linux ZIP 有 54 个文件，Web、配置、迁移和可执行文件均按哈希核对，ELF 权限为 `755`。脱离源码目录运行后，实际 Chromium、REST 与 WebSocket 的强停重启、ACK 与 receipt 重放均通过。本地冷备恢复再次校验 SQLite/WAL 和两类对象，缺失或损坏副本会被拒绝。ZIP SHA-256 为 `a7bc4f14799e54f38642d15f8d5e1893a756f74bef311927635287d7e7a687a0`。 [Linux ZIP](E:/Works/arkret-org/convert/office/target/dist/contrix-office-x86_64-unknown-linux-gnu-phase4.zip) · [验收记录](E:/Works/arkret-org/convert/office/target/review-phase4-validation/linux-package/phase4-package-7726395451094913beb074183223fa47/summary.json)
- 使用当前 workspace 的实际测试 ELF，在新启动的独立 Garage 2.5.0 服务上通过 3 项真实 S3 兼容验收，覆盖签名、编码键与二进制读删、分页回收和数据库共享桶隔离。独立测试凭据在清理后删除。 [Garage 实测](E:/Works/arkret-org/convert/office/target/review-phase4-validation/garage/result.json)
- Windows 原生 release SHA-256 为 `439f10b38cc6eac5a38d19986ad43250cef0a856a5bd8e5ed9fc88d149d4e572`。首轮包装器选中两个 PowerShell 路径，打包调用未启动。修正选择后保留同一可执行文件继续打包，真实 Chromium、REST/WS 强停恢复、ACK 和 receipt 重放及本地冷备均通过。新版 Windows ZIP SHA-256 为 `2da212e3dfd264598961e698b2aa5f77c9469428e47539bc4dc7dab4dfebb310`。 [Windows ZIP](E:/Works/arkret-org/convert/office/target/dist/contrix-office-x86_64-pc-windows-msvc-phase4.zip) · [打包验收](E:/Works/arkret-org/convert/office/target/review-phase4-validation/windows-release-8cf724d544f540acb9cd19ec80360517/result.json) · [冷备恢复](E:/Works/arkret-org/convert/office/target/disaster-recovery/ebeb838423174fd783fd9f97c1c50e3d/result.json)
- 在专用 D 盘 WSL2 rootless builder 上重新验证数据盘、实际挂载、Docker、BuildKit、containerd 和测试卷后，完成当前 `linux/amd64` 镜像构建与实际运行。镜像 `sha256:6032284ad770f6cb054bfd6ddad368e9530d5a2f0976ad29f2ff4537c8428e2e` 使用非 root 用户，真实 Chromium、REST 和 WebSocket 强停重启、ACK 及操作重放均通过。归档 SHA-256 为 `e5f4bd11d9f14be64b6762e008c64485c84bd254eaf52f582556edf896960239`，配置和六层摘要已核验，测试容器和卷清空后停止专用发行版。 [D 盘构建证明](E:/Works/arkret-org/convert/office/target/review-phase4-container/Build-storage-proof.json) · [镜像烟测](E:/Works/arkret-org/convert/office/target/package-smoke/f48d2853a42f499895dc7fc3d709c110/result.json) · [镜像归档](D:/Works/arkret-org/office-container-validation-20261111/contrix-office-linux-amd64-20261111-phase4.tar.gz) · [归档校验](E:/Works/arkret-org/convert/office/target/office-container-artifact-proof-phase4.json)

两个原生 release ELF 使用同一 Rust 1.98.1、相同配置和迁移。每个并发组以旧、新、新、旧顺序运行，每 worker 100 次请求，90% GET、10% CAS PUT，零自动重试。普通混合负载结束后，再运行 30 次大文档导出与另一文档读取。各轮 WSL I/O 完全停滞比例最高为 **0.1055%**。

| 并发 | 旧版两轮成功数 | 新版两轮成功数 | 旧版成功 req/s | 新版成功 req/s |
| --- | --- | --- | --- | --- |
| 2 | 200/200、200/200 | 200/200、200/200 | 70.11、77.00 | 78.44、77.85 |
| 8 | 800/800、799/800 | 800/800、799/800 | 72.21、70.97 | 71.42、71.24 |

8 并发两版本各一次 PUT 429，新版请求 `528df8e8-af36-4b83-b66f-6b7a1ba19902` 对应 `document_save_capacity / capacity_exhausted`，拒绝前 4 次租约和 4 条 SQL 均已完成，SQL 共 118 微秒。旧版返回相同保存容量错误。八轮未出现 503、数据库操作超时或启动超时，历史超时证据继续保留。现有样本不足以确认 8 并发吞吐有明确改善。 [同工具链 ABBA 数据与请求关联](E:/Works/arkret-org/convert/office/target/review-phase3-validation/performance/phase4-abba-20261110T191449Z-summary.json) · [原始证据哈希](E:/Works/arkret-org/convert/office/target/review-phase3-validation/performance/phase4-abba-20261110T191449Z-raw/export-manifest.json)

### 原始验收补证

- **S01** 在真实 loopback HTTP 监听器上分别直连和通过透传 TCP 反向代理发出原始路径。点段、反斜杠、盘符前缀和单双层编码路径均无法读取根目录外的标记文件。缺失资产仍返回 404，合法资产和前端路由正常。已有 symlink 与 junction 覆盖保留。 [代理回归](E:/Works/arkret-org/convert/office/crates/server/src/handlers/spa.rs:121) · [Linux 定点结果](E:/Works/arkret-org/convert/office/target/review-phase4-validation/phase4-additional-regressions.log)
- **D04** 用同一 Linux ZIP 实际强停两次。首次在确认首块后强停，重启后禁止同索引异内容覆盖，补第二块且无需重发首块。两个并发完成返回同一文件 ID，仅计 20,209 字节。第二次强停后完成重放仍返回同一 ID，下载字节和配额保持一致。共享 ZIP/镜像脚本已纳入同一强停边界的续传断言，并在 Windows ZIP 上实际通过，完成与重放的计费均为 20,209 字节。 [两次强停实测](E:/Works/arkret-org/convert/office/target/review-phase4-validation/upload-restart/9877d55af484453e85943fa98fc1b38a/result.json) · [共享验收](E:/Works/arkret-org/convert/office/scripts/test-package.ps1:169) · [Windows 续传烟测](E:/Works/arkret-org/convert/office/target/package-smoke/639822b170cf4fb58daa9e6ae53e030e/result.json)
- **D06** 真实 WebSocket 连接在 1,100 条广播消息越过生产 channel 容量后，从数据库恢复原版本和原内容，随后仍能接收下一次 REST 提交。测试用额外订阅者确认实际发生 `Lagged`，并用同版本快照区分周期刷新。 [广播溢出回归](E:/Works/arkret-org/convert/office/crates/server/tests/messaging_regressions.rs:576) · [Linux 定点结果](E:/Works/arkret-org/convert/office/target/review-phase4-validation/phase4-additional-regressions.log)
- **D07** 独立原生 release 长会话在 55,474 次成功 ACK 后，第 55,475 次操作因对象清理队列积压被拒绝，100,000 次验收未通过。只读 WAL 回放确认版本为 55,475、收据高水为 55,474，失败操作未持久化。末态 GC 恰好 1,024 条，全未认领、无仍被引用的对象，抽样对象仍存在，未见删除失败日志。清理进程的二次等待超时掩盖了初始错误，原始 TSV、失败帧与 WAL 交叉恢复结果已单独留存。 [安全长测记录](E:/Works/arkret-org/convert/office/target/review-phase3-validation/performance/phase4-d07-20261110T215000Z-raw/result.json) · [只读取证](E:/Works/arkret-org/convert/office/target/review-phase3-validation/performance/phase4-d07-20261110T215000Z-failure-forensics.json)
- 长测中操作日志和收据始终各不超过 2,048 条。成功 ACK 的总体 p50/p95/p99 为 8.595/17.023/45.645 ms，另一文档 GET 的 p95/p99 为 4.340/12.243 ms，RSS 峰值 59.04 MiB。运行期间有较高 I/O 停滞，时序不足以将延迟变化归因于历史长度。版本快照和墓碑持续增长，未完成自愿离开后的回落验证。上述结果只覆盖这次固定参数的单次运行。

### 后续修正与待验收项

- 对象回收改为小批事务认领、有界并行物理删除和批量完成。新到达的清理提示跨 worker 退出边界保留，已有 1,024 条写入门禁继续生效。失败删除保留在队列，同批成功对象仍可完成。新 GC 与历史快路径已通过默认完整回归，10 万次长测仍待执行。
- 原生文档历史增加计数积压追赶任务，按实体轮换和配置批量删除。事务内重新确认 newest-N 与保守 OT 保护窗口，空页只读，积压时每 250 毫秒追赶一页，空轮恢复原配置周期。空闲后出现新热点，可等待一个配置周期才重新检查，默认最多约 300 秒。现有完整年龄、重复快照和文件版本策略继续由原维护路径处理。
- **F02** 已加入同源公式 Worker，大工作量按文档修订计算，编辑、取消和十秒超时会终止旧 Worker。正常保存等待当前修订结果，降级保留原始公式和输入。83 项前端原生单测、WASM 严格 Clippy、7 项定点浏览器用例通过。新版 Worker Web 搭配既有已验收后端完成全量 **143/143** HTTP 与 Chromium 回归，无跳过或重试。随后补充包流程中的实际 Worker 与页面退出测试，已随下述新版后端再验；新发行包仍需独立验收。 [Worker 定点](E:/Works/arkret-org/convert/office/target/review-phase4-validation/worker/targeted-browser-result.json) · [143 项结果](E:/Works/arkret-org/convert/office/target/review-phase4-validation/worker-full/633f957efe36451389d2155e6b3ef4bf/result.json)
- **F03** 转换 worker 的容量边界提取为同一实现并补取消回归，验证调用方取消时实际 blocking 工作仍持有槽位，以及工作退场、预算拒绝后能再次获取容量。新增取消与预算释放回归已在本轮服务端单测实际通过。
- 本轮 Linux 定点联合运行 451 通过、4 失败，涵盖六个库、GC 和消息集成；四项失败为数据库或网络等待超时。同期曾记录持续 I/O 停滞和 `HCS_E_CONNECTION_TIMEOUT`，不能据此确定每项失败的原因。环境回稳后，同一服务端单测 ELF 串行复跑 **99/99 通过**。新增 GC 集成 **4/4**、消息集成 **16/16** 在首轮已通过，原生历史快路径的三项单测也通过。随后完整 workspace 首试在服务端 95 通过、4 失败后停止，失败涉及 checkout、worker 启动信号及清理等待；原始记录保留。环境回稳后，完整套件复跑 **575 通过、0 失败、4 默认忽略**，覆盖相同服务端 ELF 的 99 项单测及全部默认集成测试。Linux/Windows 严格原生 Clippy 与 WASM 严格 Clippy 均通过，新版 release、长测及新交付产物验收继续进行。 [完整首试](E:/Works/arkret-org/convert/office/target/review-phase4-validation/phase5-workspace-full-first.log) · [575 项完整复跑](E:/Works/arkret-org/convert/office/target/review-phase4-validation/phase5-workspace-quiet-rerun.log) · [Linux Clippy](E:/Works/arkret-org/convert/office/target/review-phase4-validation/phase5-clippy-workspace.log) · [Windows Clippy](E:/Works/arkret-org/convert/office/target/review-phase4-validation/windows-phase5-final-clippy.log) · [WASM Clippy](E:/Works/arkret-org/convert/office/target/review-phase4-validation/phase5-final-clippy-wasm.log)Windows workspace 全目标 `cargo check` 已通过。随后将仅供 WASM 与测试调用的 `workbook_cells` 按目标条件编译，新一轮全目标检查零警告通过；本轮完整回归与严格 Clippy 结果如上。 [Windows 修正后检查](E:/Works/arkret-org/convert/office/target/review-phase4-validation/windows-phase5-final-check.log) [首轮完整记录](E:/Works/arkret-org/convert/office/target/review-phase4-validation/phase5-server-regressions-complete.log) · [同 ELF 99 项复跑](E:/Works/arkret-org/convert/office/target/review-phase4-validation/phase5-server-unit-same-elf-rerun.log) · [Windows 检查](E:/Works/arkret-org/convert/office/target/review-phase4-validation/windows-phase5-check.log)
- 新增 Worker 与包流程三项用例最终 **3/3 通过**，9.1 秒，零重试、跳过或不稳定用例。覆盖真实 Worker 求值、Dashboard 导航关闭、替换/取消/超时、降级保存及 Writer/Calc 保存重载。首轮两项环境超时和次轮测试同步失败记录均保留；修正了可见导航链接、选中单元格后等待公式栏更新，以及登录跳转后读取持久化凭证的 fixture，产品代码未随复跑改动。此轮沿用既有后端，随后已在下述新版后端完整套件中联合通过，新发布包仍待独立验收。 [最终三项结果](E:/Works/arkret-org/convert/office/target/review-phase4-validation/worker-package/2475ef3dfec14d5c9755ff53e1f5356a/result.json) · [观察超时记录](E:/Works/arkret-org/convert/office/target/review-phase4-validation/worker-package-wsl-timeout.json) · [首轮失败原件](E:/Works/arkret-org/convert/office/target/review-phase4-validation/worker-package/cf120dc8b3e6467192405be3b50eb4b8/result.json)

### 新版后端与 Worker 联合验收

上述修正的 Linux 原生 release 已实际构建成功，ELF SHA-256 为 `8cfa8f41f44d1fdbdda23f72fc62299a1a8a67c3189e3224fa6d417208f72220`，WASM 为 `b30ef73ff6b0f5aa57219095392d2a9875fc28bdc5f53153159a9f243626a4ad`。正常 Cargo target 路径由本 workspace 的 metadata 核对，未重定向 Cargo home 或临时目录。 [构建与身份](E:/Works/arkret-org/convert/office/target/review-phase4-validation/phase5-release-identity.log) · [最终原生 Clippy](E:/Works/arkret-org/convert/office/target/review-phase4-validation/phase5-final-clippy-workspace.log)

使用该 ELF 与独立组装的 `phase5-final-web` 完成 **143/143** HTTP/Chromium 联合回归，耗时 127.36 秒，零重试、跳过、失败或不稳定用例。完整源及两套子模块工作树共 2,487 个文件逐字节匹配，运行前后源码、ELF 和全部 21 个 Web 资产未变，包含 Worker JS。应用和 Playwright 进程均已退出，正常环境未变。首轮只读 preflight 被 Git ownership 检查拒绝，随后仅对各次只读 Git 调用传入该仓库的精确 `safe.directory`，未改全局配置；失败原件保留。59 个导出证据文件已与原件逐个核对 SHA-256。 [联合运行](E:/Works/arkret-org/convert/office/target/review-phase5-validation/e2e/b77986ff9dd64c618029b9f2a83a5663/acceptance/result.json) · [完整源码与前后校验](E:/Works/arkret-org/convert/office/target/review-phase5-validation/e2e/b77986ff9dd64c618029b9f2a83a5663/result.json) · [安全导出核对](E:/Works/arkret-org/convert/office/target/review-phase5-validation/e2e/b77986ff9dd64c618029b9f2a83a5663/export-proof.json)

本轮 575 项默认测试中的三个 S3 环境忽略项，已使用本轮实际测试 ELF 在独立 Garage 2.5.0 服务上另行通过 **3/3**，覆盖真实签名、编码键和二进制读删，以及新回收路径的分页和共享桶隔离。测试服务退出、临时凭据删除，未导出运行数据库。另一个默认忽略项为显式 release 算法容量基准，本轮待独立执行。 [新版 S3 兼容验收](E:/Works/arkret-org/convert/office/target/review-phase5-validation/garage/result.json)

新版 Linux ZIP 已通过独立组包、真实浏览器 Worker/D04 分块续传、REST/WS 强停恢复与冷备恢复。归档有 55 个文件，ELF 权限为 `755`，全部组包输入哈希匹配，SHA-256 为 `fcb6d23fa37822046561ecf56ce1a51d632afc227df23ec9ba6cccef631fad3f`。测试进程均已退出，原 canonical ZIP/Web 在 finally 中恢复，phase4 产物未覆盖。 [新版 Linux ZIP](E:/Works/arkret-org/convert/office/target/dist/contrix-office-x86_64-unknown-linux-gnu-phase5.zip) · [发布包与冷备验收](E:/Works/arkret-org/convert/office/target/review-phase5-validation/linux-package/phase5-package-30401fb549664359a28f176a70445d48/summary.json)

新版 Docker 镜像 `sha256:de55c66cd4ef4b0315c415f11423327ecb91b50ab59a7d450a958ea831bd48cb` 已在专用 D 盘 builder 构建成功。首轮共享烟测在 Dashboard 导航收到 429 JSON，尚未进入 Calc Worker 步骤。随后两组同源码的原生生产配置诊断确认是产品作用域缺陷：全新实例仅运行正常浏览器旅程也会触发 429。A 在该窗口共记录 105 个真实响应，其中 API 34、页面与资源 71；B 在共享烟测重启后的窗口为 API 45、页面与资源 60，同样合计 105，复现原 Docker 的 Dashboard 失败位置。两组均在放行 100 次后开始拒绝，配置与冻结 Docker 上下文相同，诊断进程已清理。这些是被失败截断的前缀，不能代替完整旅程验收。已修改路由，让 API 与页面资源各自使用独立 IP 桶，保留用户桶和原有 100/60 默认值；仍需验证完整生产流程及超限保护。 [A/B 原始计数与安全 trace](E:/Works/arkret-org/convert/office/target/review-phase5-validation/rate-limit-diagnostics/summary.json) [首轮浏览器记录](E:/Works/arkret-org/convert/office/target/package-smoke/87f89af9fb344e309f908738dd8fd1f5/logs/browser.stdout.log)

新版 Windows release 在正常项目 target 下构建成功，耗时 13 分 21 秒，EXE SHA-256 为 `aac40b1d1ab53965ae901f1cf61b80a6a6844b81c13fa39de666438dd73f203d`。新版 ZIP 的真实 Worker 浏览器流程、REST/WS 强停恢复、ACK/receipt 重放和 D04 分块续传均通过，续传完成与重放均只计费 20,209 字节；独立冷备恢复再次确认 WAL 必需、完整性及备份未变。ZIP SHA-256 为 `570b07d819cd449db53eddaad3426399ecb33535fff39a818baf7cfb5c0d0dea`，旧 canonical ZIP 已按原哈希恢复。 [Windows 新包](E:/Works/arkret-org/convert/office/target/dist/contrix-office-x86_64-pc-windows-msvc-phase5.zip) · [新包验收](E:/Works/arkret-org/convert/office/target/review-phase5-validation/windows-package-2fbf270ebfd844a3900884be8b409776/result.json) · [冷备恢复](E:/Works/arkret-org/convert/office/target/disaster-recovery/4d99fbf91a804b0faf0aa4d505ec2295/result.json)

为了定位 Docker 的 429，创建新诊断容器/卷的命令被工具自动审批以 `blocked by policy` 拒绝，未提供具体原因，命令未执行且未创建资源。没有换壳重发同一操作。当前以同源码的原生 production 实例继续做独立请求计数诊断；它不能替代 Docker 运行验收。 [拒绝记录与动作骨架](E:/Works/arkret-org/convert/office/target/review-phase5-container/Diagnostic-A-create-rejection.txt)

新增 5 项完整路由、真实 TCP 的限流回归，涵盖双向预算隔离、各桶超限、静态路由共享、JWT 用户预算和默认忽略伪造转发 IP。首轮旧路由执行五项均在数据库 fixture 初始化 checkout 超时，未进入 HTTP，不能作为限流行为红测。原记录保留；fixture 随后改用实际生产启动初始化 API，并显式等待服务与数据库清理、验证临时目录删除，沿用原有时限。修正后的五项 TCP 回归 **5/5 通过**，其中显式超限与伪造转发地址拒绝均保留，服务和数据库清理及临时目录删除验证完成。新路由的 Windows 本机编译检查亦退出码 0。已固定一个调试 ELF 供完整 production 浏览器旅程验证，其 SHA-256 为 `54c0be4f4efbcd3f95fcd18d4ed5842a3c7e0cac0d1733d2e7d4468ccb734be3`；它不是已发布的 phase5 release，后续仍需全套回归、优化构建及新产物验收。 [修正后五项结果](E:/Works/arkret-org/convert/office/target/review-phase5-validation/phase6-rate-limit-scope-tests.log) · [Windows 路由编译](E:/Works/arkret-org/convert/office/target/review-phase5-validation/phase6-scope-windows-check.log) [回归源码](E:/Works/arkret-org/convert/office/crates/server/tests/rate_limit_regressions.rs) · [首轮 fixture 失败](E:/Works/arkret-org/convert/office/target/review-phase5-validation/phase6-rate-limit-baseline-first.log)

两桶方案的完整 production 旅程仍失败：API 43 次、公共资源 103 次，公共第 101 次请求是最终 Calc 重载所需的 WASM，返回 429。全流程约 7 秒，单窗口证据明确，未靠等待重置获得额度。候选、源码、资源后验一致，进程已清理，B 组按失败即停没有启动。 [两桶候选失败与分类](E:/Works/arkret-org/convert/office/target/review-phase5-validation/rate-scope-diagnostics/api-page-separate-v1-A-d397dfcdc9b74ff4b05f2bfc75b89a47/classification-summary.json)

后续修正为 API、`/assets/`、页面与外壳三类独立 IP 桶，仍各自使用原配置的 100/60，JWT 用户桶保留。IP 规则检查移到各 edge limiter 后；超过额度时 429 优先于 IP 403/规则查询 503，未超限仍失败关闭。当前顺序可在过限时完全避免该次规则查询。新增回归扩展为 8 项，Linux **8/8 通过**，明确验证三类隔离、各类拒绝、不增加 checkout、各类 IP 403/规则查询 503，以及原有用户/伪造 IP 保护。Windows 编译检查通过，真实 Windows TCP 回归亦 **8/8 通过**（23.94 秒），包含临时数据库实际删除验证。 [Windows 八项结果](E:/Works/arkret-org/convert/office/target/review-phase5-validation/phase6-three-scope-windows-tests.log)新的调试 ELF `5f06f968…512f66cf` 已冻结供完整 A/B 旅程复验，尚未构成新版 release 或 Docker 验收。 [八项结果](E:/Works/arkret-org/convert/office/target/review-phase5-validation/phase6-three-scope-tests.log) · [Windows 三类编译](E:/Works/arkret-org/convert/office/target/review-phase5-validation/phase6-three-scope-windows-check.log)
三类方案的完整 production A 组已通过：API 50、资产 87、页面外壳 20，零 429。各类全部请求明确位于首个 60 秒窗口内，原 Writer/Calc 保存、重载、Worker 断言完整通过，候选/源/资源前后未变，进程均退出。共享 B 组随后因 SQLite WAL 初始化 25,037 毫秒超过既有 15 秒启动时限而退出，零业务请求、零 429；保留失败，没有自动重试。诊断 adapter 将省去一次额外的 1.69 GB 调试 ELF 复制，改为在私有 package 工作目录直接执行冻结候选并额外核验实际 executable 身份；这只用于诊断，不作为 ZIP 验收。待 I/O 回稳后另建 B 复验，不增加生产时限。 [三类 A 成功与 B 启动失败](E:/Works/arkret-org/convert/office/target/review-phase5-validation/rate-scope-diagnostics/api-assets-shell-v2-summary.json)
I/O 回稳且 Windows 八项测试结束后，采用直启冻结候选的全新 B 组完整通过。重启后 API 67、资产 87、页面外壳 20，零 429，全部明确处于各类首个 60 秒窗口。两轮实际 executable 路径及 SHA-256 均为冻结的 `5f06f968…512f66cf`，CWD 为私有 package，所有原业务/续传/计费/强停/真实浏览器断言保留，原始 archive 内的 `8cfa…` 与实际运行候选分开记录。200 条真实响应包含预期的冲突 409；没有连接拒绝或自动重试。进程与源/资产后验均确认。此前 B 的 WAL 启动失败继续保留。本次只证明原生调试候选的完整生产限流功能，不冒充新版 ZIP 或 Docker 验收。 [A/B 最终完整证据](E:/Works/arkret-org/convert/office/target/review-phase5-validation/rate-scope-diagnostics/api-assets-shell-v2-completed-summary.json)
三类修正后的完整 workspace 首次联合运行终态为 578 通过、5 失败、4 默认忽略。仅服务端库单测失败（94/99），具体为 checkout 超时、worker 信号关闭及清理等待；146 项 API、鉴权容量、GC、消息、持久化、安全、传输与新增八项限流目标均通过。同期持续较高 I/O 停滞记录保留，不能将每个失败分别归因。完整原件已单独保存；I/O 回稳后正在以相同源码和已编译产物重新运行全套，尚未据首试放行发布构建。 [首试完整结果](E:/Works/arkret-org/convert/office/target/review-phase6-validation/phase6-final-workspace-first-failure.log) · [首试计数](E:/Works/arkret-org/convert/office/target/review-phase6-validation/phase6-final-workspace-first-summary.json)

同源码第二次完整运行终态为 579 通过、4 失败、4 默认忽略，库单测 99/99 已通过。四项失败分别属于 API、消息和持久化目标，均停在公共 fixture 的普通 checkout 初始化，尚未进入业务断言。现将该唯一初始化入口改为生产 `run_startup_migrations`，保持池及请求 checkout/operation/busy 参数不变，并将编译链接与运行分开，待 I/O 稳定再执行。 [第二次完整原件](E:/Works/arkret-org/convert/office/target/review-phase6-validation/phase6-final-workspace-second-failure.log) · [第二次计数](E:/Works/arkret-org/convert/office/target/review-phase6-validation/phase6-final-workspace-second-summary.json)

逐条对照原始验收还发现五项明确要求缺乏独立自动断言，现已补齐并完成编译：D01 在双 PUT 到达且数据库仍 v1 时由屏障放行；D02 对 files/documents/quota 分别注入失败并验证完整回滚、对象清理和移除故障后的正控；D03 真实 S3 高层同名并发保存、精确读回及删除隔离；D05 固定 SQL 写竞争的过期锁接管和旧 token 拒绝；F03 对同格数的近端/最大坐标工作簿比较实际访问计数。新增 `workbook_to_xlsx_with_stats` 与普通导出复用一个实现，在实际 cell 遍历/查找/发出位置记录事件；Serde 副本遍历、排序、压缩及分配不在计数范围，不能解释为总 CPU 或内存 SLO。真实 S3 新用例独立默认忽略，需另行在 Garage 上执行，不算默认套件已通过。

上述改动的完整 Linux workspace 运行结果为 **587 通过、1 失败、5 忽略**。持久化 17 项、限流 8 项及服务端库 99 项均通过。唯一失败是新增图表回归错误地期待 `numCache`：该夹具未配置 `series_sources`，现有导出器合法输出 `numLit/strLit`。仅修正测试断言，明确验证三点数值、标签、数量与索引，保留访问计数、pivot 和重新导入检查；随后独立导出回归 **5/5 通过**。首轮失败原件保留；修正后的完整 workspace、严格 Clippy、新 release/发布包、真实 S3 新用例及容量长测仍待完成，不把定点通过等同整套通过。 [完整运行原件](E:/Works/arkret-org/convert/office/target/review-phase6-validation/phase6-completion-first-failure.log) · [修正后导出回归](E:/Works/arkret-org/convert/office/target/review-phase6-validation/phase6-xlsx-count-regressions.log)

容量 HTTP 与 D07 harness 均显式关闭 HTTP rate limiter，以测量底层数据库与业务准入；两版本的条件相同。其 req/s 不能解释为默认生产 100/60 配额的放行吞吐。生产配额的证据是独立 TCP 与上述单窗口 A/B，二者不能混用。

### `2ae0faa` 提交后的整体验收

修正图表断言后，以同一源码完成整套 Linux workspace：**588 通过、0 失败、5 忽略**，共 31 个测试目标结果，未把定点复跑累加进总数。严格 workspace/all-targets Clippy（`--no-deps`、`-D warnings`）退出码 0，耗时 48.44 秒。正常 release 构建退出码 0，耗时 3 分 14 秒，ELF SHA-256 为 `b3199e5ce0d32a5a7ae44b5e5f75bcda8c3c09d9c37653a1537daeb9a4fa1c94`，已在实际 metadata 的 target 内保存独立只读副本，供后续验收使用。后续 opt-in 测试构建可能改变 canonical Cargo 二进制，不能只凭其路径认定身份。 [全量计数与身份](E:/Works/arkret-org/convert/office/target/review-phase6-validation/phase6-final-workspace-summary.json) · [全量原件](E:/Works/arkret-org/convert/office/target/review-phase6-validation/phase6-final-workspace.log) · [严格 Clippy](E:/Works/arkret-org/convert/office/target/review-phase6-validation/phase6-final-clippy.log) · [release 构建](E:/Works/arkret-org/convert/office/target/review-phase6-validation/phase6-final-release-build.log) · [固定产物身份](E:/Works/arkret-org/convert/office/target/review-phase6-validation/phase6-final-frozen-release.json)

默认忽略的四项 S3 环境用例随后使用上述完整运行所记录的实际测试 ELF，在全新独立 Garage 2.5.0 服务上 **4/4 通过**。其中新增 D03 用例实际调用高层 `save_file` 同名并发保存，验证不同对象键、精确读回、删除隔离和对象清理；原有签名/编码键/二进制及两项维护回归保留。服务已退出、临时凭据已删除，未导出运行数据库。此结果证明真实 S3 兼容服务行为，仍不代表 AWS 专用桶验收。 [四项结果与清理](E:/Works/arkret-org/convert/office/target/review-phase6-validation/garage/result.json) · [D03 及协议用例](E:/Works/arkret-org/convert/office/target/review-phase6-validation/garage/real_s3_acceptance.log) · [维护用例](E:/Works/arkret-org/convert/office/target/review-phase6-validation/garage/maintenance_regressions.log)

上述固定 ELF 与原有最终 Web 完成 **143/143** HTTP/Chromium 回归，耗时 127.774 秒，零重试、跳过或不稳定用例。递归受跟踪源码共 2,488 个文件、ELF 和 21 个 Web 资产在运行前后保持一致，测试进程均确认退出。两次先前预检因 WSL Git 读取 E 盘工作区状态超过 30 秒而失败，均未启动应用；改为宿主 Windows Git 精确读取各仓库元数据，保留原 30 秒时限、完整字节校验及每仓库 HEAD/index/status，不改 Git 配置。 [完整结果](E:/Works/arkret-org/convert/office/target/review-phase6-validation/e2e/7726b56224734db68e0830297a1e281b/result.json) · [143 项内层结果](E:/Works/arkret-org/convert/office/target/review-phase6-validation/e2e/7726b56224734db68e0830297a1e281b/acceptance/result.json)

Windows 正常 release 同样构建成功，耗时 10 分 53 秒，EXE SHA-256 为 `a02906e1149e12da9c2dd606031a00cc7046d49e56cff46ac052e5ffd4711881`。两平台新 ZIP 均通过完整资产哈希、真实浏览器 Worker、REST/WS 强停恢复、ACK/receipt 重放、D04 分块续传及独立冷备恢复；原 canonical ZIP/Web 已恢复且测试进程均清理。Linux ZIP 共 55 文件，ELF 权限 `755`，直接使用固定 `b319…` 输入，未覆盖 Cargo 当前产物；输入适配器的原脚本、完整差异及哈希随证据保留。 [Windows 构建身份](E:/Works/arkret-org/convert/office/target/review-phase6-validation/windows-release-80630ad23c334f029227e85b9fabd132/result.json) · [Linux 新 ZIP](E:/Works/arkret-org/convert/office/target/dist/contrix-office-x86_64-unknown-linux-gnu-phase6.zip) · [Linux 包验收](E:/Works/arkret-org/convert/office/target/review-phase6-validation/linux-package/phase6-package-8b55456445484aadb08891d644da2c63/summary.json) · [Windows 新 ZIP](E:/Works/arkret-org/convert/office/target/dist/contrix-office-x86_64-pc-windows-msvc-phase6.zip) · [Windows 包验收](E:/Works/arkret-org/convert/office/target/review-phase6-validation/windows-package-9ceac94ebbb94fafb30d362113416de0/result.json)

第五个默认忽略项的 release 算法基准随后实际通过：5 场景各 30 样本，整体 3.008 秒；测量期间 full I/O stall 累计增量为 0。同为 1,001 格的近端与末行末列 XLSX 均实际访问源单元格 7,007 次、发出 1,001 个 `<c>`，p95 分别 30.783/30.053ms。这是所定义遍历事件与本次耗时的证据，不是总 CPU、内存或浏览器时延保证。 [算法原始输出](E:/Works/arkret-org/convert/office/target/review-phase6-validation/performance/final-20261111T015948Z-994e8e4b/algorithm.raw.log)

同 Rust 1.98.1 的固定旧版 `69b0…` 与新版 `b319…` 按 ABBA 完成全部 8 场容量对照。每场保留原混合流、30 次热导出及独立冷文档采样；每请求无重试，各场 full I/O stall 均低于既定 1% 比较门槛。2 并发四场全为 200/200。8 并发依次为旧 800/800、新 799/800、新 799/800、旧 799/800；三个失败均为 GET 503，按 request ID 分别确认 `ip_filter_rules`、`session_identity` 和 `documents` 的准入容量耗尽，所有 PUT 成功，无 429 或操作超时。原测量退出码为 1，不能称为零拒绝验收，也不能据此承诺稳定性能提升；8 个实例均确认退出。 [完整对照与失败归因](E:/Works/arkret-org/convert/office/target/review-phase6-validation/performance/final-20261111T015948Z-994e8e4b/abba.summary.json)

**D07 完整长测仍未通过。** 在构建、浏览器及组包工作结束后，固定新版 ELF 的独占 10 万次测试于 7,589 次 ACK 后失败，第 7,590 次收到 `Database error: database operation timed out; cleanup remains admitted`，原始帧已保留，无重试、补操作或断点恢复。首个 5,000 检查点通过，操作日志与 receipt 均为 2,048 行。停止后只读校验全部 39,611 个 WAL 帧并在内存重建，完整性通过：全部已 ACK 操作持久存在，未确认操作未发布到文档/receipt；其新物理 blob 已写完，库存最终为 ready，但这不能证明 `WriteLease.finish` 已向调用方成功返回。该次发送至错误共 21.486 秒，同一文案既可能来自 2 秒 WS 数据库等待，又可能来自 15 秒 worker deadline；服务日志为空，尚不能确定具体阶段或归因于某条 SQL/I/O。所有人离开后的会话、历史/GC 收敛及 RSS 回落未执行，仍为未验收。原数据库/WAL 字节未变，测试进程已确认退出，下一步先补齐阶段诊断再修复，不能以定点或部分长测代替完整 10 万次。 [长测失败及原始帧、只读复核](E:/Works/arkret-org/convert/office/target/review-phase6-validation/performance/d07-final-20261111T020437Z-b54fc2a6-safe/result.json)

### D07 阶段诊断续修

进一步核对原始错误帧的发送路径后，可以排除本次由 2 秒 WS 等待直接产生：该路径最终经 `close_with_error` 统一发送固定的 `Database busy; reconnect to resynchronize`，本次保留原始数据库错误的帧来自 Operation 的失败分支。因此它触发的是 15 秒数据库 worker deadline，仍不能据此断言 SQL 本身执行了 15 秒；前驱等待、阻塞线程调度、SQLite 执行/提交及返回都需区分。独立只读 SQL 计划审计未发现明确的长期读事务或足以解释本次停顿的大扫描；WAL 增长仅为待观测现象，未归咎于已正确关闭事务的采样器。 [只读假设及证据](E:/Works/arkret-org/convert/office/target/review-phase6-performance/d07-sql-audit/hypotheses.md)

现补齐每个 WS operation 独立的数据库观测上下文，贯穿提交和 ACK 前复验，复用已有 `blob_write_finish`、`document_metadata_commit` 标签及 worker 进度。仅服务器失败产生一条结构化告警，包含类型化标识、静态阶段/原因和计数，不记录原始错误、凭据或正文；新操作不会继承前次超时状态。2 秒等待使用独立原因，仍保持 503/1013 策略。分离上传任务只继承现有观测上下文，提交、取消、准入、GC、心跳及所有生产时限保持原语义。三项新增确定性回归验证真实阻塞 worker、跨任务阶段继承、迟到清理隔离、敏感 sentinel 不入日志，以及真实 2 秒边界的关闭策略。

本次 core/server 单元及鉴权、消息、持久化集成共 **140 通过、0 失败、0 忽略**，源码在运行前后不变；先前的 5 项纯内存定点结果不累加。Windows `core + server --all-targets` 编译检查通过；Linux 对这两个组件的严格全目标 Clippy（`--no-deps -D warnings`）通过。首轮定点脚本单独选择 core，未启用服务器的 bundled SQLite 依赖组合，链接失败且未运行测试；修正选择范围后完成编译。编译后首次 120 秒 I/O 门槛未通过，文件型测试未启动；自然回稳后在原门槛下执行全部 140 项。新诊断候选的完整 workspace、release 运行、10 万次及新版交付仍待验证，不能沿用 `2ae0faa` 的全套通过结论。 [140 项结果](E:/Works/arkret-org/convert/office/target/review-phase7-validation/phase7-runtime-d504124f019c4da1be442549f46bca10/summary.json) · [Windows 检查](E:/Works/arkret-org/convert/office/target/review-phase7-validation/windows-check-fd66f985885244bd9c080311d038af1d/result.json) · [严格 Clippy](E:/Works/arkret-org/convert/office/target/review-phase7-validation/phase7-diagnostic-build-e5d37ba4c30448638862336237e1a57c/clippy.log)

诊断 harness 在原 cold TSV 后追加带时间戳的 PSI 字段，并在失败时保留即时压力读数；原请求计时边界、0.25 秒调度、100,000 次操作、21 检查点、零重试、同进程回收及有限收敛预算保持。导出器兼容旧数据，6 项合成检查通过；长 GET 造成的采样间隙按真实时间记录，不假定每 250ms 必有样本。非空 WS 失败日志采用白名单字段导出，仅含静态阶段/原因、worker 状态、数字计数、时间和原行号；原始日志留在 native，7 项合成隐私与格式检查通过。

新诊断候选 `0372b451…784e9` 正常 release 构建完成后，通过独立 2,488 文件/21 资产预检，再执行一次完整定义的长测。在 **19,976 次 ACK 后，第 19,977 次**收到 GC 积压门禁拒绝，未重试或补发；该次发送到错误仅 1.261ms，不是前次数据库超时。停止后的只读 WAL 复核确认全部 ACK 已持久化、未确认操作未提交，GC 队列正好 1,024 项且最终均未认领，抽样物理对象仍在；停止后的状态不能证明拒绝瞬间的 GC 阶段。失败前 30 秒的 120 个 PSI 样本显示 full stall 累计 78.30%，最大采样间隔 271ms；这些是主机级同时发生的压力，不能归因于具体设备或任务。唯一原服务告警为更早的 operation history compaction deferred / database is locked。完整 10 万次及后续回收/RSS 验收仍未完成。 [0372 身份](E:/Works/arkret-org/convert/office/target/review-phase7-validation/phase7-diagnostic-build-e5d37ba4c30448638862336237e1a57c/frozen-proof.json) · [源码预检](E:/Works/arkret-org/convert/office/target/review-phase7-validation/source-preflight/90b4aa5e107e40189438458c5c5f37cf/result.json) · [19,976 次失败及连续压力证据](E:/Works/arkret-org/convert/office/target/review-phase7-validation/d07-20261111T030017Z-b167619f-safe/result.json)

### GC FIFO 查询工作量修复

进一步发现了独立可复现的查询工作量缺口。使用正常 release 实际链接的 bundled SQLite **3.54.1** 静态库，在独立内存 catalog 重建 19,978 条库存与 1,024 条未认领队列：原 GC 选择 SQL 走 claim-deadline 范围索引，执行 **20,542 VM steps、1 次排序**；仅指定既有 `idx_storage_gc_order` 后为 **315 VM steps、0 次排序**。两者分别通过独立计算的“最旧 16 项”校验。工具的 archive 成员与 release rlib 中原生对象字节一致，固定 0372 ELF 包含相同 SQLite version/sourceid，原源码、迁移、C 工具、编译参数和输入哈希均已冻结。工作量差约 65.2 倍；内存诊断耗时不代表现场 I/O，也不能单独证明全部积压原因。 [实际 bundled SQLite 诊断](E:/Works/arkret-org/convert/office/target/review-phase7-validation/gc-bundled-diagnostics/7cf256f551f744d38a5ec568d40fa095/result.json) · [冻结输入](E:/Works/arkret-org/convert/office/target/review-phase7-validation/gc-bundled-diagnostics/7cf256f551f744d38a5ec568d40fa095/frozen-inputs-manifest.json)

生产 GC 选择查询现提取为同一私有常量，并指定已有 FIFO 索引，避免对正常可认领积压先排序再应用 LIMIT。认领/写入租约保护、失败项排除补足、事务、100 项页、16 项认领批、8 并发 unlink、1,024 门禁和原时限保持。新增两项回归直接使用该生产常量和实际迁移：检查 bundled 查询计划无临时排序及 FIFO ties；实际清理 26 项并验证 20 项失败前缀、活跃认领与写入者继续保留，过期认领/写入者和物理文件正确处理。

此轮包含上述两项的新定点集 **146/146 通过**，运行前后源码不变，先前内存单项不累加。两次编译后 I/O 门槛未通过时没有运行文件型测试，随后原门槛通过后完整执行。严格 core/server 全目标 Clippy 通过。完整 workspace、新 release 的 10 万次及最终发布包仍待验证，不能把查询计划改善等同长测成功。 [146 项结果](E:/Works/arkret-org/convert/office/target/review-phase8-validation/phase8-runtime-9e6e0c76bb944bdf8050e5fec14cc9fc/summary.json) · [严格 Clippy](E:/Works/arkret-org/convert/office/target/review-phase8-validation/phase8-diagnostic-build-6e1aa411ed23463aac02f89bb5257ffa/clippy.log)

GC FIFO 候选随后完成正常 release 构建，冻结 ELF SHA-256 为 `cfa2f228bbf8b6017b9813ab6485f9df7e808b969633b6be8dde935a1497e06d`；Windows core/server 全目标编译检查通过。独立预检验证 2,488 个受跟踪文件与 21 个 Web 资产。本次同进程、同文档/客户端、单个在途、零重试的长测实际完成 **100,000 次 ACK、全部 21 个持久化检查点**，操作耗时 836.808 秒。离开后只读检查首个样本即确认历史/GC 收敛：operation log 与 receipt 各 2,048 行，快照 2,048 行且无可按数量清理项，GC 队列为 0，库存为 deleted 100,000 / ready 2，客户端高水位 100,000。独立冷文档负载中 3,345 个样本 p95/p99 为 2.767/7.218ms，离开后 100 个样本为 2.464/2.852ms；这些是本次实测，不能承诺通用时延。 [候选身份](E:/Works/arkret-org/convert/office/target/review-phase8-validation/phase8-diagnostic-build-6e1aa411ed23463aac02f89bb5257ffa/frozen-proof.json) · [Windows 检查](E:/Works/arkret-org/convert/office/target/review-phase8-validation/windows-check-e3f9aeee670d48a6b1c2470f6e5bd81e/result.json) · [源码预检](E:/Works/arkret-org/convert/office/target/review-phase8-validation/source-preflight/f4283fb7ead54dd3a51d552c41c99ecd/result.json) · [原始运行的安全导出](E:/Works/arkret-org/convert/office/target/review-phase8-validation/d07-20261111T040540Z-ebf3a775-safe/result.json)

**该次 D07 仍以退出码 1 结束，完整后置验收未通过。** 重开会话的颜色断言硬编码期待第三个颜色 `#64B5F6`，但生产颜色表的第二个颜色实际为 `#81C784`，本次前后两次第二连接均观察到后者。这与会话重置一致，仍不能将原失败结果改记成功；没有直接 registry 计数证据，断言之后的重开连接正常关闭与最终 REST 检查也未执行。未重跑、补发或修改原始结果。RSS 从初始 28,712,960 字节增至离开前 116,576,256 字节，30 秒后及收敛时为 116,477,952 字节，仅下降 98,304 字节，尚未回到基线；不能用历史/GC 收敛代替内存充分回收证明。原服务已确认退出且无遗留自有进程。安全 result SHA-256 为 `0e39eb64c6ef49b26d462b5ee429c4d9d946f9893704f405a5b6d270d0010020`，保留原 native 数据和失败证据。当前候选的完整 workspace、新版交付包及上述完整后置验收仍待完成。 [终态、RSS 与未执行项](E:/Works/arkret-org/convert/office/target/review-phase8-validation/d07-20261111T040540Z-ebf3a775-safe/terminal-acceptance-notes.json)

### 逐项验收补证与会话退出竞态修复

按原 27 项的字面验收要求补齐直接回归：S03 增加群组父行删除失败时成员与父行一起回滚；S04 串联真实密码重置、旧 JWT 禁止再次签发 WOPI、既有 WOPI 读写拒绝与新凭证读回原字节；S06 分别验证已注册和未注册目标邮箱的邀请详情保密、无合法邮件 token 时不能消费管理员邀请，且 accepted/用户数量/角色不变。合法持有邮件 token 的接受流程仍是 bearer capability，未改变产品的邮箱验证流程。S05 新增同一房间的 owner、editor、viewer、已移除创建者、陌生人权限矩阵，覆盖文档读写/删除/导出/历史/分享及 WOPI 签发、读写和锁；拒绝前后比较目标文档、文件、两类历史、分享和锁的业务行，未将该快照夸大为所有全局存储副作用证明。 [安全补证](E:/Works/arkret-org/convert/office/crates/server/tests/security_regressions.rs:161) · [跨协议权限矩阵](E:/Works/arkret-org/convert/office/crates/server/tests/acl_matrix_regressions.rs:141)

F05 增加保存内容中故意陈旧的公式缓存、文本引用、SUM 依赖链及除零错误的真实转换回归。F08 使用同一份独立编写标准 parts 的重排丰富 PPTX：slide2 在 slide1 前，后者仍含图片、图表、形状与明确降级告警；fixture 6,050 字节、SHA-256 `5428f2e2d5c5d4955bcaac310d6bebfba26f374269be0f639a5c32a69f27f5ce`，固定 ZIP 头的两次生成逐字节一致。浏览器测试另补真实编辑/保存/CSV 下载，以及该 PPTX 的页序、丰富元素显示与原件逐字节恢复；已实际收集 **144 项**，尚未执行本轮浏览器验收。F07 独立阅读器脚本原已有字体/颜色/边框/对齐断言，本轮再明确数字格式、边框颜色与对齐值；在保留的历史输出上通过，不冒充本轮新导出文件验收。 [CSV 转换](E:/Works/arkret-org/convert/office/crates/server/src/handlers/convert.rs:955) · [同 fixture 重排与丰富元素](E:/Works/arkret-org/convert/office/crates/export/tests/review_regressions.rs:220) · [浏览器收集](E:/Works/arkret-org/convert/office/target/review-phase9-validation/playwright-collection-bcd558b36bc1432a953795656eaefac3/result.json) · [独立阅读器预检](E:/Works/arkret-org/convert/office/target/review-phase9-validation/reader-assertion-precheck-035753d319154c6580bc4e1c7a739fb2/result.json)

上述候选完整 Linux workspace 实测为 **599 通过、0 失败、5 忽略**、32 个测试目标结果，严格 workspace/all-targets SSR Clippy 通过（50.77 秒）。完整 2,490 个受跟踪文件及仓库 HEAD/index/dirty 实际状态前后一致，编译前源码清单 SHA-256 为 `591019aec3a4aa802e804eff4e2af190ddc574b01b4b1f37dbe610aef78c2143`。先前新增 PPTX 测试错误地把 ZIP 文件名迭代的 Result 当字符串，编译失败原件保留；显式检查合法文件名后才完成此轮全套，未删除断言或累加重复运行。**这份 599 结果发生在下述新增会话生产修复之前。** [完整结果与源码绑定](E:/Works/arkret-org/convert/office/target/review-phase9-validation/phase9-workspace-ceeab61e1d5f4cb18040228d5fb3ae63/workspace-summary.json)

独立审计确认两处会话生命周期缺口。旧清理先检查空状态并释放会话 mutex，再等待注册表写锁；新连接可在此间完成加入，临时 Arc 随后释放，旧清理仍按先前空状态移除活跃会话。真实确定性回归在旧实现上 **Cargo 101、1 项失败**，断言注册表数量实际 0、预期 1，而非超时。另有任务持 Arc 跨越最后 leave 时，现有保护会跳过退休；引用后来释放后，原先没有自动重试。现保留同一会话 mutex 到注册表决策结束，继续检查空状态、引用计数与指针；等待会话 mutex 时不持注册表锁。服务器另显式启动独立 **60 秒** reaper，首次 tick 在 60 秒后、跳过积压 tick，不依赖数据库/存储/回收站循环。 [旧实现真实失败](E:/Works/arkret-org/convert/office/target/review-phase9-validation/phase9-collab-race-red-e5cc4ce149fe4d0ab39d1cc7c399da53/result.json) · [生产清理与策略](E:/Works/arkret-org/convert/office/crates/collab/src/manager.rs:146)

修复后同一 race 断言通过，协作库 **19/19 通过、0 忽略、0 过滤**；两项暂停时钟回归直接控制 poll 与时间，验证 59/60 秒边界、额外引用存续时不能提前移除、释放引用后无新请求也能在下一 tick 退休，以及重新加入的活跃会话跨多个 tick 保留。另两项 Weak 回归验证最后连接离开后会话/历史/undo/redo 所有权及文档锁不被注册表保留。Tokio test-util 仅为 dev-dependency，构造器不偷偷启动任务。**这不证明此前 RSS 的具体归因，也不将释放所有权等同进程 RSS 回到基线。** 新生产修复的完整 workspace/Clippy、正常 release、两平台交付及含 60 秒策略窗口的完整 D07 仍待验证。 [19 项真实结果](E:/Works/arkret-org/convert/office/target/review-phase9-validation/phase9-lifecycle-green-00b7e7472ee14e4c9ff920358582ec9d/result.json)

### 提交前最新验证快照

上述新增会话生产修复随后完成两平台全套验证：Linux 与 Windows 各 **602 通过、0 失败、5 忽略**，均为 32 个测试目标结果，两平台严格 workspace/all-targets SSR Clippy 通过。Windows 原包装器在 Cargo 已退出 0 后，因结果汇总解析错误退出 1；保留原件，通过独立离线解析原始日志并核对 26 个真实测试 EXE 与编译清单、六个 doctest、源码与环境，得到 602 项结果，没有重跑测试或改写原失败。 [Linux 全套](E:/Works/arkret-org/convert/office/target/review-phase9-validation/phase9-workspace-ec1be7005ec0462baa2ac57b93df32a6/workspace-summary.json) · [Windows 原始结果](E:/Works/arkret-org/convert/office/target/review-phase9-validation/windows-workspace-test-702c4c40b11841d6a35336e79ca3bc50/result.json) · [Windows 离线核验](E:/Works/arkret-org/convert/office/target/review-phase9-validation/windows-runtime-derived-330995ece6bb49ab9ca15387ce80129b/result.json)

正常 release 二进制已冻结：Linux ELF SHA-256 为 `2b9553408109ce1ce42d2d08591f7415696a89d76a119babfa1f90c829597fe8`，Windows PE 为 `b34ab638ae2c3719141f679b86f79e913e9af021aa31d94f8f2ef5a18ac1b56b`。Linux 实际运行全部 **144 项**浏览器/API，0 跳过、0 失败、0 flaky、0 重试；独立 Office 阅读器也在本轮新导出文件上通过。 [Linux release 身份](E:/Works/arkret-org/convert/office/target/review-phase9-validation/phase9-normal-release-818bf26ed2614be99fd80aab2141da1f/release-identity.json) · [Windows release 身份](E:/Works/arkret-org/convert/office/target/review-phase9-validation/windows-normal-release-c7cf4ba1c99b4456bc55cf0493dde6ca/release-identity.json) · [144 项验收](E:/Works/arkret-org/convert/office/target/review-phase9-validation/e2e/06ad909f631a475e9735d3af66db21fb/acceptance/result.json) · [本轮独立阅读器](E:/Works/arkret-org/convert/office/target/review-phase9-validation/office-readers-c0bf73c6f17745d982274dedd91d2d91/result.json)

Linux phase9 ZIP 含 55 个文件，SHA-256 为 `e874ef12c8cb4fcd9192f6351f451f41d64ca2c119213119d004834aceec50f2`；Worker、强停与 ACK 重放、分块续传、带 WAL 的冷备恢复及缺失/篡改负例均通过。原 canonical ZIP 已恢复到原哈希，原 Web 恢复为旧 ZIP 内的 20 个静态文件，自有服务进程为 0。该提交前快照中的 Windows ZIP 包装器预检首败原件保留；其后只修包装器、完成当前提交桥接并通过完整验收，见下一节。 [Linux 发布包验收](E:/Works/arkret-org/convert/office/target/review-phase9-validation/linux-package/phase9-package-43e6e06c892f4286a01edc792af604f5/summary.json) · [独立清理核验](E:/Works/arkret-org/convert/office/target/review-phase9-validation/linux-package/phase9-package-43e6e06c892f4286a01edc792af604f5/independent-final-cleanup.json) · [Windows 包装器首败与恢复](E:/Works/arkret-org/convert/office/target/review-phase9-validation/windows-package-outer-8655253d12e1493cadff5f0522efa67c/result.json)

这些运行绑定的是 `e6388d0` 加本次 13 个改动文件的实测快照，其中报告当时的 SHA-256 为 `c8321d0e91cbe3b98480fe4812785e2f7d7b47a86531eebca786d018effe0d00`；此节属于运行结束后的报告更新，不把新提交 HEAD 冒充构建时 HEAD。当前 Garage、Windows ZIP、提交桥接与远端运行状态以下一节为准；D07 该 phase9 阶段协议与后置功能验收已通过，RSS 充分回收仍未证明。各轮旧 ZIP/镜像保留各自原身份。

### 当前交付、提交桥接与远端状态

该轮六个修复提交推送完成时，`main` 与 `origin/main` 均为 `2c9a249b3041c0a226ab753942714b5060ee3eca`。构建、602 项测试和真实 144 项验收仍保留原 `e6388d0` 加 13 项工作区修改的身份。统一只读桥接随后两次核对全部 **2,490** 个受跟踪文件、递归仓库状态及 native mirror：**2,489** 个产品及配套源文件的内容与 Git 模式相同，仅本报告由运行时的 `c8321d0e…` 变为提交时的 `d24e1d2d…`；HEAD、索引 blob 和 dirty→clean 状态变化逐项列明，native 报告仍保留旧内容。历史未记录的 POSIX 权限位不补造为相等结论。这证明当前提交与实测产品内容等价，未重写历史 proof，也不等于在新 HEAD 重新执行测试。本节又是后续报告更新，不追记进既有桥接的报告哈希。 [统一桥接 a627](E:/Works/arkret-org/convert/office/target/review-phase9-validation/current-commit-source-bridge-99a6e5cb2a3341f38492694637f86602/bridge.json) · [当前完整清单 96fa](E:/Works/arkret-org/convert/office/target/review-phase9-validation/current-commit-source-bridge-99a6e5cb2a3341f38492694637f86602/current-source-inventory.json) · [已推送提交](https://github.com/arkret-org/office/commit/2c9a249b3041c0a226ab753942714b5060ee3eca)

Windows phase9 ZIP 最终 SHA-256 为 `305942b42935638ed7802385383f16a9616104ab13e75c58522be12ee38dfaa1`，内部 PE 为已冻结的 `b34ab638…ac1b56b`。完整 21 项资产逐项哈希、真实 Worker 浏览器流程、REST/WS 强停恢复与 ACK/receipt 幂等、D04 续传/冲突/重放完成只计费一次、停写后数据库+有效 WAL+blob/上传原件冷备恢复及缺失/篡改负例均通过。七次移动均限制在实际 metadata target 和本轮 owned UUID，canonical ZIP 恢复到原 SHA `2da212e3…bb310`，原 Web 的 20 个文件全哈希恢复；历史 phase4/5/6 档案未变，旧备份及 staging 路径已消失。独立核验三个自有服务 EXE 路径及本轮浏览器命令均无存活进程。 [Windows ZIP](E:/Works/arkret-org/convert/office/target/dist/contrix-office-x86_64-pc-windows-msvc-phase9.zip) · [外层 27cc](E:/Works/arkret-org/convert/office/target/review-phase9-validation/windows-package-outer-08394337960046e0bd7038f940d71fdf/result.json) · [完整验收 5933](E:/Works/arkret-org/convert/office/target/review-phase9-validation/windows-package-e5c54a186d04497889f45071e4079db0/result.json) · [独立恢复与进程核验 4ee7](E:/Works/arkret-org/convert/office/target/review-phase9-validation/windows-package-outer-08394337960046e0bd7038f940d71fdf/independent-final-cleanup.json)

两次 Windows 包装器前置失败未改写：`865525…` 因 `$Run`/`$run` 类型冲突停止，`a303…` 在桥接校验已通过后因外层 guard 的 script-scope 状态不可读取而在首个移动前停止。两次均未移动文件或启动包内应用，ZIP/Web 与历史档案保持不变。这两处错误分别以重命名小写目录变量为 `$packageRun`（保留 `$Run` 开关）和外层同作用域点调用修正；嵌套脚本只读自检后，另开唯一 UUID 执行上述完整验收。 [变量冲突首败](E:/Works/arkret-org/convert/office/target/review-phase9-validation/windows-package-outer-8655253d12e1493cadff5f0522efa67c/result.json) · [作用域首败](E:/Works/arkret-org/convert/office/target/review-phase9-validation/windows-package-outer-a303989b075a4fc8a20829cc6b327147/result.json) · [目录变量最小修正](E:/Works/arkret-org/convert/office/target/review-phase9-validation/package-prep-c1f8d851f52b4d0988bce273079ab458/directory-variable-only.diff) · [点调用最小修正](E:/Works/arkret-org/convert/office/target/review-phase9-validation/guard-a303-to-083943-scope-only.diff)

当前 Garage S3 兼容服务独立验收 **4/4**、零重试：认证/二进制及编码键读删、高层同名并发保存隔离、维护回收及共享桶隔离均通过。它使用已接受的 602 项构建产物，四项不并入默认全套通过数，也不将默认 ignored 改成 passing。owned launcher/child 的实际 `/proc` 路径均已消失，临时凭据删除，只导出安全证据。这是 Garage 兼容服务结果，不代表 AWS 专用桶验收。 [四项原结果](E:/Works/arkret-org/convert/office/target/review-phase9-validation/garage-c8f20a1cfacf4628b035b47c32274a9f-safe/result.json) · [清理](E:/Works/arkret-org/convert/office/target/review-phase9-validation/garage-c8f20a1cfacf4628b035b47c32274a9f-safe/cleanup.json) · [实际退出核验](E:/Works/arkret-org/convert/office/target/review-phase9-validation/garage-c8f20a1cfacf4628b035b47c32274a9f-safe/post-exit-verification.json)

推送触发的 GitHub Actions **42986005158** 已结束：七个 job 为 failure，均 **steps=0、runner 为空**，另两项 skipped；没有远端测试或镜像构建步骤实际执行。container job 注解提示近期账户付款失败或需要提高支出额度，属于账户付款/支出额度启动门禁，尚未确认具体是哪一种账户状态。账户当前是否恢复不能从该次失败判断，远端仍须另行实际执行 CI；本地验收不替代远端 container/macOS 结果，不将本次未启动标成源码测试失败。 [GitHub 运行](https://github.com/arkret-org/office/actions/runs/42986005158) · [安全运行快照](E:/Works/arkret-org/convert/office/target/review-phase9-validation/remote-ci-f00dab4198b44e02898d37edcf3d92c8/run.json) · [九项 job 状态](E:/Works/arkret-org/convert/office/target/review-phase9-validation/remote-ci-f00dab4198b44e02898d37edcf3d92c8/jobs-summary.json) · [启动门禁注解](E:/Works/arkret-org/convert/office/target/review-phase9-validation/remote-ci-f00dab4198b44e02898d37edcf3d92c8/failure-annotations.json) · [09:18 UTC 刷新仍为 attempt 1 failure](E:/Works/arkret-org/convert/office/target/review-phase9-validation/remote-ci-f00dab4198b44e02898d37edcf3d92c8/latest-run.json)

先前 D07 控制器 session **93718** 在原 Windows 资源门槛退出 **2**：120 秒预算内记录 69 个样本，连续安静样本为 0，`admitted=false`、`workload_started=false`。该次未进入 native 执行，未创建新的 native execution/runner 日志，也未启动 D07 server 或 100,000 次负载；不是产品运行失败，更不能记为 100k 已开始或通过。此前历史 100k 数据阶段、终态失败与 RSS 数值继续按各自原身份保留。其后在协调窗口满足原双门槛的独立正常运行见下文；本次门禁失败原件不改，未放宽门槛或生产时限。 [原资源门禁 c753](E:/Works/arkret-org/convert/office/target/review-phase9-validation/d07-current2c-0ce44cce964941d38ee38590d7a798ec/phase9-windows-gate-e7b7ef8bebcc439587b96d7787dcd795.json) · [未启动诊断](E:/Works/arkret-org/convert/office/target/review-phase9-validation/d07-current2c-0ce44cce964941d38ee38590d7a798ec/gate-diagnosis.json)

随后原正常 release `2b955340…97fe8` 在独立协调窗口通过双资源门槛，控制器 session **97241** 真实退出 **0**。同进程、同文档/客户端、单个在途、零重试的运行完成 **100,000 次尝试/ACK、全部 21 个持久化检查点**，操作阶段耗时 **811.642 秒**；历史与 GC 收敛（GC=0），原 60 秒策略窗口后的会话退休/重开、连接正常关闭和最终 REST 版本 **100,001** 均通过。运行后再用冻结 provider 完整读取 2,490 个文件及递归仓库状态，整个 source inventory 对象与运行前 `96fa4414…` 相同，历史构建身份未改；独立 `/proc` 检查确认自有 server/runner 均退出。安全 result 的 `success` 仅指协议、持久化、历史/GC 和清理断言，不能外推为 RSS 验收通过。 [最新正常运行 b788](E:/Works/arkret-org/convert/office/target/review-phase9-validation/d07-20261111T085256Z-dadd2d45-safe/result.json) · [完整源码前后比较](E:/Works/arkret-org/convert/office/target/review-phase9-validation/d07-20261111T085256Z-dadd2d45-safe/comparison.json) · [独立进程清理](E:/Works/arkret-org/convert/office/target/review-phase9-validation/d07-20261111T085256Z-dadd2d45-safe/independent-cleanup.json)

**RSS 充分回收仍未证明。** 基线、离开前、离开 30 秒后及只读收敛后的 RSS 分别为 **28,733,440 / 133,451,776 / 132,993,024 / 132,993,024 字节**；离开后仅下降 **458,752 字节**，仍高于基线 **104,259,584 字节**。冷文档 baseline / during / after 的 p95/p99 分别为 **2.4077/2.6887、2.3934/8.0331、2.5533/2.9359 ms**。首 5,000 次与末 5,000 次 ACK 的 p95/p99 从 **7.2208/23.3444** 变为 **13.7703/30.5338 ms**；本轮后段成本有实际变化，不能只用总体成功率或冷文档 p95 宣称无退化，也不能据此归因于某个 owner、设备或分配器。 [完整阶段与 RSS 数值](E:/Works/arkret-org/convert/office/target/review-phase9-validation/d07-20261111T085256Z-dadd2d45-safe/result.json)

只读 DB/ORM/连接池依赖审计未找到已证实的每操作无界容器；SQLite 连接与其缓存是实际持久 owner，但没有本轮字节归因。Diesel 默认 prepared cache 为 Unbounded，而已核 raw SQL 明确不进入该缓存，不能把 counter 绑定值误判为每次新增 SQL cache key。此结论不证明无泄漏；后续内存归属诊断尚待实测，不以 trim、降低负载或改变生产阈值冒充修复。 [DB owner 审计及未证部分](E:/Works/arkret-org/convert/office/target/review-phase9-validation/db-memory-owner-audit-1835b8678d8e4d6fba76cb832568c1cf/audit.md)

### 分配器诊断与操作日志边界续修

正常 100,000 次结果保留为上述独立运行。其后等待自然空闲的两次诊断前置门禁未通过，未启动应用；第三次在原双门槛通过后，仅向所属服务进程注入只读分配器采样库。该仪表化运行在 **28,363 次 ACK 后，第 28,364 次**因 `database is locked` 退出 **1**，没有重试或断点续跑。结构化日志把失败定位到 `blob_write_reservation`，约 3.005 秒；同期主机级 I/O 压力不能单独证明原因。它不是正常长测验收，也未执行离开后的三个观察阶段。 [诊断失败原结果](E:/Works/arkret-org/convert/office/target/review-phase9-validation/d07-allocator-diagnostic-20261111T103641Z-245822b9-safe/result.json) · [失败时间关联](E:/Works/arkret-org/convert/office/target/review-phase9-validation/d07-allocator-diagnostic-20261111T103641Z-245822b9-safe/failure-capture-analysis.json)

149 份分配器 XML 全部可解析；末次 arena 已获系统内存约 50.28 MB，其中空闲 bin 估计约 44.38 MB（88.26%）。这支持该部分运行存在空闲保留的方向，不能把它当作 Rust 活对象字节或驻留空闲页，也不能解释正常 100,000 次离开后的全部 RSS 差额。最长单次采样约 1.019 秒，结束早于失败操作约 166 秒；失败操作窗口重叠的两次采样分别约 0.452 和 0.599 毫秒，不能据此断言采样器导致数据库超时。没有修改分配器参数、执行 trim 或降低负载。 [分配器汇总与范围](E:/Works/arkret-org/convert/office/target/review-phase9-validation/d07-allocator-diagnostic-20261111T103641Z-245822b9-safe/allocator-summary.json)

对已停止实例的所属副本取证确认：全部 28,363 次 ACK 对应持久状态，未确认的 counter 28,364 没有 receipt/log 或文档变更；原数据库、WAL、SHM 哈希保持不变。原取证包装器额外要求日志恰好 2,048 行，实际为 **2,049 行**，因此其退出 1 和 false 结果原样保留。依据原字段另行推导的提交原子性结论与日志超限分别记录，没有重写原失败。 [原始取证失败](E:/Works/arkret-org/convert/office/target/review-phase9-validation/d07-allocator-diagnostic-20261111T103641Z-245822b9-safe/durability-proof.json) · [限定字段推导](E:/Works/arkret-org/convert/office/target/review-phase9-validation/d07-allocator-diagnostic-20261111T103641Z-245822b9-safe/durability-fields-derived.json)

源码复核确认新的生产边界缺口：操作提交后的日志裁剪是独立事务，失败只告警而仍可 ACK，且没有无新流量的补偿 owner。持续 DELETE 故障下日志可以超过 **2,048 行/32 MiB**，并非有意保留的 OT anchor。本次另有匹配路径的裁剪锁错误告警；这仍不是 RSS 增长的已证原因。 [回归设计与源码依据](E:/Works/arkret-org/convert/office/target/review-phase9-validation/oplog-retention-fault-design-b4bfe161eb5345f3934ab3d7dbb7ca17/regression-design.md)

新增真实 WebSocket 回归先在旧生产代码运行，**0 通过、1 失败、退出 101**。一致的 2,048 条历史夹具和文档限定的 DELETE 故障触发器均已通过前置检查；随后客户端实际收到新版 `DocumentState`（版本 **2,050**），在“不得 ACK 或发布”的断言处失败。测试停在该帧，未继续读取，故不把实际收到 ACK 写成实测结论。先前启动包装器在资源门禁通过后误读 tuple，未运行测试；该退出 1 单列保留。后续已将裁剪纳入提交事务，并补有界后台收敛；真实绿例及尚未完成的新交付验收见下一节，旧红例原件不改。 [旧代码实际红例](E:/Works/arkret-org/convert/office/target/review-phase10-validation/oplog-atomic-red-ae72e1b5dd7a43c783d008109c347503/red-runtime-v2-b745554879e340a4b917a6eadf7f08fd/red-runtime-result.json) · [红例日志 8499](E:/Works/arkret-org/convert/office/target/review-phase10-validation/oplog-atomic-red-ae72e1b5dd7a43c783d008109c347503/red-runtime-v2-b745554879e340a4b917a6eadf7f08fd/red-runtime.log) · [未运行测试的包装器错误](E:/Works/arkret-org/convert/office/target/review-phase10-validation/oplog-atomic-red-ae72e1b5dd7a43c783d008109c347503/red-runtime-result.json)

所有诊断所属进程已退出。结束后的单次完整 source capture 与运行前 2,490 项完整清单相同；上述实测仍绑定原 `2c9a249` 等价产品快照。此后才解除源码冻结、更新本报告并开始日志边界修复，新修改不冒充既有 release/602 项/144 项或 ZIP 的验收身份。 [源码前后比较](E:/Works/arkret-org/convert/office/target/review-phase9-validation/d07-allocator-diagnostic-20261111T103641Z-245822b9-safe/post-source-comparison.json) · [进程清理](E:/Works/arkret-org/convert/office/target/review-phase9-validation/d07-allocator-diagnostic-20261111T103641Z-245822b9-safe/independent-cleanup.json)

### Phase10 日志事务与平台复验（阶段记录）

新操作的 operation_log 裁剪已进入文档提交的同一 IMMEDIATE 事务。写入前按持久文档版本拒绝已有 future 日志，避免旧 current+1 被本次提交吸收；写入、receipt/客户端高水位和裁剪一起提交，裁剪失败或最多 1,000 行仍不足以恢复 **2,048 行/32 MiB** 后缀预算就回滚，不发布本次内容或 ACK 新操作。协作读取与写入使用共享预算；旧积压由独立、有界、公平的后台 owner 在无新编辑时收敛。已存在 receipt 的幂等快路径保留原语义，不把它描述为所有历史重放 ACK 时刻均强制满足新预算。 [事务内裁剪](E:/Works/arkret-org/convert/office/crates/server/src/services/persistence.rs:1027) · [共享策略与后台 owner](E:/Works/arkret-org/convert/office/crates/server/src/services/operation_history.rs:248) · [新增索引迁移](E:/Works/arkret-org/convert/office/migrations/027_operation_history_retention.sql)

独立静态复核未见阻塞，覆盖事务内失败回滚、后台只读预检、16 实体/1,000 删除/2,049 条元数据投影及单 retry 槽的有界性；该结论不计入运行时测试数量。 [独立只读审计](E:/Works/arkret-org/convert/office/target/review-phase10-validation/operation-history-readonly-audit-d7ea74c7ac604ff2b9ce11a7ec1b2f13/AUDIT.md)

旧代码 session **83624** 的真实红例是前节已记录的 DocumentState 2050，没有继续读取 ACK。修复后 session **78150** 的三个真实集成目标共 **45 通过、0 失败、0 忽略、0 过滤**：消息 18、operation_history 8、持久化 19。回归覆盖 DELETE 故障时文档/文件/历史/配额/receipt/客户端与日志全事务回滚、失败候选 blob 的真实清理、同 op ID 解除故障后恰好一次提交、2,048 行及精确 32 MiB/超 1 字节边界、3060 条旧积压的前台拒绝与无新编辑后台收敛，以及 current+1 future 防护。这是生产日志边界的实际定点绿例，不累加成额外 workspace 通过数。 [45 项真实终态](E:/Works/arkret-org/convert/office/target/review-phase10-validation/phase10-green-compile-2fbbcc0e44674bfaacc1d051c5ceff7e/green-runtime-9f85e5b2e0ba44d1a2f36e59f3af1a8e/result.json) · [三个目标原日志](E:/Works/arkret-org/convert/office/target/review-phase10-validation/phase10-green-compile-2fbbcc0e44674bfaacc1d051c5ceff7e/green-runtime-9f85e5b2e0ba44d1a2f36e59f3af1a8e/tests.log)

第一份 phase10 Linux 完整 workspace 已真实完成 **614 通过、0 失败、5 忽略、33 个目标**，严格 Clippy 通过。它绑定 2,493 文件清单 7112614e…889d3，其中持久化测试源文件仍为 09ee4892…6c2a9ca。该快照的正常 release ELF 为 **2408c4f29974329d6d2cdcb3053886082ccd1708ddc904963a721608264a0445**，正常构建、完整 workspace 与源码清单有独立身份绑定。随后 source-only 预检验证 2,493 文件及 21 资产成功，但 run_requested=false、phase=preflight，**不是实际 144 项运行**。这些原件保留其修正前测试快照身份。 [首份完整矩阵](E:/Works/arkret-org/convert/office/target/review-phase10-validation/phase10-workspace-6c0cc1120de64b869be423b77b0ba22a/full-runtime-243fa13b8e8a48f8a8baf65ce0db880f/workspace-summary.json) · [严格 Clippy](E:/Works/arkret-org/convert/office/target/review-phase10-validation/phase10-workspace-6c0cc1120de64b869be423b77b0ba22a/strict-clippy.log) · [2408 正常 release 身份](E:/Works/arkret-org/convert/office/target/review-phase10-validation/phase10-normal-release-e10ce524ae954b27a62eb9a64564ad76/release-identity.json) · [仅源码预检](E:/Works/arkret-org/convert/office/target/review-phase10-validation/source-only-preflight-012051042e1a4931b38894d01b5fd8c1/result.json)

Windows 首轮完整运行 session **48134** 真实以 **Cargo 101 / success=false** 结束：已完成的 23 个目标合计 **581 通过、1 失败、3 忽略**，未运行到其余目标，不按预计 614 补齐。失败在旧 expired_lock_takeover_contends_and_fences_the_previous_token 的预热阶段：两个 warmer 已返回后，立即读取池 idle 得到 **1**，旧断言期待 **2**；尚未执行外部 writer 的 BEGIN IMMEDIATE 或进入实际锁竞争。该轮新增日志裁剪四项均通过，不能隐藏整套的这一失败；原日志、09ee 源码及 failed test EXE 7fab… 均保留。 [首轮 Windows false 原结果](E:/Works/arkret-org/convert/office/target/review-phase10-validation/windows-test-133dedc94ad94650ab440c1e9d3fe913/result.json) · [原始测试日志](E:/Works/arkret-org/convert/office/target/review-phase10-validation/windows-test-133dedc94ad94650ab440c1e9d3fe913/cargo.log) · [failed EXE 7fab 保存身份](E:/Works/arkret-org/convert/office/target/review-phase10-validation/warmup-fixture-fix-7c231141356147a79b9c6706e73ab682/failed-executable-identity.json) · [完整旧测试源码备份](E:/Works/arkret-org/convert/office/target/review-phase10-validation/warmup-fixture-fix-7c231141356147a79b9c6706e73ab682/persistence_regressions.rs.windows101.original)

两位独立 agent 沿源码确认同步假设缺口：Database::run 等待业务 SQL worker，lease 的 Drop 另行排队异步 cleanup，随后才把连接对象归还池；warmer JoinHandle 完成不等于池已经恢复两个 idle 连接。因此瞬时 idle=1 不是池泄漏的证据。修正仅在测试文件：原始完整字节先备份，再以唯一原 **5 秒绝对 deadline** 共用 arrivals、warmer joins 和只读 state settle，要求 connections=2/idle=2/admitted=0，保留原 idle 精确断言。没有发 SQL 轮询预热，也未增加预算；其后的原 **2 秒**真实竞争、独占阻塞、单 winner/409 与全部旧 token fencing 断言字节未变。新测试文件 SHA 为 38712bc4…b09cd，生产代码未变。 [完整最小差异](E:/Works/arkret-org/convert/office/target/review-phase10-validation/warmup-fixture-fix-7c231141356147a79b9c6706e73ab682/fixture-only.diff) · [块外字节及原断言证明](E:/Works/arkret-org/convert/office/target/review-phase10-validation/warmup-fixture-fix-7c231141356147a79b9c6706e73ab682/fixture-fix-proof.json) · [异步归还实现](E:/Works/arkret-org/convert/office/crates/server/src/database.rs:267)

38712 修正后 Windows 新 Compile 与严格 Clippy 均退出 **0**。新持久化 test EXE 9ac5a183…fea84 实际运行该 exact case 为 **1 通过、0 失败、18 过滤，0.69 秒**，包括真实锁竞争及旧 token 防护；过滤数说明这是定点，并非新全套。Native 对应持久化目标 **19 通过、0 失败、0 忽略、0 过滤，10.80 秒**。新 2,493 文件清单与 7112 之间严格仅此测试记录改变，另外 2,492 个记录相同；此桥接只记录测试输入差异，不复用旧验证身份。 [新 Windows Compile](E:/Works/arkret-org/convert/office/target/review-phase10-validation/windows-compile-a9b2772c12fc42eeac41bc9bb2640f84/result.json) · [新 Windows Clippy](E:/Works/arkret-org/convert/office/target/review-phase10-validation/windows-clippy-b1877de11c8f47b39c25f093d59873c6/result.json) · [Windows exact 原结果](E:/Works/arkret-org/convert/office/target/review-phase10-validation/windows-lock-fixture-exact-a36de92e209546e2a8d1d5b3137698bc/result.json) · [Windows 1/0/18 日志](E:/Works/arkret-org/convert/office/target/review-phase10-validation/windows-lock-fixture-exact-a36de92e209546e2a8d1d5b3137698bc/test.log) · [Native 19 项结果](E:/Works/arkret-org/convert/office/target/review-phase10-validation/phase10-fixture-v2-f5ee4babef2142ab8b2917da6f97a107/key19-result.json) · [仅测试输入桥接](E:/Works/arkret-org/convert/office/target/review-phase10-validation/phase10-fixture-v2-f5ee4babef2142ab8b2917da6f97a107/test-only-source-bridge.json)

WindowsTest2 session **31437** 随后也以 **Cargo 101 / success=false** 结束：23 个已完成目标合计 **581 通过、1 失败、3 忽略**。旧 warmup/fencing 用例已通过，新的失败在 concurrent_document_saves_cannot_lose_updates_or_split_download_content 的第 536 行：期待状态集合 **[200,409]**，实际 **[200,503]**。该断言未记录 503 响应 body 或数据库阶段，不能将它归因于 I/O、超时或某个 SQL，也不能据此证明 503 操作未提交；失败后的完整一致性断言未执行，不补造结果。原 WindowsTest2 false 与日志保留，不因后面的 exact 通过而抹去。 [WindowsTest2 原结果](E:/Works/arkret-org/convert/office/target/review-phase10-validation/windows-test-d3b55b7abb5f4710b075d00de60db29e/result.json) · [CAS 状态断言原日志](E:/Works/arkret-org/convert/office/target/review-phase10-validation/windows-test-d3b55b7abb5f4710b075d00de60db29e/cargo.log)

之后按原严格双资源门槛安排独立诊断。首次 Windows 门槛在 120 秒预算内一直 busy，未准入、未启动测试；第二次 Windows 门槛通过，但 WSL 长参数命令在 CreateProcess 阶段因 filename/extension too long 失败，Linux 门槛和 exact 测试均未启动。父包装器观测到的退出 0 不等于双门槛或测试成功，单独分类文件保留这一差别。只修 ignored transport，以短 driver 文件和 payload 文件代替长 argv，没有修改生产代码、测试时限或原失败。 [首次未准入](E:/Works/arkret-org/convert/office/target/review-phase10-validation/resource-only-gate-a73141a6d8394f9aafddd8272466e414/phase10-windows-gate-1848f049dac5456f84dc7a305b14890a.json) · [启动前失败分类](E:/Works/arkret-org/convert/office/target/review-phase10-validation/resource-only-gate-3a5bb39602e5417a996cdc38d71e8a93/launcher-failure-classification.json)

short-file controller 在新的独立双门槛真实通过后，使用同一 test EXE **9ac5a183…fea84**、同一 **38712** 测试源码实际运行 exact CAS：**1 通过、0 失败、18 过滤**，测试日志耗时 **0.47 秒**，包装器起止约 **0.70 秒**。11 项冻结输入、EXE 与调用者环境前后一致，无自动重试。这仅证明该 exact 运行通过，不解释前一全套的 503，也不代替全套。 [原双门槛 828b](E:/Works/arkret-org/convert/office/target/review-phase10-validation/resource-only-gate-828b7c52f7654c2b93e9c0a5d5b581d9/controller-result.json) · [exact CAS 原结果](E:/Works/arkret-org/convert/office/target/review-phase10-validation/windows-cas-exact-ff6ca7860292411a80a733e141bf1eb9/result.json) · [exact CAS 原日志](E:/Works/arkret-org/convert/office/target/review-phase10-validation/windows-cas-exact-ff6ca7860292411a80a733e141bf1eb9/test.log)

在保留上述失败与门禁记录后，38712 新快照的完整矩阵已实际完成：**Linux 与 Windows 各 614 通过、0 失败、5 忽略、33 个目标**。Linux 新完整清单为 **498a989f9587134f8f4d59c67fbc69a7414c43d20b14af6f19a3707ff66a1ebd**，2,493 个记录及递归仓库状态前后相等；新的 full summary 为 b6124fc3…c24efb。WindowsTest3 session **38529** 在 fresh 双门槛通过后完成，真实 Cargo 退出 **0**，源字节和调用者环境前后相同。前一份 7112/09ee 的 Linux 614 仍保留原身份，两轮不累加，也不把两次 Windows false 改记为成功。 [新 Linux 完整矩阵 b612](E:/Works/arkret-org/convert/office/target/review-phase10-validation/phase10-fixture-v2-workspace-8f29b0a4dbaf4a5cbf9d71d993a53062/full-runtime-110c806a2253444e87ad892a8c29e6c6/workspace-summary.json) · [新 Windows 完整矩阵](E:/Works/arkret-org/convert/office/target/review-phase10-validation/windows-test-230158b99c3d49ada38f612bf399bb64/result.json) · [Test3 原日志](E:/Works/arkret-org/convert/office/target/review-phase10-validation/windows-test-230158b99c3d49ada38f612bf399bb64/cargo.log)

新快照的正常 release 命令已重新实际完成。Linux ELF 字节仍为 **2408c4f2…4a0445**，但新的 identity SHA 为 **950d255b5f79685a1c6bdcb850e1d2e5f138e1672a9a7b6c8d389a18d1185c43**，明确绑定新 498a 清单、新 b612 全套与严格 Clippy；没有改写旧 6167/7112 身份。Windows 正常 release 也实际退出 **0**，命令没有 SSR/test features，新 frozen PE 为 **909f41ab2885f99cbb33badd848317bfad8883520142afd4319b4f25a369b298**。四个 Windows 实际 raw stage 的结果、源码/环境前后文件、metadata 与日志已按实际 hash 绑定；Windows 的 2,493 个 path/hash 记录与新 Linux reference 哈希逐项相等，mode/index/递归仓库证据来源仍明示为 native provider/host Git，而非伪造 Windows 独立捕获。该输入绑定未调用包装器或 validator；它生成时尚无 144 结果，因此其预留字段保留 null，随后实际 144 以独立同 run proof 记录，不回填或改写这些 raw stage 原件。 [新 Linux 正常身份 950d](E:/Works/arkret-org/convert/office/target/review-phase10-validation/phase10-fixture-v2-normal-release-9e76da3b32cb4fcfa40a65fc3c536655/release-identity.json) · [新 Windows 正常 release 原结果](E:/Works/arkret-org/convert/office/target/review-phase10-validation/windows-release-35a062e8b837491ba0d35e9048f87c4d/result.json) · [四 stage 实际输入绑定](E:/Works/arkret-org/convert/office/target/review-phase10-validation/windows-input-binding-060d443d6fb0480aa064499ea466889a/WindowsValidationInputs.json) · [绑定范围与来源](E:/Works/arkret-org/convert/office/target/review-phase10-validation/windows-input-binding-060d443d6fb0480aa064499ea466889a/binding-scope.json)

上述新 source/release 身份随后完成真实 **144/144 HTTP/Chromium** 验收：0 跳过、0 unexpected、0 flaky、0 retry，repeat=1、无 grep，Playwright 耗时 **112.597 秒**。outer 明确为 run_requested=true/acceptance，inner 的 server 与 Playwright cleanup 均已退出；2,493 文件与 21 个资产在该次完整合同中保持一致。该次绑定的是新 498a 清单、950d 正常身份及 2408 ELF，不是先前 source-only 预检。outer SHA 为 48d75673…8c4ca9，inner 为 6a4a8bde…ba9651，同次 source-proof 为 beb2316a…306667。 [本轮实际 144 outer](E:/Works/arkret-org/convert/office/target/review-phase10-validation/e2e/5d52f40252d441f490eeba14436f96cc/result.json) · [144 inner 及退出核验](E:/Works/arkret-org/convert/office/target/review-phase10-validation/e2e/5d52f40252d441f490eeba14436f96cc/acceptance/result.json) · [同 run 完整源码/21 资产](E:/Works/arkret-org/convert/office/target/review-phase10-validation/e2e/5d52f40252d441f490eeba14436f96cc/source-proof.json)

两个 phase10 发布包随后均完成真实验收。Linux ZIP SHA-256 为 **44d7b765f4592eb112bb10fe261667d20170b11acce66fbd8a4cedb3579aa198**，56 个文件、ELF 100755，内含正常 2408 ELF 与 027 迁移。完整 source-free Worker 浏览器、REST/WS 强停恢复与 ACK/receipt、D04、冷备恢复及实际数据库索引核验全部退出 0。独立清理确认原 Web 的 20 个文件全哈希恢复，含 canonical 的 13 份既有归档哈希保持，三个自有 EXE 路径与对应 UUID 关联进程均无存活。 [Linux 完整 summary b54a](E:/Works/arkret-org/convert/office/target/review-phase10-validation/linux-package/phase10-package-44bd5ef8f3254e85a3840f969e51c151/summary.json) · [Linux 实际三库索引](E:/Works/arkret-org/convert/office/target/review-phase10-validation/linux-package/phase10-package-44bd5ef8f3254e85a3840f969e51c151/database-index-proof.json) · [Linux 独立清理 5d99](E:/Works/arkret-org/convert/office/target/review-phase10-validation/linux-package/phase10-package-44bd5ef8f3254e85a3840f969e51c151/independent-cleanup.json)

Windows Web copy 的首份计划把 owned ID 误填为 33 位十六进制；路径 guard 在 mkdir/copy 前拒绝，原计划和拒绝记录保留，未触 canonical 或启动应用。改为真实 32 位 UUID f9d8c0ae0fc348d8a2af8b4ec2e7e4d1 后，实际 copy 的 21 个文件逐哈希匹配同次 beb231 source-proof，源内容与调用者环境前后不变，再生成 ready 参数进入包验收。该准备阶段拒绝不改写为包测试失败。 [首份计划路径拒绝](E:/Works/arkret-org/convert/office/target/review-phase10-validation/windows-package-prep-35dfa5e9d1394cbd81114ea932327665/copy-first-path-guard-rejection.json) · [真实 21 资产 copy](E:/Works/arkret-org/convert/office/target/review-phase10-validation/windows-web-owned-f9d8c0ae0fc348d8a2af8b4ec2e7e4d1/web-copy-proof.json)

Windows ZIP SHA-256 为 **58179dd00f98fbc9418ded578e797d24b429e95f48e0366b3aa735d1cfbf41eb**，同样 56 个文件、PE909f…69b298。实际 bf95 烟测、d426 冷备恢复和三库索引检查均成功，Worker、REST/WS 强停与 ACK/receipt 幂等、D04 分块续传/冲突/完成重放只计费一次、DB+有效 WAL+当前 blob/上传原件恢复及缺失/篡改负例全部保持。索引检查的结论限于 ZIP027 字节及 smoke/recovery source/recovery restored 三个 owned DB 的实际 owner=operation_log、document_id ASC/version DESC/id DESC 和 DB/WAL 字节未变，不补造迁移 ledger。 [Windows outer f0f3](E:/Works/arkret-org/convert/office/target/review-phase10-validation/windows-package-outer-f0f3d09a2d0345f3970efc6aff12e360/result.json) · [Windows inner 7aee](E:/Works/arkret-org/convert/office/target/review-phase10-validation/windows-package-7aee6600b8994d18843b7628f833e419/result.json) · [实际 bf95 烟测](E:/Works/arkret-org/convert/office/target/package-smoke/bf95fb544d364ca58ed6cd8ba1f49d14/result.json) · [实际 d426 冷备](E:/Works/arkret-org/convert/office/target/disaster-recovery/d4268ff60cbf4be584490f2306a65538/result.json) · [Windows 实际三库索引](E:/Works/arkret-org/convert/office/target/review-phase10-validation/windows-package-7aee6600b8994d18843b7628f833e419/database-index-proof.json)

Windows 独立结束核验确认原 canonical ZIP 恢复为 **2da212e3…bb310**、原 Web20 全文件及 legacy archives 全哈希恢复；三个 exact owned server 路径及关联 UUID/package Node 浏览器进程为 0。canonical-before.zip 与 web-before 已移回原位置，web-phase10 不再存在；**assembled-phase10.zip 和 assembled-phase10-web 两项所属证据仍保留**，不声称所有 staging 路径消失。 [Windows 独立最终恢复与进程核验](E:/Works/arkret-org/convert/office/target/review-phase10-validation/windows-package-outer-f0f3d09a2d0345f3970efc6aff12e360/independent-final-cleanup.json)

本轮 Garage 兼容服务验收也已真实完成 **4/4、零重试**：维护回收/共享桶隔离 2 项及真实 S3 协议认证、二进制/编码键读删、同名并发保存 2 项均退出 0。该轮绑定新 498a 清单，运行前所有 2,493 个 source 记录匹配已接受输入，完整源码与调用者环境前后相等；它是独立环境验收，不把4项累加到614，也不把默认 ignored 改写为 passing。独立 cleanup 确认所属 launcher/child 两个进程已无、临时凭据已删，安全导出不含运行 fixture/秘密。Garage 兼容服务结果不代表 AWS 专用桶已验收。 [本轮 current scope cf6a](E:/Works/arkret-org/convert/office/target/review-phase10-validation/phase10-garage-validation-47e39b5363c34ccbb86923bf30111488/current-scope-result.json) · [4 项实际 safe runtime bf7f](E:/Works/arkret-org/convert/office/target/review-phase10-validation/phase10-garage-validation-47e39b5363c34ccbb86923bf30111488/safe-runtime/result.json) · [独立清理 8dbe](E:/Works/arkret-org/convert/office/target/review-phase10-validation/phase10-garage-validation-47e39b5363c34ccbb86923bf30111488/independent-cleanup.json)

本轮保存的远端运行派生快照仍为 GitHub Actions **42986005158 / attempt 1 / completed / failure**。旧付款/支出额度 annotation 解释的是该次运行当时未分配 runner、steps=0 的情况；**同一失败 attempt 的状态不能用来判断当前账单、额度或账户 readiness 是否已恢复**。派生快照没有记录精确 fetch 时间，fetch_observed_at_utc=null；snapshot_recorded_at_utc 只代表文件保存时刻，未冒充抓取时间。远端托管 CI/container/macOS 的实际执行结果仍待取得。 [本轮派生状态快照](E:/Works/arkret-org/convert/office/target/review-phase10-validation/remote-ci-refresh-derived.json) · [原运行](https://github.com/arkret-org/office/actions/runs/42986005158)

本轮未注入分配器采样库的正常 100k 已真实结束，退出 0：**100,000 次尝试/ACK、21 个持久化检查点**，操作阶段 **1,183.191 秒**。历史候选、GC、writing/deleting 最终均为 0，在原 **337.5 秒**预算内用 **240.831 秒**完成收敛；同进程重开及最终 REST v100001 均通过。完整 2,493 文件 source-after 与 498a 相等，实际027索引及 DB/WAL 只读核验前后字节相同，自有 server/runner 已退出。该次成功范围为协议、持久化、历史/GC 收敛、重开及清理，**内存项仍未验收**。 [正常运行完整 safe result](E:/Works/arkret-org/convert/office/target/review-phase10-validation/normal100k-byte-only-safe-export-5876fba9741144cdb7c7354324c174db/safe/result.json) · [完整源码及原结果保留](E:/Works/arkret-org/convert/office/target/review-phase10-validation/normal100k-byte-only-safe-export-5876fba9741144cdb7c7354324c174db/post-evidence-result.json) · [实际索引/DB-WAL只读核验](E:/Works/arkret-org/convert/office/target/review-phase10-validation/normal100k-byte-only-safe-export-5876fba9741144cdb7c7354324c174db/database-index-proof.json) · [独立退出核验](E:/Works/arkret-org/convert/office/target/review-phase10-validation/normal100k-byte-only-safe-export-5876fba9741144cdb7c7354324c174db/independent-cleanup.json)

RSS 基线、离开前、30 秒后及收敛后分别为 **29,278,208 / 122,978,304 / 122,855,424 / 123,392,000 字节**。收敛后仍高于基线 **94,113,792 字节（约89.75 MiB）**，较离开前反而增加 **413,696 字节**。首/末 5,000 次 ACK 的 p95/p99 为 **15.881391/31.222572 → 26.367309/90.747934 ms**；本轮后段成本上升，原因尚未证明，不能承诺长期成本平坦或无退化。协议通过与这些内存、时延事实分别保留。

首次结束证据 collector 在 finally 跨 DrvFS 的 copytree/copystat 元数据步骤遇到 Errno1，退出1；原 native 正常运行成功与旧 partial 导出均未改写。随后新 UUID 仅用 copyfile 字节复制已经生成的12份安全文件，导出退出0；**没有重跑业务或数据库分析**。这次导出成功仅证明证据传输完整，不是 RSS 通过。 [byte-only 导出及首错保留](E:/Works/arkret-org/convert/office/target/review-phase10-validation/normal100k-byte-only-safe-export-5876fba9741144cdb7c7354324c174db/byte-only-export-proof.json)

其后单独的 instrumented 分配器诊断在 **8,433 次 ACK / 8,434 次尝试**后退出1、未重试，原 false 结果保留。停止后副本取证确认：8,433 条 ACK 时序齐全，最终版本8434/客户端高水8433、retained receipt 与 operation_log 各2,048，内容哈希与最后ACK一致；未确认的8434没有提交新业务版本、receipt/client高水或日志。原DB/WAL/SHM及 false result 哈希前后相等。全库存补证确认当前内容仍为最后ACK，未发现匹配 pending.new 的现存非deleted实体。 [诊断原 false safe result](E:/Works/arkret-org/convert/office/target/review-phase10-validation/allocator-diagnostic-20261111T153158Z-aefcc6e1-safe/result.json) · [停止后耐久与未提交取证](E:/Works/arkret-org/convert/office/target/review-phase10-validation/phase10-diag-failure-forensics-0eaa17466aa04b05b3ee8e85c9d94a8d/proof.json) · [全库存内容补证](E:/Works/arkret-org/convert/office/target/review-phase10-validation/phase10-diag-failure-forensics-0eaa17466aa04b05b3ee8e85c9d94a8d/inventory-appendix.json)

唯一 WARN 的操作阶段为 blob_write_reservation、分类 sqlite_lock_contention，观测约3.005284秒；实际WS wire为 Error，不能改写成实际HTTP503。worker阶段为None，缺持锁者/TX-owner起止证据，故根因仍未知；同期PSI34.14%也不是因果证明。估计失败窗口只与约0.000400秒的capture重叠，最长约0.324208秒capture在窗口开始前约0.652844秒已结束，这些时点未建立observer直接致错的因果，间接扰动仍未知。 [失败与capture时点](E:/Works/arkret-org/convert/office/target/review-phase10-validation/allocator-diagnostic-20261111T153158Z-aefcc6e1-safe/failure-capture-timing.json)

45份XML完整可解析且未触cap，没有pending/unindexed文件，但只覆盖active partial；nearest baseline仍是loader早期样本，before_leave/after_30_seconds/after_readonly_convergence三个阶段缺失。估计bin-free增长与U值不是live Rust堆归属，不能从正常运行RSS中扣减。该诊断不推翻独立正常100k的协议结果，也未完成内存验收。随后 boundary-only 工具完成独立 C 16 项、Python 10 项小回归；这些是工具验证，不累加到产品 614 项，也不代表完整内存诊断通过。 [allocator summary](E:/Works/arkret-org/convert/office/target/review-phase10-validation/allocator-diagnostic-20261111T153158Z-aefcc6e1-safe/allocator-summary.json) · [四阶段覆盖缺口](E:/Works/arkret-org/convert/office/target/review-phase10-validation/allocator-diagnostic-20261111T153158Z-aefcc6e1-safe/four-phase-coverage.json) · [58文件byte-only安全导出](E:/Works/arkret-org/convert/office/target/review-phase10-validation/allocator-diagnostic-20261111T153158Z-aefcc6e1-safe/byte-only-export-proof.json)

新的 boundary-only 唯一业务诊断在 **78,465 次 ACK / 78,466 次尝试**后退出1，首错实际 WS wire 为 **RateLimit: Deferred object cleanup backlogged**，与前次 reservation 锁竞争错误不同。停止后取证确认版本78466、客户端高水78465，receipt 与 operation_log 各2,048，78,465条ACK记录、最终内容及quota一致；pending78466没有提交新版本、receipt或日志。1,026个非deleted对象均存在，未发现pending.new；原DB/WAL/SHM及原false结果哈希未变。GC为1,024个ref0对象，其中16个claim lease有效、1,008个未领取；库存为16 deleting、1,010 ready、77,441 deleted，无writing。终态不足以判定失败当时删除正在等待哪一步，同期PSI full44.75%也仅为相关观测。 [boundary原失败结果](E:/Works/arkret-org/convert/office/target/review-phase10-validation/boundary-diagnostic-terminal-20261111T160854Z-341c7931-c246eb10-safe/result.json) · [耐久与库存取证](E:/Works/arkret-org/convert/office/target/review-phase10-validation/phase10-diag-failure-forensics-d2c0d33f3e164ab89a506105c8f9238e/proof.json)

该诊断仅取得seq0基线，离开前、30秒后、只读收敛后三阶段均未执行。安全collector退出0、完整2,493文件source-after相等、自有进程已退出，证明证据收集与身份核对完成；**原业务结果仍为false，完整四阶段仍为false**。两次partial诊断都不抵扣正常100k的RSS差额，也不推翻其协议/持久化通过范围。后续优先补P2阶段持锁者与GC进度观测；可评估首错后在原预算内保留memory tail的方案，但须不重试、不续期并继续标记failed partial，目前尚未实现。完整RSS及同进程复用仍需在稳定资源窗口下评估，不以反复重跑、强制trim或提高队列上限替代验收。 [collector与完整性范围](E:/Works/arkret-org/convert/office/target/review-phase10-validation/boundary-diagnostic-terminal-20261111T160854Z-341c7931-c246eb10-safe/collector-result.json) · [四阶段未完成](E:/Works/arkret-org/convert/office/target/review-phase10-validation/boundary-diagnostic-terminal-20261111T160854Z-341c7931-c246eb10-safe/boundary-coverage.json)

**本轮剩余验收。** 两平台614/0/5、严格Clippy及正常release、实际144、两个phase10 ZIP和Garage4各有独立通过记录；正常100k的协议、持久化、历史/GC收敛及同进程重开亦已完成。RSS收敛后仍高于基线94,113,792字节，完整内存验收未完成，两个分配器诊断的partial失败与缺失阶段保留如上。远端CI仍缺真实执行（旧失败不能说明当前账户状态）、本地Docker被自动审批拒绝，以及macOS/AWS实际环境验收仍未完成。索引工具早期SYNTHETIC 8/8仅为工具回归，不代替两包实际owned DB索引结果。 [仅SYNTHETIC索引工具回归](E:/Works/arkret-org/convert/office/target/review-phase10-validation/SYNTHETIC-index-probe-tool-597554f04a2742c889f92458de2c47d0/SYNTHETIC-summary.json)

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
| S04 | [活动会话校验](E:/Works/arkret-org/convert/office/crates/server/src/services/wopi_sessions.rs:25) | [撤销及禁用](E:/Works/arkret-org/convert/office/crates/server/tests/security_regressions.rs:503)、[改密后禁止旧 WOPI 读写/签发](E:/Works/arkret-org/convert/office/crates/server/tests/security_regressions.rs:421) |
| S05 | [共享文件 ACL](E:/Works/arkret-org/convert/office/crates/server/src/handlers/access.rs:105) | [五角色跨协议矩阵](E:/Works/arkret-org/convert/office/crates/server/tests/acl_matrix_regressions.rs:141)、[撤权后分享失效](E:/Works/arkret-org/convert/office/crates/server/tests/security_regressions.rs:628) |
| S06 | [邀请详情授权](E:/Works/arkret-org/convert/office/crates/server/src/handlers/invitations.rs:230) | [两类邮箱保密与无能力消费拒绝](E:/Works/arkret-org/convert/office/crates/server/tests/security_regressions.rs:295) |
| S07 | [长连接持续授权](E:/Works/arkret-org/convert/office/crates/server/src/handlers/ws.rs:18) | [只读 key 拒绝写连接](E:/Works/arkret-org/convert/office/crates/server/tests/messaging_regressions.rs:219)、[空闲撤权](E:/Works/arkret-org/convert/office/crates/server/tests/messaging_regressions.rs:775) |
| S08 | [原子重置密码](E:/Works/arkret-org/convert/office/crates/server/src/handlers/auth.rs:972) | [单次消费与事务回滚](E:/Works/arkret-org/convert/office/crates/server/tests/security_regressions.rs:1408) |
| D01 | [CAS 与原子提交](E:/Works/arkret-org/convert/office/crates/server/src/services/persistence.rs:652) | [双 PUT 屏障、冲突及下载一致性](E:/Works/arkret-org/convert/office/crates/server/tests/persistence_regressions.rs:245) |
| D02 | [版本恢复](E:/Works/arkret-org/convert/office/crates/server/src/handlers/documents.rs:976) | [files 故障回滚](E:/Works/arkret-org/convert/office/crates/server/tests/persistence_regressions.rs:361)、[documents 故障回滚](E:/Works/arkret-org/convert/office/crates/server/tests/persistence_regressions.rs:366)、[配额故障回滚](E:/Works/arkret-org/convert/office/crates/server/tests/persistence_regressions.rs:371) |
| D03 | [UUID 对象键](E:/Works/arkret-org/convert/office/crates/server/src/services/storage.rs:281) | [本地同名并发读删](E:/Works/arkret-org/convert/office/crates/server/tests/persistence_regressions.rs:513)、[真实 S3 高层同名并发保存](E:/Works/arkret-org/convert/office/crates/server/tests/real_s3_acceptance.rs:99) |
| D04 | [分块上传](E:/Works/arkret-org/convert/office/crates/server/src/handlers/upload.rs:253)、[幂等完成](E:/Works/arkret-org/convert/office/crates/server/src/handlers/upload.rs:606) | [并发完成只计费一次](E:/Works/arkret-org/convert/office/crates/server/tests/persistence_regressions.rs:572)、[失败可重试](E:/Works/arkret-org/convert/office/crates/server/tests/persistence_regressions.rs:615) |
| D05 | [事务独占锁](E:/Works/arkret-org/convert/office/crates/server/src/services/persistence.rs:1210)、[唯一索引](E:/Works/arkret-org/convert/office/migrations/025_atomic_persistence.sql:3) | [锁竞争及撤权](E:/Works/arkret-org/convert/office/crates/server/tests/persistence_regressions.rs:635)、[过期锁竞争及旧 token 拒绝](E:/Works/arkret-org/convert/office/crates/server/tests/persistence_regressions.rs:708) |
| D06 | [操作与 receipt 持久提交](E:/Works/arkret-org/convert/office/crates/server/src/services/persistence.rs:831) | [提交后 ACK](E:/Works/arkret-org/convert/office/crates/server/tests/messaging_regressions.rs:467)、[失败不 ACK](E:/Works/arkret-org/convert/office/crates/server/tests/messaging_regressions.rs:618)、[真实进程重启](E:/Works/arkret-org/convert/office/scripts/test-package.ps1:370) |
| D07 | [按文档隔离](E:/Works/arkret-org/convert/office/crates/collab/src/manager.rs:40)、[原子退休与独立重试](E:/Works/arkret-org/convert/office/crates/collab/src/manager.rs:146)、[有界历史读取](E:/Works/arkret-org/convert/office/crates/server/src/services/collaboration.rs:100) | [重新加入竞态](E:/Works/arkret-org/convert/office/crates/collab/src/manager.rs:321)、[暂停时钟重试策略](E:/Works/arkret-org/convert/office/crates/collab/src/manager.rs:207)、[最后连接的所有权释放](E:/Works/arkret-org/convert/office/crates/collab/src/manager.rs:404)、[裁剪后的旧操作](E:/Works/arkret-org/convert/office/crates/server/tests/messaging_regressions.rs:313) |
| F01 | [快照三方合并](E:/Works/arkret-org/convert/office/crates/frontend/src/pages/calc/snapshot_merge.rs:89) | [dirty 与历史保留](E:/Works/arkret-org/convert/office/crates/frontend/src/pages/calc/glory_calc.rs:3365)、[双浏览器](E:/Works/arkret-org/convert/office/e2e/tests/review.ui.spec.ts:89) |
| F02 | [共享预算与求值缓存](E:/Works/arkret-org/convert/office/crates/doc-engine/src/calc.rs:1385) | [重复依赖计数](E:/Works/arkret-org/convert/office/crates/doc-engine/src/calc.rs:3481)、[预算拒绝](E:/Works/arkret-org/convert/office/crates/doc-engine/src/calc.rs:3510) |
| F03 | [同一导出实现及实际访问计数](E:/Works/arkret-org/convert/office/crates/export/src/xlsx.rs:168) | [近端/远端同格数及标准 XML](E:/Works/arkret-org/convert/office/crates/export/tests/review_regressions.rs:33) |
| F04 | [外部公式读取](E:/Works/arkret-org/convert/office/crates/export/src/xlsx.rs:1001) | [无原生缓存的外部 fixture](E:/Works/arkret-org/convert/office/crates/export/tests/review_regressions.rs:15) |
| F05 | [转换前重算](E:/Works/arkret-org/convert/office/crates/server/src/handlers/convert.rs:127) | [真实 CSV 结果](E:/Works/arkret-org/convert/office/crates/server/src/handlers/convert.rs:939)、[文本/错误/依赖链缓存重算](E:/Works/arkret-org/convert/office/crates/server/src/handlers/convert.rs:955)、[浏览器保存后下载](E:/Works/arkret-org/convert/office/e2e/tests/review.ui.spec.ts:16) |
| F06 | [保留原生字段](E:/Works/arkret-org/convert/office/crates/frontend/src/pages/calc/glory_calc.rs:199) | [未编辑类型保留](E:/Works/arkret-org/convert/office/crates/frontend/src/pages/calc/glory_calc.rs:3332)、[新格格式化及重载](E:/Works/arkret-org/convert/office/crates/frontend/src/pages/calc/glory_calc.rs:3552)、[旧格式损坏仍可计算](E:/Works/arkret-org/convert/office/crates/frontend/src/pages/calc/glory_calc.rs:3626) |
| F07 | [标准样式表](E:/Works/arkret-org/convert/office/crates/export/src/xlsx_styles.rs:11) | [标准样式 parts 与索引](E:/Works/arkret-org/convert/office/crates/export/tests/review_regressions.rs:33)、[独立阅读器格式/颜色/边框/对齐](E:/Works/arkret-org/convert/office/crates/export/tests/verify_office_readers.py:20) |
| F08 | [外部页序、布局与元素](E:/Works/arkret-org/convert/office/crates/export/src/pptx_import.rs:991)、[标准导入入口](E:/Works/arkret-org/convert/office/crates/export/src/pptx.rs:1067) | [同 fixture 重排与丰富元素](E:/Works/arkret-org/convert/office/crates/export/tests/review_regressions.rs:220)、[浏览器原件逐字节恢复](E:/Works/arkret-org/convert/office/e2e/tests/review.ui.spec.ts:118) |
| O01 | [metadata 与路径前置检查](E:/Works/arkret-org/convert/office/scripts/workspace.ps1:2) | 正常环境 metadata、PowerShell 解析、调用者环境不变验证 |
| O02 | [完整容器构建](E:/Works/arkret-org/convert/office/docker/Dockerfile:1) | [D 盘隔离 builder 实际构建](E:/Works/arkret-org/convert/office/target/office-container-build-phase3.log)与[镜像运行验收](E:/Works/arkret-org/convert/office/target/package-smoke/72406505711549ce8171b27f08136c32/result.json) |
| O03 | [完整 ZIP 组装](E:/Works/arkret-org/convert/office/scripts/package.ps1:1) | [ZIP 和镜像共用无源码启动、资源哈希、浏览器登录保存与重启验收](E:/Works/arkret-org/convert/office/scripts/test-package.ps1:1) |
| O04 | [共享事务 purge](E:/Works/arkret-org/convert/office/crates/server/src/services/persistence.rs:1259) | [清理释放配额](E:/Works/arkret-org/convert/office/crates/server/tests/persistence_regressions.rs:914)、[保留对象去重计费](E:/Works/arkret-org/convert/office/crates/server/tests/persistence_regressions.rs:1350) |

## 当前剩余边界与后续事项

- Docker 历史 phase3 镜像的 D 盘构建与浏览器验收仅属于旧输入；当前源码镜像尚未构建验收。已证实的审批拒绝针对 Diagnostic-A 额外诊断卷/容器创建启动流程，未找到另一次新 builder 创建拒绝的原证，原泛化表述撤回。Desktop 数据盘仍在 C；构建前须取得实际 D/远端存储证明。远端 22ab8a4 的 CI43122967873 为7个 failed job、均 runner0/steps0，另2个 skipped，原因未知；Intel macOS 尚缺实际运行。
- 第二轮外部 PPTX/XLSX 的实现与能力限制见上文；这些是当前模型可表达的兼容能力，不等同完整 Microsoft Office 兼容性认证或精确排版/播放承诺。
- Calc 可自动合并不同单元格的并发修改；同格不同输入、并发结构或非单元格元数据变化保守提示冲突。离线 CRDT 合并不在当前协议中，历史窗口外操作需要重新同步。
- 历史保留、对象登记与分页回收仍依赖后台最终收敛。当前固定查询 worker 候选已完成双端完整624、strict Clippy、正常 release、144、两个ZIP/冷备、Garage4及两次100k功能验证；当前normal/仪表离开后均无RSS下降，较基线分别高15,958,016 / 16,211,968字节，内存目标继续开放。旧phase9/phase10 normal残留104,259,584 / 94,113,792及c04/f5仪表残留96,043,008字节按旧输入保留。所有权/Weak、GC清零或allocator free均不能替代RSS实测；真实缓存和所有权归因仍待直接观测，进程强停亦不等于真实断电。
- 正常超限上传在返回 413 前按帧排空，累计预算为 multipart 上限加 1 MiB、总计 2 秒，不按声明长度分配内存；成功 multipart 也需要完成 HTTP framing。超出排空预算或超时会关闭连接，不能保证无限输入、认证/准入前拒绝或持续恶意发送者总能收到完整 JSON。
- WOPI 临时 capability 存于进程内，重启后需重新交换。空闲长连接每 2 秒复核凭证，消息处理和推送前另行复核。合法邀请 token 仍是 bearer capability，未新增邮箱所有权验证流程。
- 样式 CSP 保留 inline 以兼容现有控件；脚本使用 hash/nonce 与 WASM 所需策略。Unix symlink 与本地扫描/删除专项已在本次 Linux Rust 套件真实运行通过；Windows junction 已有动态专项。macOS 尚未验收。
- 平台结果分别按 Windows 本机与 Ubuntu 24.04 x86_64 / WSL2 列出，不能推广到全部 Linux 发行版、架构或 macOS。真实 Garage S3 兼容服务及性能工具已有第二轮工作；不将准备好的脚本或旧版本性能结果当作当前版本验收。未连接 AWS 生产桶，未给出硬件无关 SLO、全面 WCAG 或全设备支持结论。
- 管理员备份接口本身仍只生成 SQLite 快照，不复制或固定 blob，DB-only 快照不能保证文件恢复。历史 phase10 的两个 ZIP 已通过独立停写、数据库+有效 WAL+匹配 blob/上传原件的一致冷备与全新路径恢复演练，并通过缺失/篡改负例；这项外部演练不等于管理员接口已有全量备份能力。本轮演练仅覆盖本地后端；共享桶或多实例的数据库、对象与 GC 协调仍需另行验收，AWS 专用桶及真实断电恢复尚未验收。 当前两个phase12 ZIP亦已完成该外部冷备演练。
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
