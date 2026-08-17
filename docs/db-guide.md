# 数据库开发指南

> 最后核对：2026-08-13

## 1. 技术和边界

- ORM：SeaORM 2.0 稳定版。
- 驱动：MySQL 8.4。
- 连接池：SeaORM/SQLx 异步连接池。
- 应用模型：每个进程建立一个主库连接池、零到多个命名只读副本连接池，以及零到多个显式命名业务数据源连接池。
- 租户模型：共享表通过 `tenant_id` 隔离。
- 结构演进：`ryframe-db-migration` 是基线和增量迁移的唯一可执行事实来源；`sql/` 只保留便于审查的 MySQL 快照。

当前拓扑明确区分自动读写路由和显式业务数据源：

- `primary` 是唯一写库，迁移、事务、命令和一致性敏感读取都使用它。
- `replicas` 是同结构的 MySQL 只读副本，普通列表和详情查询按配置顺序轮询。
- `sources` 是独立 MySQL 业务数据库，只能由具体用例按名称显式选择；本机测试数据源为 `ryframe_device`。
- 没有配置副本时，读取自动使用主库；已经配置的副本不会在连接失败时被静默忽略。
- 一次 Service 用例只选择一次连接，复合查询不会在执行中途切换副本。
- Handler、认证和监控 crate 不选择数据库连接。控制库路由只存在于 `ControlDatabaseCluster`，租户业务数据路由只存在于 `TenantDatabaseRouter` 和业务 Service。
- 主库迁移及系统表校验不作用于 `sources`；业务数据源自行管理结构演进。

复制、延迟、只读权限和故障切换由数据库基础设施负责。应用不会把普通查询失败悄悄改发主库，也不会把业务数据源当作副本承接系统查询。

## 2. 配置

主库使用 `[database.primary]`，副本使用一个或多个 `[[database.replicas]]`：

```toml
[database]
sql_log_level = "off" # off | slow | summary | full
sql_slow_threshold_ms = 200

[database.primary]
host = "127.0.0.1"
port = 3306
database = "ryframe_config"
username = "root"
password = ""
max_connections = 10
min_connections = 1
acquire_timeout_secs = 10
idle_timeout_secs = 600
max_lifetime_secs = 1800
connect_timeout_secs = 10

[[database.replicas]]
name = "replica-a"
host = "10.0.0.21"
port = 3306
database = "ryframe_config"
username = "ryframe_readonly"
password = ""
max_connections = 10
min_connections = 1

[[database.replicas]]
name = "replica-b"
host = "10.0.0.22"
port = 3306
database = "ryframe_config"
username = "ryframe_readonly"
password = ""
max_connections = 10
min_connections = 1
```

SQL 日志统一走应用的 text/JSON writer、文件滚动和非阻塞写入管线，不会直接写入
`stdout` 或 `stderr`。`off` 完全关闭 SQL 事件；`slow` 仅以 `WARN` 输出达到阈值的
语句；`summary` 输出全部摘要；`full` 才输出完整参数化 SQL。所有模式均不记录绑定参数
值，生产环境默认保持 `off`；只有短时排障才应临时启用 `summary` 或 `full`。慢 SQL
阈值由 `APP_DATABASE_SQL_SLOW_THRESHOLD_MS` 覆盖，日志模式由
`APP_DATABASE_SQL_LOG_LEVEL` 覆盖；生产显式启用 `summary` 或 `full` 时进程会输出一次
安全警告。记录会携带操作名、耗时、阈值、慢查询标记及可关联的请求、租户、用户或任务
上下文；OpenTelemetry 数据库 span 不包含原始 SQL 或绑定参数。

消息收件箱的索引必须先由慢 SQL 日志收集候选语句，再在脱敏后的代表性数据上执行
`EXPLAIN ANALYZE`。候选索引为 `(tenant_id, user_id, deleted_at, message_id DESC)`、
`(tenant_id, user_id, deleted_at, read_at, message_id DESC)` 和
`(tenant_id, user_id, deleted_at, acked_at, message_id DESC)`；只有执行耗时或扫描行数至少
改善 30% 时，才允许把对应索引加入迁移。没有这份执行计划证据时不得仅凭代码中的过滤条件
创建索引，也不得改变后台任务领取查询既有的优先级和创建时间排序语义。

