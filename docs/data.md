# 数据

[English](data.en.md)

## 全新环境初始化

先创建隔离的空 MySQL database，再通过 `APP_ENV` 选择 `dev`、`test` 或 `prod`。`config/app.toml` 可省略，所选 `config/app.<environment>.toml` 必须存在；省略公共文件时，环境文件须完整提供必填配置；数据库密码、JWT 和 `[bootstrap] admin_password` 直接写入明文配置，准备方式见 [README](../README.md#下载后首次运行)。用 `--config-dir <directory>` 选择其他目录，应用相对路径以当前工作目录解析。默认单租户仍有控制库与 `shared-control` 两个独立账本，从后端根目录依次运行： 本机沿用 README 的忽略目录时，本文默认 `config` 的 Cargo 别名示例须在 `--` 后传 `--config-dir .local-tests/dev-config`，例如 `cargo migrate -- --config-dir .local-tests/dev-config control verify` 或 `cargo serve -- --config-dir .local-tests/dev-config`；后续校验、导出和运行继续选择同一目录，避免连接另一份未初始化配置。

```powershell
cargo migrate -- control up
cargo migrate -- control verify
cargo migrate -- tenant-data up --target shared-control
cargo migrate -- tenant-data verify --target shared-control
cargo serve
```

迁移路径使用 `control up` 新建或升级控制库；全新空物理库也可选下文完整 SQL 一次初始化；`control verify` 只校验账本、结构和编译期业务目录，新空库上不能代替初始化。首次初始化需要配置 `[bootstrap] admin_password`，API 与 Worker 启动时只校验，不执行 DDL。非生产默认 `jobs.mode="embedded"`，`cargo serve` 已消费后台任务；验证独立进程时按[开发指南](development.md#启动后端)创建 API／Worker 两份私有配置，均设置 `jobs.mode="external"`，采用不同实例编号并显式传入各自配置目录。每个 MySQL 租户目标都要先创建空库、登记配置并单独执行 `tenant-data up/verify`。

## 控制库初始化 SQL

```powershell
cargo migrate -- control export-sql --output .local-tests/control-init.sql --plan
cargo migrate -- control export-sql --output .local-tests/control-init.sql --write
```

`control export-sql --plan` 不读密码、应用配置或数据库，也不写文件；实际 `--write` 导出通过 `APP_ENV` 和 `--config-dir <directory>` 加载文件配置并读取 `[bootstrap] admin_password` 生成哈希，不连接数据库。正式导出覆盖同一物理库的控制面与 `shared-control` 租户数据面：控制库结构、后置约束、系统数据、fence/slot/投影基础表和初始化行、两个框架迁移账本，以及已注册 Control 与 Tenant 业务初始化迁移及单一业务账本，不包含明文密码；手写业务迁移必须提供与 `up` 同源的 schema 和初始化 SQL。导出文件含密码哈希，须按凭据保管；输出文件必须新建，不能覆盖旧文件。选择空库并使用客户端遇错停止，不使用 `--force`。下例为本机库；客户端主机、端口、账号和数据库须与所选文件一致，远程生产还须显式选择主机名校验和相应 CA，不能依赖客户端默认 TLS 模式。PowerShell 使用：

```powershell
mysql --default-character-set=utf8mb4 -h 127.0.0.1 -P 3306 -u ryframe_app -p ryframe_dev --execute="source .local-tests/control-init.sql"
```

bash 可使用 `< .local-tests/control-init.sql`；PowerShell 不支持该重定向，不要把 bash 命令原样用于 PowerShell。导入后执行：

```powershell
cargo migrate -- control verify
cargo migrate -- tenant-data verify --target shared-control
```

成功导入后直接完成上述两侧 verify，无需补 `up`；独立 MySQL 租户目标不在此物理库导出范围内，仍须使用各自 `tenant-data up/verify`。`sql/ryframe_config.sql` 是含占位哈希、没有账本的审阅快照，不能部署；导入中断后不要通过重复 `up` 猜测补齐或自动删除对象，确认用途后另建隔离库。MySQL DDL 不能依赖外层事务整体回滚。导入按迁移执行前置与执行后实际元数据断言，覆盖列顺序、默认值、排序规则、索引、外键及 `CHECK` 表达式和 `enforced`；仅通过后登记本次业务账本。最终复核框架、业务结构及归属/账本，并在持有元数据锁的登记事务内写入两侧框架完成标记。失败前已完成的迁移及 DDL 可能保留；错误结果先核对输出的迁移、阶段、类别与现场，禁止自动重放。

## 数据边界与迁移

控制库保存身份、授权、产品、租户目录、任务、文件元数据和 placement。无 `tenant_id` 的控制库业务默认只允许系统租户访问；租户业务通过 placement 路由到 `shared-control`、共享业务目标或独立租户目标。控制库与租户数据面使用不同账本，编译期目录记录表、列、游标和 schema；启动、`verify` 或生成器发现缺失、漂移、未知账本和归属冲突时失败关闭，不自动建表或改写授权。 `shared-control` 校验仅排除组合根正式登记为 control 的业务表；这些表仍由 `control verify` 校验结构，不进入租户复制目录或独立 MySQL 目标许可集合。未知或缺失业务表会列出表名；若 `tenant-data up` 已完成 DDL 后校验失败，先保留现场并运行只读 `control verify` 与 `tenant-data verify` 核对归属和账本，勿删库或自动重放。

当前 `0.x` 只面向全新隔离库。旧结构或无效能力代码不会被自动转换；`v1.0.0` 后冻结迁移只追加并 roll-forward。迁移账号需要读取业务表元数据。DDL 可能隐式提交，账本事务不保证整个迁移回滚；执行后校验失败应保留现场，不能假定回滚或自动重放。常用检查为：

```powershell
cargo migrate -- control status
cargo migrate -- control verify
cargo migrate -- tenant-data status --all
cargo migrate -- tenant-data verify --all
```

框架维护者可用 `cargo xtask data migrate check` 只读核验迁移目录、注册顺序、生成迁移接线和 `catalog/migrations.lock.toml` 摘要；提交前检查使用 `cargo xtask data migrate check --require-frozen`。开发版显式重整基线使用 `cargo xtask data migrate baseline --write`，稳定版只冻结当前工作树中的新迁移，使用 `cargo xtask data migrate freeze`。`cargo xtask data migrate reconcile --write` 按当前版本阶段选择 baseline 或 freeze，全栈夹具复用它。以上命令不连接或修改数据库。

## 已有表接管

已有表只能显式接管。在正常控制库另加未登记的业务表后，启动校验拒绝未知表属于预期边界；保留表及数据，不能把生成模型后直接 `serve` 当成接管。先停止 API/Worker 等生产者，准备已登记模块及匹配模型、迁移，并使用包含该登记的当前迁移程序。接管前置只读核验控制库框架账本、完整物理结构和目标边界；新增业务权限/菜单不必预先 seed，业务结构、物理目标、来源摘要和其他账本约束仍由接管计划逐次核对。保存预览和 inspect 不写数据库，apply 只登记已核对的 adopted 账本元数据，不执行历史业务 DDL 或更改业务行。应用后先 inspect 原计划；需要初始化新增访问目录或执行已审阅的后续迁移时，再显式执行对应范围的 `up`，最后完整 verify。以下新模块示例用 `control up` 初始化访问目录，再校验；API/Worker 启动前仍要求完整业务权限与菜单通过，不存在宽松启动路径：

```powershell
cargo migrate -- control adopt --module order --output .local-tests/order-adopt.json --write
cargo migrate -- control adopt --inspect .local-tests/order-adopt.json
cargo migrate -- control adopt --apply .local-tests/order-adopt.json --write
cargo migrate -- control adopt --inspect .local-tests/order-adopt.json
cargo migrate -- control up
cargo migrate -- control verify
```

租户表的接管、inspect、目标迁移和 verify 使用 `tenant-data` 并显式提供 `--target business-shared`，不接受 `--all`；业务权限/菜单仍由控制库的 `control up` 初始化并经 `control verify` 核验，不能把这一步改成只迁移租户目标。`--output` 必须配合 `--write`，`--inspect` 禁止写入，计划只能使用 `.local-tests` 内的新路径。`adopt --plan` 不读取应用配置、不连接数据库、不写入；它与实际读取结构并保存接管计划的 `adopt --output ... --write` 不同。接管只登记现有结构，不执行历史 DDL、不更改业务行；部分登记、结构漂移或未知结果先 inspect 原计划，不能换计划重放。目标为空时按正常 `up` 建表。

授权、任务领取、导出申请、删除受理和配置迁移使用主库与显式事务。可接受陈旧的列表读才使用 eventual consistency；执行前校验、下载和状态转换使用 strong consistency。租户数据范围由 `sys_tenant.data_scope_version` 驱动：控制库事务写入 outbox，Worker 在 placement 目标发布并原子切换投影；认证主体版本落后或投影不可用时失败关闭。两个数据库之间没有分布式事务，已并发进入租户库的写入不能被控制库回滚。

## 备份、恢复与保留

正式备份前停止 API、Worker、调度和消息生产者。库存以 `--quiesced-at` 表明外部已静止；`--observed-at` 只记录观察起点，不能作为正式备份资格。备份登记验证文件大小和 SHA-256，恢复数据校验比较当前 schema、完整表摘要、placement、ownership 和对象清单，资源与登记元数据不参与业务摘要。隔离恢复必须使用新 scope、数据库和对象前缀，保留目标 ownership；通过数据验证、API/Worker 就绪和真实业务证明后才登记成功。参考目标是 24 小时恢复点、60 分钟 RTO，实际结果不能由计划或单次库存推断。下列输出父目录须已存在，目标文件必须不存在；详细恢复命令见[运维指南](operations.md#备份失败或过期)。

```powershell
cargo xtask data backup inventory --output .local-tests/backup/inventory.json --source-sha <40-character-sha> --quiesced-at <RFC3339>
cargo xtask data target inventory --target <target-key> --output .local-tests/backup/target-inventory.json
```

开发复制先用 `cargo xtask check recovery clone plan --input <manifest> --output <new-plan> --write` 固定计划，再用 `cargo xtask check recovery clone verify --plan <plan>` 只读复核。两个入口由 Rust 原生执行，不启动 Python，也不访问数据库、网络或共享服务；严格核验输入对象字段、16 MiB 上限、schema/业务目录、SQL 数据转储、对象大小与 SHA-256、租户 placement/fence、任务关系和确定性计划哈希。`bridge --build <receipt> --inventory <manifest> --output <new-bridge-file> --write` 只在 Rust 中核验明确构建收据、完整来源快照、文件集合、产物字节、构建上下文和当前产品输入后登记复用边界，不编译、不改旧收据、不取得恢复资格；输出必须 create-new。`maintenance --operation build --output <new-directory> --write` 通过统一进程层以 Cargo 离线模式分别定向构建 `reset`、`migrate`、`tenant-data` 三个维护 CLI，冻结二进制、Cargo JSON 事件、日志、源码、工具链和 target 收据；失败留下 `failed.json` 与原始日志，不用成功收据掩盖部分写入。`maintenance --operation verify --output <directory-or-build.json>` 只读复核冻结收据、当前源码与工具链，重复构建已有输出会在启动 Cargo 前拒绝。`storage` 同样由 Rust 接管：RustFS 固定请求、数据目录身份、原始启动和实际进程代次、监听端口、服务日志、失败与未知启动收据、服务身份变化和精确进程回收都在原生入口核验；`storage status` 不创建 attempt 或证据文件，其他操作要求显式 `--write`。`cache` 的 successor 与前代工具换代路径尚未完整迁移，因此本版本继续使用原状态机，不保留半完成的 Rust 入口。

`source_to_seed` 的 `init`、只读 `status` 和 `stage` 运行目录记录、内核互斥、控制器身份、单调 attempt、来源终检及结果发布由 Rust 执行；`init/status` 不启动 Python。`stage export` 也已迁至 Rust：预检读取固定源请求、环境、构建、运行和 producer 收据，执行阶段只读核验 MySQL/对象 ownership、迁移、完整 schema、前后库存、SQL 转储、对象字节、调度关系和 generation/storage 漂移，所有本地证据在 `.local-tests` 内 create-new 发布；`stage target-verify` 由 Rust 编排完整数据库、对象与 Redis 只读复核，执行前后保护初始化文件、配置和实际进程身份，完整历史及恢复谱系仍通过私有只读 Python 协议核验；MySQL 原始输出持有同一文件句柄，成功收据先完整写入并同步临时文件，再排他发布。`copy` 继续使用单一私有 Python 子进程，只产生本阶段业务结果，不持有第二套生命周期状态机。`seed_to_arm` 仍需目标文件树、外部存储代次和已发布来源深层校验，因此其 `init/status/stage` 整体继续使用原 Python 状态机，不在 Rust 中留下部分迁移。运行目录、证据和输出必须位于当前后端 `.local-tests`，拒绝重复字段、链接、越界、未知写入、来源与目标重叠和未登记资源；计划、桥接和注册目录只以 create-new 写入，状态收据原子发布。其余 `runtime`、`recover`、post-copy、seed 和 cache 阶段继续使用现有私有协议。中断后先用只读 `clone status` 或 `storage status` 和完整前后像确定下一动作；只能按原账本执行 `reconcile`、`resume` 或绑定创建身份的 `recover`，不能重放未知写入。API、Worker、Redis 和存储重启分别登记配置、凭据引用、端口、进程身份和数据目录。source 代次 start/stop 失败后先查 `source-generation-status`，普通恢复只回收同代已登记进程，不补签历史前像或数据零漂移。 源后继启动的冻结请求、成对构建等价性、普通恢复完整前像及 C69 收据类型由同一绑定 Rust 工具只读核验；仅允许环境摘要和维护构建日志内容的既定归一化。守卫成功不授予启动或重放资格，失败保留原阶段现场。 复制、fresh-target、seed source 与 clone stage 的私有请求通过单次有界 UTF-8 JSON 标准输入传递，不进入业务 argv 或子进程环境；长时恢复阶段不增加总超时，中断或退出不自动重试。

迁移初始化续作的状态判定和成功确认由 Rust 只读核验；Python 控制器通过私有标准输入协议提交固定请求、prepare 快照和锁状态。核验绑定连续迁移前缀、完整阶段文件集合、可信服务重启代次、Redis 标记重绑及唯一允许的只读失败前驱；失败保留原始证据，不重放未知写入。外部运行代次的来源继续复用独立可信来源校验器，不以 started 内嵌值替代；来源核验后再次执行原生证据终检。

原生身份、配额和部门生产者在读取凭据或开始业务操作前，核验同一运行目录中最新通过的 seed 登记及其原始文件摘要；最新登记失败或未收尾时拒绝回退。启动授权结束前再次核对固定清单、完整阶段记录和登记结果，发现来源变化时停止执行，保留原始证据。登记操作的文件验证与发布也由绑定的 Rust 工具执行：外层控制器继续持有原 run/target 锁，原生入口核对当前 attempt、存活控制器、目标锁身份、最新 post-copy 代次与明确的文件配置。相同登记可以复核重用；冲突或部分文件不会被覆盖。登记文件已写入但外层阶段未通过时仍不具备启动资格，须保留现场并先核对阶段和锁。`prepare/start` 的完整复制历史及资源守卫仍由现有编排执行，不能用登记发布成功代替。身份生产者在正式执行、身份发布边界和收尾重新核对固定 post-copy、计划与存活控制器；配额和部门协议在每次业务操作前后复核同一代次。输入截断、重复字段、超限或输出通道中断时仍关闭已持有会话；显式退出不依赖后续来源继续有效，业务与清理同时失败时不能返回成功。

部门计划采用链的只读命令、原生 S3 收据、owner 内容、MySQL 输出与守卫文件闭集由 Rust 核验并以 create-new 发布清单；出现未登记文件、来源漂移或部分证据时拒绝采用，保留原现场。身份前提复用同一 Rust 有序账本投影，完整核对 622 项写入、11 个角色和 200 个用户。十租户计划、受限 XLSX 的完整部门路径、目录与模板前后像、幂等意图、逐次核对、唯一确认及身份授权证据也由 Rust 只读核验；统一阶段选择器仍负责选择最新可信阶段。模型请求使用带摘要的受管临时输入文件，处理后复核输入未变，不扩大 stdin 边界，不新增环境入口。会话关闭预算固定覆盖十个管理员、十个模板会话和 pacing 清理，不因后续来源漂移阻断退出。部门远端操作与失败计划采用编排仍由现有持锁阶段执行，证据通过不代表获得写入、Worker 或恢复授权。

导出任务固化规范筛选、授权指纹、`snapshot_at` 和 `upper_id`；指定 ID 仍受租户及数据权限约束。默认上限为 50 万行、每批 1000 行、1800 秒、512 MiB 和每租户两个运行中任务。删除先写 `delete_pending_at`，再幂等清理对象与记录；对象不存在视为成功，存储失败保留 tombstone 重试。每个部署使用稳定唯一的 `scope_id`，Redis、对象和数据库均校验 ownership；禁止 `FLUSHDB`、`KEYS`、模糊 bucket 删除和扫描共享服务器。

## 文件配置与凭据

`APP_ENV` 是唯一项目配置环境入口，只接受精确的 `dev`、`test`、`prod`，不接受大小写变体或空白；普通 API、Worker 和迁移进程未设置时选 `dev`，reset 则要求显式设置，操作示例始终主动选择环境。API、Worker、迁移和 reset 从 `--config-dir <directory>` 指定的目录读取可选公共 `app.toml`，再递归合并必须存在的 `app.<environment>.toml`；公共文件确实不存在时，环境文件须完整提供必填字段，已存在但损坏、不可读或为悬空链接的公共文件仍会报错；默认目录为 `config`，相对路径以当前工作目录解析。生产连接、密钥强度、TLS、实例编号和资源隔离校验继续保留。 本地化使用 TOML 的 `app.locales_dir`，默认 `locales`，普通应用相对进程当前工作目录解析；不读取 `APP_LOCALES_DIR`。资源缺失或加载失败时生产严格失败，开发环境可内嵌回退。

数据库、Redis、对象存储、JWT、监控 token 和初始管理员密码直接写入对应明文字段；真实配置只保存在忽略的本机目录，不进入日志、公开收据或版本库。根字段 `snowflake_worker_id` 在同一部署的每个实例上必须不同；`[bootstrap] admin_password` 显式提供管理员初始密码。普通初始化用户 `user`、新建和导入的普通用户仍使用 `123456` 并强制首次改密。数组在环境文件中整体替换：`cors.allow_origins`、`proxy.trusted_cidrs` 使用 TOML 字符串数组；`database.replicas`、`database.sources` 和 `tenant_data.targets` 使用完整 TOML 表数组，不能只覆盖其中一项。

生成器的 `--config-dir <directory>` 和 Builder `.configuration_directory(path)` 以后端 Workspace 根目录解析相对路径，从业务 crate 的 `src` 运行也不改变默认的 `workspace/config`。优先级为显式 CLI/Builder、本机 `.local-tests/tools.toml`、公共 `config/tools.toml`、`workspace/config`；工具文件中的 `[generator] config_dir` 可省略。共享 MySQL 测试固定读取后端根 `.local-tests/mysql/config`，公共 `app.toml` 可省略，`app.test.toml` 必须存在；完整连接与 TLS 参数来自 `database.primary`，不接受 Disabled。目录缺失仅跳过且未验收；已有非法或链接路径、缺失环境文件及无效配置均失败。文件 `scope_id` 参与唯一 schema，详细 opt-in 与证据边界见[集成验证](development.md#集成验证)；仅实际运行的用例构成对应服务证据，配置或门禁入口本身不代表完整真实链路验收。部署模板与私有配置目录的结构及 CI 工具测试已通过，真实容器、镜像和整链仍未验收，不能以局部测试代替整体门禁。

## 租户路由

`database.primary` 是控制库唯一写库，也是强一致读取和 `shared-control` 的来源；`database.replicas` 是同一控制库的只读副本；`database.sources` 是业务显式选择的命名 MySQL 来源。三者都不替代租户 placement。以下是数据库连接拓扑片段，须合并进完整应用配置，并非可单独启动的配置文件；密码占位值必须替换，证书路径按运行进程平台解析：

```toml
scope_id = "orders-production"
snowflake_worker_id = 1

[multi_tenancy]
enabled = true

[database]
migration_mode = "verify"

[database.primary]
host = "mysql-control.internal"
port = 3306
database = "ryframe_control"
username = "ryframe_app"
password = "replace-with-primary-password"
max_connections = 20
min_connections = 2
tls_mode = "verify_identity"
tls_ca = "/run/secrets/mysql-ca.pem"

[[database.replicas]]
name = "control-read-east"
host = "mysql-control-read.internal"
port = 3306
database = "ryframe_control"
username = "ryframe_reader"
password = "replace-with-replica-password"
max_connections = 8
min_connections = 1
tls_mode = "verify_identity"
tls_ca = "/run/secrets/mysql-ca.pem"

[[database.sources]]
name = "legacy"
host = "mysql-legacy.internal"
port = 3306
database = "legacy_orders"
username = "legacy_reader"
password = "replace-with-source-password"
max_connections = 4
min_connections = 0
tls_mode = "verify_identity"
tls_ca = "/run/secrets/mysql-ca.pem"

[tenant_data]
default_target = "business-shared"
max_open_targets = 4
max_total_connections = 24
idle_pool_secs = 300

[[tenant_data.targets]]
key = "business-shared"
kind = "mysql"
mode = "shared"
host = "mysql-business.internal"
port = 3306
database = "ryframe_business"
username = "business_app"
password = "replace-with-shared-target-password"
max_connections = 8
tls_mode = "verify_identity"
tls_ca = "/run/secrets/mysql-ca.pem"

[[tenant_data.targets]]
key = "tenant-acme"
kind = "mysql"
mode = "dedicated"
host = "mysql-acme.internal"
port = 3306
database = "ryframe_tenant_acme"
username = "acme_app"
password = "replace-with-dedicated-target-password"
max_connections = 6
tls_mode = "verify_identity"
tls_ca = "/run/secrets/mysql-ca.pem"
```

primary、replicas、sources 和 MySQL 租户目标均直接使用 `password` 字段，不读取凭据环境覆盖，也不接受 DSN；空密码仍须显式填写，能否认证由服务端决定。副本及 source 名称不能重复或互相冲突，source 不能使用 `primary`。MySQL 租户目标的主机、端口、库名组合不能与控制库、副本、source 或其他目标重叠；不同服务器允许同名库，不能只按库名判断身份。命名 source 可作为 primary 的显式别名，但仍受租户目标独立性检查约束。TLS 默认 `required`，它只要求加密；`verify_ca` 验证证书链，`verify_identity` 另核对主机名，两者都要求 CA；远程生产的上述连接和目标必须使用 `verify_identity`。mTLS 证书与私钥必须成对配置；隔离开发可显式禁用 TLS，但共享真实 MySQL 测试不接受 Disabled。

replica 只服务显式 `ReadConsistency::Eventual` 读取：健康副本轮询，全部不可用时在选池阶段回退 primary，不把已失败 SQL 自动切主重放；`Strong`、写后读、授权、任务、导出和状态转换使用 primary。启动不等待副本，后台重连并通过健康及结构核验后才加入路由；primary 或已配置的命名 source 连接失败则阻断启动。RyFrame 不建立 MySQL 主从复制或等待追平，延迟监控和故障切换由运维负责。`database.sources` 通过 `control_database.source("<name>")` 获取并返回 `Option`，不参与自动读路由；每个来源有独立连接池和事务，与 primary 的事务不共享提交或回滚，不自动分库，也没有跨库分布式事务。MySQL 租户目标没有副本配置，最终一致性读取也使用其自身目标池。生成器的结构来源不自动成为运行目标。

| 布局 | placement 目标 | 边界 |
| --- | --- | --- |
| 共享控制库 | 隐式 `shared-control`，`kind="control"`、`mode="shared"` | 默认单租户和系统租户；复用控制库池及显式一致性读策略，租户业务仍有独立迁移账本 |
| 共享业务库 | `kind="mysql"`、`mode="shared"` | 多租户共享 `biz_*` 表，查询仍按 `tenant_id` 隔离 |
| 独立业务库 | `kind="mysql"`、`mode="dedicated"` | 同一时刻一个 active 租户，可使用独立账号、容量和备份 |

新配置默认 `multi_tenancy.enabled=false`；在配置文件中显式设为 `true` 才开放租户选择和管理。`tenant_data.targets[].key` 是迁移、健康检查、API 和 placement 的稳定身份，数据库名不能代替。生成器 `source="local"` 是简写：`database=control` 时读取 primary；存储范围为 tenant 时读取 `tenant_data.default_target`，因此该键必须是 MySQL 目标，隐式 `shared-control` 不能作为 local 结构来源，也可直接写 `primary` 或 `tenant_data.targets.<key>` 消除歧义。新租户开通仍显式提交 `data_target_key`，运行时只以控制库 placement 为准；缺 placement、未知或不可达目标、fence 或代次不符时失败关闭，不退回 `default_target` 或 `shared-control`。

从单租户 `shared-control` 演进时，先在配置中增加共享或独立目标，创建空库并在目标的 `password` 字段填写密码，运行 `cargo migrate -- tenant-data up --target <target-key>` 与 `verify`，在“平台管理 → 数据目标”完成实时探测。新租户开通时选择目标键；已有非系统租户改用租户数据放置/迁移流程，先预览和校验 placement generation、计划哈希与幂等键，再复制数据并切换 placement。修改 `default_target`、DNS 或复制 schema 都不会迁移既有数据；系统租户禁止迁移业务数据，独立目标被占用时先完成迁移或清理。

独立目标池按需建立，每池默认上限为 `max_total_connections / max_open_targets` 的整数商，目标 `max_connections` 可覆盖；两项预算仅约束 MySQL 租户目标，不包含 primary、replicas、sources 或复用控制库池的 `shared-control`，API 与 Worker 各自计数。达到预算时仅回收无活动租约的闲置池；仍不足则拒绝新池，不挤占活动会话。`tenant-data up/verify/status` 使用目标键，`--all` 只处理登记目标；多个目标先核对物理实例与 schema，别名重复时在 DDL 前拒绝，失败后先查状态，不自动重放已完成目标。
