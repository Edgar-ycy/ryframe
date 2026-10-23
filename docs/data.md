# 数据

[English](data.en.md)

## 全新环境初始化

先创建隔离的空 MySQL database，再通过 `APP_ENV` 选择 `dev`、`test` 或 `prod`。`config/app.toml` 提供公共默认值，所选 `config/app.<environment>.toml` 必须存在；数据库密码、JWT 和 `[bootstrap] admin_password` 直接写入明文配置，准备方式见 [README](../README.md#下载后首次运行)。用 `--config-dir <directory>` 选择其他目录，应用相对路径以当前工作目录解析。默认单租户仍有控制库与 `shared-control` 两个独立账本，从后端根目录依次运行： 本机沿用 README 的忽略目录时，本文默认 `config` 的 Cargo 别名示例须在 `--` 后传 `--config-dir .local-tests/dev-config`，例如 `cargo migrate -- --config-dir .local-tests/dev-config control verify` 或 `cargo serve -- --config-dir .local-tests/dev-config`；后续校验、导出和运行继续选择同一目录，避免连接另一份未初始化配置。

```powershell
cargo migrate -- control up
cargo migrate -- control verify
cargo migrate -- tenant-data up --target shared-control
cargo migrate -- tenant-data verify --target shared-control
cargo serve
```

`control up` 是控制库唯一的新建/升级入口；`control verify` 只校验账本、结构和编译期业务目录，新空库上不能代替初始化。首次初始化需要配置 `[bootstrap] admin_password`，API 与 Worker 启动时只校验，不执行 DDL。非生产默认 `jobs.mode="embedded"`，`cargo serve` 已消费后台任务；验证独立进程时按[开发指南](development.md#启动后端)创建 API／Worker 两份私有配置，均设置 `jobs.mode="external"`，采用不同实例编号并显式传入各自配置目录。每个 MySQL 租户目标都要先创建空库、登记配置并单独执行 `tenant-data up/verify`。

## 控制库初始化 SQL

```powershell
cargo migrate -- control export-sql --output .local-tests/control-init.sql --plan
cargo migrate -- control export-sql --output .local-tests/control-init.sql --write
```

`control export-sql --plan` 不读密码、应用配置或数据库，也不写文件；实际 `--write` 导出通过 `APP_ENV` 和 `--config-dir <directory>` 加载文件配置并读取 `[bootstrap] admin_password` 生成哈希，不连接数据库。正式导出包含控制库结构、后置约束、系统数据、已注册控制库业务迁移和账本，不包含明文密码；手写业务迁移必须提供与 `up` 同源的 schema 和初始化 SQL。选择空库并使用客户端遇错停止，不使用 `--force`。PowerShell 使用：

```powershell
mysql --default-character-set=utf8mb4 -h 127.0.0.1 -P 3306 -u ryframe_app -p ryframe_dev --execute="source .local-tests/control-init.sql"
```

bash 可使用 `< .local-tests/control-init.sql`；PowerShell 不支持该重定向，不要把 bash 命令原样用于 PowerShell。导入后执行：

```powershell
cargo migrate -- control verify
cargo migrate -- tenant-data up --target shared-control
cargo migrate -- tenant-data verify --target shared-control
```

该 SQL 只初始化控制库。`sql/ryframe_config.sql` 是含占位哈希、没有账本的审阅快照，不能部署；导入中断后不要通过重复 `up` 猜测补齐或自动删除对象，确认用途后另建隔离库。MySQL DDL 不能依赖外层事务整体回滚。

## 数据边界与迁移

控制库保存身份、授权、产品、租户目录、任务、文件元数据和 placement。无 `tenant_id` 的控制库业务默认只允许系统租户访问；租户业务通过 placement 路由到 `shared-control`、共享业务目标或独立租户目标。控制库与租户数据面使用不同账本，编译期目录记录表、列、游标和 schema；启动、`verify` 或生成器发现缺失、漂移、未知账本和归属冲突时失败关闭，不自动建表或改写授权。

当前 `0.x` 只面向全新隔离库。旧结构或无效能力代码不会被自动转换；`v1.0.0` 后冻结迁移只追加并 roll-forward。迁移账号需要读取业务表元数据。DDL 可能隐式提交，账本事务不保证整个迁移回滚；执行后校验失败应保留现场，不能假定回滚或自动重放。常用检查为：

```powershell
cargo migrate -- control status
cargo migrate -- control verify
cargo migrate -- tenant-data status --all
cargo migrate -- tenant-data verify --all
```

框架维护者可用 `cargo xtask data migrate check` 只读核验迁移目录、注册顺序、生成迁移接线和 `catalog/migrations.lock.toml` 摘要；提交前检查使用 `cargo xtask data migrate check --require-frozen`。开发版显式重整基线使用 `cargo xtask data migrate baseline --write`，稳定版只冻结当前工作树中的新迁移，使用 `cargo xtask data migrate freeze`。`cargo xtask data migrate reconcile --write` 按当前版本阶段选择 baseline 或 freeze，全栈夹具复用它。以上命令不连接或修改数据库。

## 已有表接管

已有表只能显式接管。先导入或手写模型、登记描述符并生成匹配迁移，停止 API/Worker，再保存、检查和应用计划：

```powershell
cargo migrate -- control adopt --module order --output .local-tests/order-adopt.json --write
cargo migrate -- control adopt --inspect .local-tests/order-adopt.json
cargo migrate -- control adopt --apply .local-tests/order-adopt.json --write
cargo migrate -- control verify
```

租户表把 `control` 改为 `tenant-data`，并显式提供 `--target business-shared`；不接受 `--all`。`--output` 必须配合 `--write`，`--inspect` 禁止写入，计划只能使用 `.local-tests` 内的新路径。`adopt --plan` 不读取应用配置、不连接数据库、不写入；它与实际读取结构并保存接管计划的 `adopt --output ... --write` 不同。接管只登记现有结构，不执行历史 DDL、不更改业务行；部分登记、结构漂移或未知结果先 inspect 原计划，不能换计划重放。目标为空时按正常 `up` 建表。

授权、任务领取、导出申请、删除受理和配置迁移使用主库与显式事务。可接受陈旧的列表读才使用 eventual consistency；执行前校验、下载和状态转换使用 strong consistency。租户数据范围由 `sys_tenant.data_scope_version` 驱动：控制库事务写入 outbox，Worker 在 placement 目标发布并原子切换投影；认证主体版本落后或投影不可用时失败关闭。两个数据库之间没有分布式事务，已并发进入租户库的写入不能被控制库回滚。

## 备份、恢复与保留

正式备份前停止 API、Worker、调度和消息生产者。库存以 `--quiesced-at` 表明外部已静止；`--observed-at` 只记录观察起点，不能作为正式备份资格。备份登记验证文件大小和 SHA-256，恢复数据校验比较当前 schema、完整表摘要、placement、ownership 和对象清单，资源与登记元数据不参与业务摘要。隔离恢复必须使用新 scope、数据库和对象前缀，保留目标 ownership；通过数据验证、API/Worker 就绪和真实业务证明后才登记成功。参考目标是 24 小时恢复点、60 分钟 RTO，实际结果不能由计划或单次库存推断。下列输出父目录须已存在，目标文件必须不存在；详细恢复命令见[运维指南](operations.md#备份失败或过期)。

```powershell
cargo xtask data backup inventory --output .local-tests/backup/inventory.json --source-sha <40-character-sha> --quiesced-at <RFC3339>
cargo xtask data target inventory --target <target-key> --output .local-tests/backup/target-inventory.json
```

开发复制先用 `cargo xtask check recovery clone plan --input <manifest> --output <new-plan> --write` 固定计划，再用 `cargo xtask check recovery clone verify --plan <plan>` 只读复核。两个入口由 Rust 原生执行，不启动 Python，也不访问数据库、网络或共享服务；严格核验输入对象字段、16 MiB 上限、schema/业务目录、SQL 数据转储、对象大小与 SHA-256、租户 placement/fence、任务关系和确定性计划哈希。`bridge --build <receipt> --inventory <manifest> --output <new-bridge-file> --write` 只在 Rust 中核验明确构建收据、完整来源快照、文件集合、产物字节、构建上下文和当前产品输入后登记复用边界，不编译、不改旧收据、不取得恢复资格；输出必须 create-new。`maintenance --operation build --output <new-directory> --write` 通过统一进程层以 Cargo 离线模式分别定向构建 `reset`、`migrate`、`tenant-data` 三个维护 CLI，冻结二进制、Cargo JSON 事件、日志、源码、工具链和 target 收据；失败留下 `failed.json` 与原始日志，不用成功收据掩盖部分写入。`maintenance --operation verify --output <directory-or-build.json>` 只读复核冻结收据、当前源码与工具链，重复构建已有输出会在启动 Cargo 前拒绝。`storage` 同样由 Rust 接管：RustFS 固定请求、数据目录身份、原始启动和实际进程代次、监听端口、服务日志、失败与未知启动收据、服务身份变化和精确进程回收都在原生入口核验；`storage status` 不创建 attempt 或证据文件，其他操作要求显式 `--write`。`cache` 的 successor 与前代工具换代路径尚未完整迁移，因此本版本继续使用原状态机，不保留半完成的 Rust 入口。

`source_to_seed` 的 `init`、只读 `status` 和 `stage` 运行目录记录、内核互斥、控制器身份、单调 attempt、来源终检及结果发布由 Rust 执行；`init/status` 不启动 Python。`stage export` 也已迁至 Rust：预检读取固定源请求、环境、构建、运行和 producer 收据，执行阶段只读核验 MySQL/对象 ownership、迁移、完整 schema、前后库存、SQL 转储、对象字节、调度关系和 generation/storage 漂移，所有本地证据在 `.local-tests` 内 create-new 发布；`stage target-verify` 由 Rust 编排完整数据库、对象与 Redis 只读复核，执行前后保护初始化文件、配置和实际进程身份，完整历史及恢复谱系仍通过私有只读 Python 协议核验；MySQL 原始输出持有同一文件句柄，成功收据先完整写入并同步临时文件，再排他发布。`copy` 继续使用单一私有 Python 子进程，只产生本阶段业务结果，不持有第二套生命周期状态机。`seed_to_arm` 仍需目标文件树、外部存储代次和已发布来源深层校验，因此其 `init/status/stage` 整体继续使用原 Python 状态机，不在 Rust 中留下部分迁移。运行目录、证据和输出必须位于当前后端 `.local-tests`，拒绝重复字段、链接、越界、未知写入、来源与目标重叠和未登记资源；计划、桥接和注册目录只以 create-new 写入，状态收据原子发布。其余 `runtime`、`recover`、post-copy、seed 和 cache 阶段继续使用现有私有协议。中断后先用只读 `clone status` 或 `storage status` 和完整前后像确定下一动作；只能按原账本执行 `reconcile`、`resume` 或绑定创建身份的 `recover`，不能重放未知写入。API、Worker、Redis 和存储重启分别登记配置、凭据引用、端口、进程身份和数据目录。source 代次 start/stop 失败后先查 `source-generation-status`，普通恢复只回收同代已登记进程，不补签历史前像或数据零漂移。

原生身份、配额和部门生产者在读取凭据或开始业务操作前，核验同一运行目录中最新通过的 seed 登记及其原始文件摘要；最新登记失败或未收尾时拒绝回退。启动授权结束前再次核对固定清单、完整阶段记录和登记结果，发现来源变化时停止执行，保留原始证据。

导出任务固化规范筛选、授权指纹、`snapshot_at` 和 `upper_id`；指定 ID 仍受租户及数据权限约束。默认上限为 50 万行、每批 1000 行、1800 秒、512 MiB 和每租户两个运行中任务。删除先写 `delete_pending_at`，再幂等清理对象与记录；对象不存在视为成功，存储失败保留 tombstone 重试。每个部署使用稳定唯一的 `scope_id`，Redis、对象和数据库均校验 ownership；禁止 `FLUSHDB`、`KEYS`、模糊 bucket 删除和扫描共享服务器。

## 文件配置与凭据

`APP_ENV` 是唯一项目配置环境入口，只接受 `dev`、`test`、`prod`。API、Worker、迁移和 reset 从 `--config-dir <directory>` 指定的目录读取可选公共 `app.toml`，再递归合并必须存在的 `app.<environment>.toml`；公共文件确实不存在时，环境文件须完整提供必填字段，已存在但损坏、不可读或为悬空链接的公共文件仍会报错；默认目录为 `config`，相对路径以当前工作目录解析。生产连接、密钥强度、TLS、实例编号和资源隔离校验继续保留。 本地化使用 TOML 的 `app.locales_dir`，默认 `locales`，普通应用相对进程当前工作目录解析；不读取 `APP_LOCALES_DIR`。资源缺失或加载失败时生产严格失败，开发环境可内嵌回退。

数据库、Redis、对象存储、JWT、监控 token 和初始管理员密码直接写入对应明文字段；真实配置只保存在忽略的本机目录，不进入日志、公开收据或版本库。根字段 `snowflake_worker_id` 在同一部署的每个实例上必须不同；`[bootstrap] admin_password` 显式提供管理员初始密码。普通初始化用户 `user`、新建和导入的普通用户仍使用 `123456` 并强制首次改密。数组在环境文件中整体替换：`cors.allow_origins`、`proxy.trusted_cidrs` 使用 TOML 字符串数组；`database.replicas`、`database.sources` 和 `tenant_data.targets` 使用完整 TOML 表数组，不能只覆盖其中一项。

生成器的 `--config-dir <directory>` 和 Builder `.configuration_directory(path)` 以后端 Workspace 根目录解析相对路径，从业务 crate 的 `src` 运行也不改变默认的 `workspace/config`。优先级为显式 CLI/Builder、本机 `.local-tests/tools.toml`、公共 `config/tools.toml`、`workspace/config`；工具文件中的 `[generator] config_dir` 可省略。共享 MySQL 测试固定读取后端根 `.local-tests/mysql/config` 的 `app.toml` 与 `app.test.toml`，完整连接与 TLS 参数来自 `database.primary`，不接受 Disabled；目录缺失仅跳过且未验收，已有非法或链接路径、缺失文件及无效配置均失败。文件 `scope_id` 参与唯一 schema，详细 opt-in 与证据边界见[集成验证](development.md#集成验证)；真实协议未验收。部署模板与私有配置目录的结构及 CI 工具测试已通过，真实容器、镜像和整链仍未验收，不能以局部测试代替整体门禁。

## 租户路由

`database.primary` 是控制库唯一写库，也是强一致读取和 `shared-control` 的来源；`database.replicas` 是同一控制库的只读副本；`database.sources` 是业务显式选择的命名 MySQL 来源。三者都不替代租户 placement。以下是一个完整连接拓扑；密码占位值必须替换，证书路径按运行进程平台解析：

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

primary、replicas、sources 和 MySQL 租户目标均直接使用 `password` 字段；租户目标允许空密码，但字段必须存在，不接受 DSN。连接名称必须唯一，source 不能使用 `primary`，租户目标必须与其他控制库、来源和目标使用不同 schema。远程生产连接和租户目标必须 `verify_identity` 并配置 CA；`verify_ca` 也要求 CA，mTLS 证书与私钥必须成对配置。

replica 只服务显式 `ReadConsistency::Eventual` 读取：健康副本轮询，全部不可用时回退 primary；`Strong`、写后读、授权、任务、导出和状态转换使用 primary。RyFrame 不建立 MySQL 主从复制或等待追平，延迟监控和故障切换由运维负责。`database.sources` 通过 `control_database.source("<name>")` 获取并返回 `Option`；每个来源有独立连接池和事务，不自动分库，也没有跨库分布式事务。生成器的结构来源不自动成为运行目标。

| 布局 | placement 目标 | 边界 |
| --- | --- | --- |
| 共享控制库 | 隐式 `shared-control`，`kind="control"`、`mode="shared"` | 默认单租户和系统租户；复用 primary 池，租户业务仍有独立迁移账本 |
| 共享业务库 | `kind="mysql"`、`mode="shared"` | 多租户共享 `biz_*` 表，查询仍按 `tenant_id` 隔离 |
| 独立业务库 | `kind="mysql"`、`mode="dedicated"` | 同一时刻一个 active 租户，可使用独立账号、容量和备份 |

新配置默认 `multi_tenancy.enabled=false`；在配置文件中显式设为 `true` 才开放租户选择和管理。`tenant_data.targets[].key` 是迁移、健康检查、API 和 placement 的稳定身份，数据库名不能代替。生成器 `source="local"` 是简写：`database=control` 时读取 primary；存储范围为 tenant 时读取 `tenant_data.default_target`，因此该键必须是 MySQL 目标，隐式 `shared-control` 不能作为 local 结构来源，也可直接写 `primary` 或 `tenant_data.targets.<key>` 消除歧义。新租户开通仍显式提交 `data_target_key`，运行时只以控制库 placement 为准；缺 placement 或未知目标时失败关闭。

从单租户 `shared-control` 演进时，先在配置中增加共享或独立目标，创建空库并在目标的 `password` 字段填写密码，运行 `cargo migrate -- tenant-data up --target <target-key>` 与 `verify`，在“平台管理 → 数据目标”完成实时探测。新租户开通时选择目标键；已有非系统租户改用租户数据放置/迁移流程，先预览和校验 placement generation、计划哈希与幂等键，再复制数据并切换 placement。修改 `default_target`、DNS 或复制 schema 都不会迁移既有数据；系统租户禁止迁移业务数据，独立目标被占用时先完成迁移或清理。

独立目标池按需建立，默认上限为 `max_total_connections / max_open_targets`，目标 `max_connections` 可覆盖；API 与 Worker 各自计数。`tenant-data up/verify/status` 使用目标键，`--all` 只处理登记目标；多个目标先核对物理实例与 schema，别名重复时在 DDL 前拒绝，失败后先查状态，不自动重放已完成目标。