2026-08-07 已在隔离的 MySQL 8.0.41 实例上完成上述验证。数据集包含 20 万条消息和 100 万条
收件记录，覆盖 2 个租户、500 个用户，每条消息 5 个收件人；目标用户样本中约 20% 已软删除、
45% 未读、25% 未确认。每条查询预热后采集 21 次，中位耗时和 `EXPLAIN ANALYZE` 结果如下：

| 查询 | 原索引中位耗时 | 新索引中位耗时 | 耗时改善 | 实际索引扫描行数 | 决策 |
|---|---:|---:|---:|---:|---|
| 普通收件箱 | 2.340 ms | 0.571 ms | 75.62% | 800 → 51 | 加入迁移 |
| 未读收件箱 | 1.521 ms | 0.574 ms | 62.25% | 360 → 51 | 加入迁移 |
| 未确认收件箱 | 0.706 ms | 0.616 ms | 12.71% | 85 → 51（改善 40%） | 加入迁移 |

三组候选均满足“耗时或扫描行数至少改善 30%”。增量迁移因此安装
`idx_message_recipient_visible`、`idx_message_recipient_unread` 和
`idx_message_recipient_unacked`，并移除已被实际查询完整替代的旧收件箱、确认索引；回滚时会先恢复
旧索引，再撤销新索引和 `deleted_at` 列。后台任务领取索引及排序语义未发生变化。
移除旧索引后的最终组合再次测得普通、未读、未确认查询中位耗时分别为 0.544 ms、
0.565 ms 和 0.517 ms，三条执行计划均只扫描 51 条收件索引记录。

每个副本名称必须非空且唯一。副本省略的超时字段使用与主库相同的默认值，但主机、端口、库名、账号和连接池仍应显式配置。

命名业务数据源使用 `[[database.sources]]`。名称不能为保留值 `primary`，也不能与副本重名：

```toml
[[database.sources]]
name = "ryframe_device"
host = "127.0.0.1"
port = 3306
database = "ryframe_device"
username = "root"
password = "123456"
max_connections = 5
min_connections = 1

[generator]
data_source = "ryframe_device"
```

`generator.data_source` 必须是 `primary` 或已经注册的业务数据源名称。不存在的名称会在配置校验时失败，不会静默回退主库。

数据库配置拒绝未知字段，连接 URL 固定按 MySQL 生成。配置只在进程启动时加载，修改文件或环境变量后必须重启；旧配置中的多余字段不会被静默忽略。

MySQL 连接默认使用 `tls_mode = "required"`，保证 `caching_sha2_password` 等认证方式通过加密通道完成，不启用无 TLS 的 RSA 密码回退。远程生产数据库仍必须使用 `verify_identity` 并配置 CA；只有明确使用兼容认证方式且位于受控本机环境时才能显式选择 `disabled`。

主库环境变量保持短名称：

```text
APP_DATABASE_HOST
APP_DATABASE_PORT
APP_DATABASE_NAME
APP_DATABASE_USERNAME
APP_DATABASE_PASSWORD_FILE
APP_DATABASE_SQL_LOG_LEVEL
APP_DATABASE_SQL_SLOW_THRESHOLD_MS
```

`APP_DATABASE_PASSWORD_FILE` 指向只包含主库密码的 UTF-8 secret 文件。全部副本通过 `APP_DATABASE_REPLICAS_FILE` 指向的 JSON 数组文件一次性覆盖，数组元素与 `[[database.replicas]]` 字段一致：

```json
[
  {
    "name": "replica-a",
    "host": "10.0.0.21",
    "port": 3306,
    "database": "ryframe_config",
    "username": "ryframe_readonly",
    "password": "secret",
    "max_connections": 10,
    "min_connections": 1
  }
]
```

业务数据源通过 `APP_DATABASE_SOURCES_FILE` 指向的 JSON 数组文件覆盖，代码生成器选择通过 `APP_GENERATOR_DATA_SOURCE` 覆盖：

