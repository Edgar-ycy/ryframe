# 数据

## 全新环境初始化

先创建隔离的空 MySQL database，再设置 `APP_ENV` 和数据库连接。配置按 `config/app.toml`、`config/app.<环境>.toml`、`APP_*` 依次覆盖，`APP_CONFIG_DIR` 可指向另一配置目录。密钥文件、JWT 和初始管理员密码的准备见 [README](../README.md#下载后首次运行)。

默认单租户仍有控制库与 `shared-control` 两个独立账本，从后端根目录依次运行：

```powershell
cargo migrate -- control up
cargo migrate -- control verify
cargo migrate -- tenant-data up --target shared-control
cargo migrate -- tenant-data verify --target shared-control
cargo serve
```

`control up` 是控制库唯一的新建/升级入口；`control verify` 只校验账本、结构和编译期业务目录，新空库上不能代替初始化。迁移需要初始化管理员密码，API 与 Worker 启动时只校验，不执行 DDL。非生产默认 `jobs.mode="embedded"`，`cargo serve` 已消费后台任务；验证独立进程时在 API 和 Worker 两端设置 `APP_JOBS_MODE=external`，再分别运行 `cargo serve` 与 `cargo worker`。每个 MySQL 租户目标都要先创建空库、登记配置并单独执行 `tenant-data up/verify`。

## 控制库初始化 SQL

```powershell
cargo migrate -- control export-sql --output .local-tests/control-init.sql --plan
cargo migrate -- control export-sql --output .local-tests/control-init.sql --write
```

`--plan` 不读密码、配置或数据库，也不写文件。正式导出包含控制库结构、后置约束、系统数据、已注册控制库业务迁移和账本，不包含明文密码；手写业务迁移必须提供与 `up` 同源的 schema 和初始化 SQL。选择空库并使用客户端遇错停止，不使用 `--force`。PowerShell 使用：

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

租户表把 `control` 改为 `tenant-data`，并显式提供 `--target business-shared`；不接受 `--all`。`--output` 必须配合 `--write`，`--inspect` 禁止写入，计划只能使用 `.local-tests` 内的新路径。接管只登记现有结构，不执行历史 DDL、不更改业务行；部分登记、结构漂移或未知结果先 inspect 原计划，不能换计划重放。目标为空时按正常 `up` 建表。

授权、任务领取、导出申请、删除受理和配置迁移使用主库与显式事务。可接受陈旧的列表读才使用 eventual consistency；执行前校验、下载和状态转换使用 strong consistency。租户数据范围由 `sys_tenant.data_scope_version` 驱动：控制库事务写入 outbox，Worker 在 placement 目标发布并原子切换投影；认证主体版本落后或投影不可用时失败关闭。两个数据库之间没有分布式事务，已并发进入租户库的写入不能被控制库回滚。

## 备份、恢复与保留

正式备份前停止 API、Worker、调度和消息生产者。库存以 `--quiesced-at` 表明外部已静止；`--observed-at` 只记录观察起点，不能作为正式备份资格。备份登记验证文件大小和 SHA-256，恢复数据校验比较当前 schema、完整表摘要、placement、ownership 和对象清单，资源与登记元数据不参与业务摘要。隔离恢复必须使用新 scope、数据库和对象前缀，保留目标 ownership；通过数据验证、API/Worker 就绪和真实业务证明后才登记成功。参考目标是 24 小时恢复点、60 分钟 RTO，实际结果不能由计划或单次库存推断。下列输出父目录须已存在，目标文件必须不存在；详细恢复命令见[运维指南](operations.md#备份失败或过期)。

```powershell
cargo xtask data backup inventory --output .local-tests/backup/inventory.json --source-sha <40位SHA> --quiesced-at <RFC3339>
cargo xtask data target inventory --target <目标键> --output .local-tests/backup/target-inventory.json
```

开发复制先用 `cargo xtask check recovery clone plan --input <清单> --output <新计划> --write` 固定计划，再用 `cargo xtask check recovery clone verify --plan <计划>` 只读复核；运行目录、证据和输出必须位于当前后端 `.local-tests`，拒绝链接、越界、未知写入和未登记资源。中断后先用只读 `clone status` 和完整前后像确定下一动作；只能按原账本执行 `reconcile`、`resume` 或绑定创建身份的 `recover`，不能重放未知写入。API、Worker、Redis 和存储重启分别登记配置、凭据引用、端口、进程身份和数据目录。source 代次 start/stop 失败后先查 `source-generation-status`，普通恢复只回收同代已登记进程，不补签历史前像或数据零漂移。

导出任务固化规范筛选、授权指纹、`snapshot_at` 和 `upper_id`；指定 ID 仍受租户及数据权限约束。默认上限为 50 万行、每批 1000 行、1800 秒、512 MiB 和每租户两个运行中任务。删除先写 `delete_pending_at`，再幂等清理对象与记录；对象不存在视为成功，存储失败保留 tombstone 重试。每个部署使用稳定唯一的 `scope_id`，Redis、对象和数据库均校验 ownership；禁止 `FLUSHDB`、`KEYS`、模糊 bucket 删除和扫描共享服务器。

## 配置覆盖与秘密

API、Worker、迁移和 reset 从配置目录读取 `app.toml`，递归覆盖 `app.<环境>.toml`，最后应用受支持的环境变量；`cargo generate` 未设置 `APP_CONFIG_DIR` 时读取后端 Workspace 的 `config`。`APP_ENV` 只接受 `dev`、`test`、`prod`，建议每条命令显式设置并使用绝对配置目录。

支持 `_FILE` 的常用秘密包括 `APP_DATABASE_PASSWORD`、`APP_AUTH_JWT_SECRET`、`APP_REDIS_PASSWORD`、对象存储凭据和监控 token；直接变量与 `_FILE` 不能同时设置。`_FILE` 必须是 UTF-8，普通值为文件全文并最多移除一个行尾换行。数组在环境文件及 `APP_*` 覆盖中整体替换：`APP_CORS_ALLOW_ORIGINS` 和 `APP_PROXY_TRUSTED_CIDRS` 使用逗号分隔字符串；`database.replicas`、`database.sources` 和 `tenant_data.targets` 使用完整 JSON 数组，不能只改一项。仓库不自动读取 `.env`。

`tenant_data.targets[].password_env` 是环境变量名，运行时直接读取该变量，不是密码或 `_FILE` 路径；通用 `_FILE` 不会自动展开任意目标变量。需要在启动前显式注入：

```powershell
$env:RYFRAME_BIZ_PASSWORD = (Get-Content -Raw -LiteralPath "D:/secrets/ryframe-biz-password.txt").TrimEnd("`r", "`n")
```

## 租户路由

`database.primary` 是控制库唯一写库，也是强一致读取和 `shared-control` 的来源；`database.replicas` 是同一控制库的只读副本；`database.sources` 是业务显式选择的命名 MySQL 来源。三者都不替代租户 placement。以下是一个完整连接拓扑；密码占位值必须替换，证书路径按运行进程平台解析：

```toml
scope_id = "orders-production"

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
password_env = "RYFRAME_BIZ_SHARED_PASSWORD"
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
password_env = "RYFRAME_BIZ_ACME_PASSWORD"
max_connections = 6
tls_mode = "verify_identity"
tls_ca = "/run/secrets/mysql-ca.pem"
```

JSON 环境覆盖必须包含完整数组，例如 `APP_DATABASE_REPLICAS_FILE`：

```json
[{"name":"read-east","host":"mysql-read.internal","port":3306,"database":"ryframe_control","username":"reader","password":"replace-me","max_connections":8,"min_connections":1,"tls_mode":"verify_identity","tls_ca":"/run/secrets/mysql-ca.pem"}]
```

`APP_DATABASE_SOURCES_FILE` 使用同名连接字段，`APP_TENANT_DATA_TARGETS_FILE` 使用目标字段。replica/source 只接受显式 `password`；租户目标只接受 `password_env`，不接受明文密码或 DSN。连接名称必须唯一，source 不能使用 `primary`，租户目标必须与其他控制库、来源和目标使用不同 schema。远程生产连接和租户目标必须 `verify_identity` 并配置 CA；`verify_ca` 也要求 CA，mTLS 证书与私钥必须成对配置。

replica 只服务显式 `ReadConsistency::Eventual` 读取：健康副本轮询，全部不可用时回退 primary；`Strong`、写后读、授权、任务、导出和状态转换使用 primary。RyFrame 不建立 MySQL 主从复制或等待追平，延迟监控和故障切换由运维负责。`database.sources` 通过 `control_database.source("<名称>")` 获取并返回 `Option`；每个来源有独立连接池和事务，不自动分库，也没有跨库分布式事务。生成器的结构来源不自动成为运行目标。

| 布局 | placement 目标 | 边界 |
| --- | --- | --- |
| 共享控制库 | 隐式 `shared-control`，`kind="control"`、`mode="shared"` | 默认单租户和系统租户；复用 primary 池，租户业务仍有独立迁移账本 |
| 共享业务库 | `kind="mysql"`、`mode="shared"` | 多租户共享 `biz_*` 表，查询仍按 `tenant_id` 隔离 |
| 独立业务库 | `kind="mysql"`、`mode="dedicated"` | 同一时刻一个 active 租户，可使用独立账号、容量和备份 |

新配置默认 `multi_tenancy.enabled=false`；显式设为 `true` 或 `APP_MULTI_TENANCY_ENABLED=true` 才开放租户选择和管理。`tenant_data.targets[].key` 是迁移、健康检查、API 和 placement 的稳定身份，数据库名不能代替。生成器 `source="local"` 是简写：`database=control` 时读取 primary；存储范围为 tenant 时读取 `tenant_data.default_target`，因此该键必须是 MySQL 目标，隐式 `shared-control` 不能作为 local 结构来源，也可直接写 `primary` 或 `tenant_data.targets.<键>` 消除歧义。新租户开通仍显式提交 `data_target_key`，运行时只以控制库 placement 为准；缺 placement 或未知目标时失败关闭。

从单租户 `shared-control` 演进时，先在配置中增加共享或独立目标，创建空库并注入目标密码，运行 `cargo migrate -- tenant-data up --target <目标键>` 与 `verify`，在“平台管理 → 数据目标”完成实时探测。新租户开通时选择目标键；已有非系统租户改用租户数据放置/迁移流程，先预览和校验 placement generation、计划哈希与幂等键，再复制数据并切换 placement。修改 `default_target`、DNS 或复制 schema 都不会迁移既有数据；系统租户禁止迁移业务数据，独立目标被占用时先完成迁移或清理。

独立目标池按需建立，默认上限为 `max_total_connections / max_open_targets`，目标 `max_connections` 可覆盖；API 与 Worker 各自计数。`tenant-data up/verify/status` 使用目标键，`--all` 只处理登记目标；多个目标先核对物理实例与 schema，别名重复时在 DDL 前拒绝，失败后先查状态，不自动重放已完成目标。