```json
[
  {
    "name": "ryframe_device",
    "host": "127.0.0.1",
    "port": 3306,
    "database": "ryframe_device",
    "username": "root",
    "password": "secret",
    "max_connections": 5,
    "min_connections": 1
  }
]
```

生产密码、副本和业务数据源文件应由密钥管理或部署平台挂载，不得提交到 Git；没有副本或业务数据源时，对应文件写入 `[]`。

## 3. 目录所有权

| 目录 | 职责 |
| --- | --- |
| `crates/ryframe-db/src/entities/` | SeaORM 实体和关系 |
| `crates/ryframe-db/src/repositories/` | 查询、租户过滤、软删除和持久化 |
| `crates/ryframe-db/src/cluster.rs` | 主库/副本池、命名业务数据源、读轮询和拓扑健康状态 |
| `crates/ryframe-db/src/migration/` | 与数据库 crate 同属的数据规则辅助模块 |
| `crates/ryframe-db-migration/src/` | 启动时执行的增量迁移 |
| `crates/ryframe/src/boot/datasource.rs` | 连接主库和业务数据源；以受限超时持续连接、探测并校验副本结构 |
| `sql/` | 由迁移基线对齐的只读审查快照，不作为运行时输入 |
| `crates/ryframe-service/` | 事务边界、业务校验和 Entity 到 Output 的转换 |

Handler 不得导入 Entity、Repository 或 SeaORM。数据库实体也不得直接作为公共 API 响应。

## 4. 主要表

### 系统表

| 表名 | 说明 |
| --- | --- |
| `sys_user` | 用户与 `authorization_version` 授权版本 |
| `sys_role` | 角色和数据范围 |
| `sys_permission` | API/按钮权限码 |
| `sys_menu` | 前端菜单树和稳定 `route_key` |
| `sys_dept` | 部门树 |
| `sys_post` | 岗位 |
| `sys_config` | 系统参数；`portable` 明确标记是否允许进入租户配置包 |
| `sys_cache_namespace_version` | 租户业务缓存命名空间的数据库权威单调版本 |
| `sys_dict_type`、`sys_dict_data` | 字典类型和数据 |
| `sys_notice` | 通知公告 |
| `sys_tenant` | 租户状态、配额、`authorization_epoch` 授权规则版本与 `configuration_version` 配置版本 |
| `sys_file` | 上传文件元数据 |
| `sys_background_job` | 持久化后台任务、租约、重试、死信和计划来源 |
| `sys_outbox_event` | 事务性 Outbox 投递状态 |
| `sys_job_schedule`、`sys_job_schedule_execution` | Cron 计划与不可变执行历史 |
| `sys_data_retention_run` | 数据保留策略快照、删除计数和运行结果 |
| `sys_user_import_job` | 异步用户导入进度、游标、文件引用和终态 |
| `sys_user_import_row_result` | 用户导入跳过与失败行；成功行只累计计数 |
| `sys_tenant_config_bundle` | 生成或上传的配置包元数据、私有文件引用、摘要、状态与过期时间 |
| `sys_tenant_config_transfer` | 目标租户预览、应用、回滚状态、计划哈希、版本栅栏与快照引用 |
| `sys_tenant_config_transfer_item` | 按稳定业务键记录的预览项目、动作、结果和安全说明 |
| `sys_tenant_operation_lease` | 每租户唯一的配置包、套餐、Capability、数据迁移和 finalize 统一租约 |
| `sys_service_account` | 租户服务账号、部门归属、状态、授权版本和请求上限 |
| `sys_service_credential` | 服务账号 API Key 元数据、Secret MAC、Pepper 版本、到期/撤销和幂等指纹 |
| `sys_service_delegation` | 用户对服务账号的显式限时委托、Token MAC、版本、到期/撤销和幂等指纹 |
| `sys_service_access_audit` | Agent 成功、拒绝与错误访问的不可变安全审计 |
| `password_reset_requests` | 一次性密码重置请求 |

### 关联与日志表

| 表名 | 说明 |
| --- | --- |
| `sys_user_role` | 用户与角色 |
| `sys_role_permission` | 角色与权限 |
| `sys_role_dept` | 角色自定义部门范围 |
| `sys_service_account_role` | 服务账号与普通角色的租户内授权关系 |
| `sys_service_delegation_capability` | 委托允许调用的编译期 Agent 能力白名单 |
| `sys_oper_log` | 操作日志 |
| `sys_login_info` | 登录日志 |

关联表对真实父记录建立外键并按业务需要级联删除。软删除实体间的关系由 Service 校验，避免数据库级联绕过审计和业务规则。

数据保留、异步导入和租户趋势使用专门的复合索引。清理查询以终态时间和稳定 ID 分批，趋势查询以 `tenant_id + 时间 + 状态/结果` 聚合；新增或调整索引前必须在代表性数据上保存 `EXPLAIN` 证据。导入历史到期时，`sys_user_import_row_result` 通过外键级联删除；导入文件对象和 `sys_file` 元数据由 FileService 安全删除，不得用裸 SQL 绕过对象清理。

服务账号迁移一次性安装上述六张表，并为 `sys_dept`、`sys_user`、`sys_role` 补齐租户复合身份
约束，使所有关系外键同时绑定 `tenant_id` 和父记录 ID。`sys_service_account.code`、Key ID、委托
Token MAC、幂等哈希和审计 request ID 具有相应唯一约束；凭据和委托分别按账号/用户维护幂等唯一
键。迁移的 `down` 明确拒绝删除这些前向安全数据，生产回退不能通过自动降级表结构完成。

数据库只保存完整 API Key 和委托令牌的 HMAC-SHA-256、对应 `pepper_version`，不保存或可逆加密
明文。`idempotency_key_hash` 只用于定位重试，`request_fingerprint` 用于拒绝相同幂等键对应不同
请求；幂等重放只能返回已有元数据，无法重新取回一次性 Secret/Token。`sys_service_access_audit`
不对账号、凭据、委托和用户建立会因业务删除而丢失历史的级联外键；关联标识可以为空，以记录身份
解析前的拒绝、未注册 Agent 路径和基础设施错误。它只保存 IP、user-agent 的带 Pepper 摘要，不
保存原值、凭据、令牌或查询响应正文。

## 5. Entity 约定

实体位于 `ryframe-db`，只描述持久化结构：

```rust,ignore
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "sys_example")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: i64,
    pub tenant_id: String,
    pub name: String,
    pub status: String,
    pub del_flag: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}
```

规则：

- Snowflake ID 在应用侧生成，不能依赖数据库自增。
- 所有租户业务表必须包含 `tenant_id`。
- 需要软删除的业务实体通常使用统一 `del_flag` 常量；`sys_message_recipient` 例外，使用 `deleted_at` 保存当前收件人的独立删除时间，所有收件箱查询必须显式过滤该字段。
- 时间统一存储 UTC，展示时由前端做时区转换。
- Entity 不派生或承诺 API 所需的序列化形状；HTTP ID 必须在 Output 中转为字符串。

### Snowflake 运行时约束

- 业务代码必须调用 `try_next_snowflake_id()`，`AutoFill` 也返回 `AppResult<()>`；时钟回拨或单毫秒 4096 个序列耗尽时立即返回可重试的 503，不等待、不中断进程，也不生成逻辑未来时间。
- 当前进程内，成功生成的 ID 严格单调且唯一；同时运行的实例必须使用不同的 `SNOWFLAKE_WORKER_ID`。
- 生成器不持久化时间戳高水位。进程重启后若复用同一 worker ID，且物理时钟回到该 worker 已使用过的毫秒（或在同一毫秒内重启），仍存在重复风险。运维必须保持时钟同步，并在复用 worker ID 前确保物理时间超过最后生成时间；需要跨重启严格保证时，应引入外部持久化 worker 租约/高水位协调。

## 6. Repository 约定

Repository 只处理持久化，不承载 HTTP 或 UI 语义：

```rust,ignore
use ryframe_core::{Repository, ValidatedPageQuery};
use ryframe_db::ExampleRepository;

// API 层先将原始 Option 参数按运行时策略转换为已校验值对象。
let validated_page = ValidatedPageQuery::from_optional(
    raw_query.page,
    raw_query.page_size,
    &config.pagination,
)?;
let repo = ExampleRepository;
let record = repo.find_by_id(&db, &actor.tenant_id, id).await?;
let page = repo
    .find_by_page(&db, &actor.tenant_id, validated_page)
    .await?;
```

Service 直接保存具体 Repository，不再增加无行为的包装类型。仓储调用需要日志或指标时，应在真正的可观测边界统一实现，不在 Service 字段上套空壳。

新增 Repository 时必须保证：

1. 所有普通查询显式接收并应用 `tenant_id`，禁止从隐式上下文推断租户。
2. 软删除表默认过滤已删除记录。
3. 更新 ActiveModel 时重置变更状态，确保赋值真正生成 SQL `SET`。
4. 批量操作有明确上限，并在需要时使用事务。
5. 跨租户管理查询使用专用、命名清晰的方法，不能偷偷绕过过滤。
6. 通过租户过滤、事务边界和结构守卫保证隔离、更新持久化与删除语义。
7. Service 与 Repository 只接收 `ValidatedPageQuery`；不得反序列化、默认构造或用字段字面量绕过 API 层校验。

## 7. Service 和事务

事务属于业务用例：

```rust,ignore
use sea_orm::TransactionTrait;

let txn = self.db.write().begin().await?;
user_repo.insert(&txn, user).await?;
role_repo.replace_user_roles(&txn, user_id, &role_ids).await?;
txn.commit().await?;
```

不要手写“任一步失败再 rollback”的分支；`?` 返回时事务对象会回滚，只有所有步骤成功后显式 `commit`。

同一用例只在入口选择一次连接，后续 Repository 共用该连接。不要把 `DatabaseConnection` 添加到公开 Service 参数，也不要让 Handler 决定读写节点。

Service 应接收 Command/Query，而不是持续增加位置参数：

```rust,ignore
pub struct CreateExampleCommand {
    pub name: String,
    pub status: ExampleStatus,
}

pub async fn create(&self, command: CreateExampleCommand, actor: &ActorContext)
    -> AppResult<ExampleOutput>;
```

控制面命令和事务通过 `ControlDatabaseCluster::write()` 固定使用主库。控制面查询不得使用隐含默认策略的辅助入口，必须显式调用 `select_read(ReadConsistency)`：

- `Strong`：写后读、认证授权、数据权限、安全决策、配额和唯一性校验。
- `Eventual`：不参与安全或业务决策的普通只读列表、详情和导出；有健康副本时轮询副本，否则回退主库。

`select_read` 返回本次用例固定使用的 `SelectedDatabase`，同一用例应持有其中的连接完成全部查询，避免执行过程中切换节点。

## 8. 租户和数据范围

- 认证后的租户只能来自已验证 Token/主体，不能被 `X-Tenant-Id` 覆盖。
- Service 显式接收 `ActorContext`，Repository 显式接收 `tenant_id`。
- task-local 只校验 HTTP 请求内的显式租户是否与认证上下文一致，不是数据查询输入。
- 后台任务必须从任务载荷或受信配置获得显式租户，并将其传入 Service/Repository；不需要伪造 HTTP task-local。
- 用户、部门、公告和日志查询还会叠加角色数据范围。
- 多角色范围取可见数据并集；任一角色拥有全部数据范围时不附加行级限制。

Agent 访问在租户内有两种主体模型。直接模式只计算服务账号所绑定普通角色的权限并集与数据范围；
委托模式分别计算服务账号角色并集、用户角色并集，再对能力权限与行级范围取交集。服务账号不能
绑定超级角色，委托能力只能来自编译期注册表且必须为双方当前共同权限。用户/部门查询把交集直接
下推为租户过滤后的 `All`、部门集合、用户本人或部门加本人条件；岗位/字典目前要求两个主体都拥有
全部数据范围。禁止先读取无界全租户行再在内存中过滤。

服务账号相关写入统一先对 `sys_tenant` 取得更新锁，再按稳定顺序锁定服务账号、凭据或委托、用户
及关系行。账号状态/角色、API Key 创建/撤销、委托创建/撤销都在同一事务提升服务账号
`authorization_version` 和租户 `authorization_epoch`；委托还提升被代表用户的授权版本。Agent
查询按“租户共享锁 → 服务账号共享锁 → 凭据共享锁 → 可选委托及能力共享锁 → 授权快照 → 只读
查询 → 成功审计”执行，只有事务提交后才能返回成功。失败审计使用独立事务；审计写入失败时请求
必须失败，不能返回未经持久化审计的成功结果。

跨租户平台用例必须使用专用方法并校验系统租户，不能通过省略租户参数绕过隔离。租户创建、修改和
启停等写用例继续要求系统超级管理员；只读分页、详情和用量查询再分别校验 `tenant:list` 与
`tenant:usage:list`。参见 [架构指南](architecture.md)。

### 租户容量聚合

平台租户容量分页、详情和用量查询都选择主库 `Strong` 读取，不能使用副本，也不能通过扫描对象
存储推算文件占用。统计口径必须与实际配额检查一致：

- 用户：`sys_user.del_flag = '0'`，包含正常、待激活、必须改密等仍占席位的状态。
- 角色：`sys_role.del_flag = '0'`。
- 存储：`sys_file.del_flag = '0'`，按数据库中的非负 `file_size` 求和。
- 后台任务：分别汇总 `pending`、`running`、`dead`。
- 计划：只统计未软删除且已启用的 `sys_job_schedule`。
- 用户导入：只统计 `pending`、`running`。

分页先在 `sys_tenant` 上确定当前页，再把该页全部租户 ID 交给一条条件聚合查询；用户、角色、文件、
后台任务、计划和导入均按这组 ID 批量汇总，查询次数不得随页内租户数增长。容量状态筛选本身也在
数据库中按三类资源聚合结果完成，不能先取无界租户列表再在内存中筛选。请求限流窗口来自 Redis
只读快照，不参与主库整体容量状态，也不能把读取快照实现成会增加计数的限流请求。

租户资源配额为 `0` 时统一表示无限制。非零配额的状态按整数交叉相乘比较 80%、90%、100% 边界，
避免浮点误差；整体状态取用户、角色、存储三项中最严重的一项，三项全部无限制时为 `unlimited`。
Redis 当前窗口不可用时只返回 `unknown`，不得回滚或隐藏已经成功读取的主库统计。

租户容量治理迁移安装以下四个聚合索引：

```text
sys_user         idx_user_tenant_del              (tenant_id, del_flag)
sys_role         idx_role_tenant_del              (tenant_id, del_flag, id)
sys_file         idx_file_tenant_del_size         (tenant_id, del_flag, file_size)
sys_job_schedule idx_schedule_tenant_del_enabled  (tenant_id, enabled, del_flag)
```

上线前应使用预计租户数、用户数、角色数和文件数的代表性数据执行 `EXPLAIN ANALYZE`，保存容量筛选、
当前页条件聚合和单租户用量查询的执行计划，确认命中上述索引且没有逐租户子查询。索引只服务查询，
不会改变配额、软删除或文件生命周期语义。

### 租户配置版本与迁移租约

部门、岗位、字典、可迁移参数、权限、菜单、角色及其关系发生创建、更新、启停、软删除或关系变化时，
必须在同一事务递增 `sys_tenant.configuration_version`。涉及授权的变化还必须递增
`authorization_epoch`；二者分别解决“预览后配置已改变”和“授权结果已改变”，不能互相替代。
`sys_config.portable` 默认 `false`，只有显式标记且不命中敏感键规则的参数才能导出；不得把密码、
Secret、Token、Credential 或 Private Key 类键值写入配置包。

所有配置写事务统一遵循以下锁顺序：

```text
sys_tenant 行
  → sys_tenant_operation_lease 行
  → 父资源或目标资源行
  → 关系行
```

普通写入在锁定租户行后检查统一操作租约，发现其他所有者的配置包应用、套餐/Capability 变更、
数据迁移或 finalize 返回 `409 tenant_operation_conflict`。
应用或回滚使用自己的 `owner_token`，在最终事务内重新核对租约所有权、到期时间、
`configuration_version`、`authorization_epoch` 和计划哈希，再按稳定业务键顺序锁定并写入资源。
禁止先在事务外读取旧 Model，再在取得租户栅栏后直接覆盖；更新、删除、父级校验、引用检查和部门
`ancestors` 计算都必须在取得栅栏后基于重新锁定的当前记录完成。

配置包和迁移记录只保存文件 ID 与 SHA-256，不保存对象内容或对象路径。应用前快照上传失败时不进入
写事务；应用事务失败时全部回滚。配置包对象和回滚快照默认分别受 168 小时 artifact/rollback
窗口保护；当前没有配置包或迁移历史元数据的自动硬删除期限，不得把文件过期误写成元数据删除策略。

## 9. 迁移与重置

生产 API 和 Worker 不执行 DDL。控制库与租户业务数据目标分别使用 `seaql_migrations` 和
`seaql_tenant_data_migrations`，运维先运行 `ryframe-migrate control up|verify|status`，再运行
`ryframe-migrate tenant-data up|verify|status --all`（或 `--target <key>`）。`shared-control` 虽与
控制库使用同一 MySQL Schema，仍分别执行两本账。副本以不可路由槽位注册：监督器每 5 秒以
2 秒总超时执行连接/PING/结构校验，连续两次完整成功后才接收最终一致性读取；连续三次网络失败
会摘除，结构不一致会立即摘除，并按 5、10、20、40、60 秒上限退避重连。命名数据源不参与
租户路由。新增结构变更时：

`sys_background_job` 与 `sys_outbox_event` 同时保存可空的 `traceparent` 和 `tracestate`；两列共同构成跨进程 W3C Trace Context，迁移、实体和规范结构指纹必须同步演进。

1. 新增迁移文件并注册到迁移器。
2. 同步 Entity 和 Repository。
3. 重新生成并校验 `sql/` 审查快照。
4. 在受控环境演练空库、已有库和旧结构升级路径，并保留运维证据。
5. 在 CHANGELOG 记录不可逆或需要运维关注的变更。

开发环境需要清空并重建时运行：

```bash
APP_ENV=dev cargo run -p ryframe --bin ryframe-db-reset -- \
  --database ryframe_config \
  --confirm-reset RESET-RYFRAME-DATABASE
```

PowerShell：

```powershell
$env:APP_ENV = "dev"
cargo run -p ryframe --bin ryframe-db-reset -- `
  --database ryframe_config `
  --confirm-reset RESET-RYFRAME-DATABASE
```

该命令要求配置库名与 `--database` 完全一致，且在 `prod`/`production` 环境永久拒绝执行。确认后，工具使用配置中的数据库账号连接同一 MySQL 实例的 `mysql` 管理库，执行 `DROP DATABASE IF EXISTS` 和 `CREATE DATABASE`，再运行 Migrator 与 Seeder；因此旧表和全部现有数据都会被永久删除，执行账号必须能连接管理库，并拥有目标库的 `DROP`、`CREATE` 权限。生产应用账号不应授予这些权限。

## 10. 连接池

| 参数 | 默认值 | 说明 |
| --- | ---: | --- |
| `max_connections` | 10 | 单个节点连接池的最大连接数 |
| `min_connections` | 1 | 单个节点最小保留连接数 |
| `acquire_timeout_secs` | 10 | 获取连接超时 |
| `idle_timeout_secs` | 600 | 空闲连接回收时间 |
| `max_lifetime_secs` | 1800 | 单连接最大生命周期 |
| `connect_timeout_secs` | 10 | 建连超时 |

`GET /api/v1/monitor/db-pool` 展示控制库池统计；`GET /api/v1/monitor/runtime` 展示控制库主库、
副本和命名数据源的状态。租户数据目标按需延迟建池，并受 `[tenant_data]` 的
`max_open_targets`、`max_total_connections` 和 `idle_pool_secs` 全局约束；空闲池按 LRU 回收，
同一目标创建采用 single-flight。单目标故障不拖垮全局 readiness，平台通过数据目标详情 API
查询低基数健康信息。数据库总预算还要预留迁移任务和管理连接；活跃连接长期接近上限时，应先
检查慢查询、长事务和并发模型，不要只按 CPU 公式扩大连接池。

## 11. 提交前检查

数据库相关改动至少运行：

```powershell
cargo fmt --all -- --check
cargo check --workspace --lib --bins
cargo clippy --workspace --lib --bins -- -D warnings
```
