# 数据库开发指南

> 最后核对：2026-07-18

## 1. 技术和边界

- ORM：SeaORM 2.0 RC。
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
- Handler、认证和监控 crate 不选择数据库连接，路由策略只存在于 `DatabaseCluster` 和 Service。
- 主库迁移及系统表校验不作用于 `sources`；业务数据源自行管理结构演进。

复制、延迟、只读权限和故障切换由数据库基础设施负责。应用不会把普通查询失败悄悄改发主库，也不会把业务数据源当作副本承接系统查询。

## 2. 配置

主库使用 `[database.primary]`，副本使用一个或多个 `[[database.replicas]]`：

```toml
[database]
sql_log_level = "off" # off | summary | full

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

主库环境变量保持短名称：

```text
APP_DATABASE_HOST
APP_DATABASE_PORT
APP_DATABASE_NAME
APP_DATABASE_USERNAME
APP_DATABASE_PASSWORD
```

全部副本通过 `APP_DATABASE_REPLICAS` JSON 数组一次性覆盖，数组元素与 `[[database.replicas]]` 字段一致：

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

业务数据源通过 `APP_DATABASE_SOURCES` JSON 数组覆盖，代码生成器选择通过 `APP_GENERATOR_DATA_SOURCE` 覆盖：

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

生产密码应由密钥管理或部署环境注入，不得提交到 Git。

## 3. 目录所有权

| 目录 | 职责 |
| --- | --- |
| `crates/ryframe-db/src/entities/` | SeaORM 实体和关系 |
| `crates/ryframe-db/src/repositories/` | 查询、租户过滤、软删除和持久化 |
| `crates/ryframe-db/src/cluster.rs` | 主库/副本池、命名业务数据源、读轮询和拓扑健康状态 |
| `crates/ryframe-db/src/migration/` | 与数据库 crate 同属的数据规则辅助模块 |
| `crates/ryframe-db-migration/src/` | 启动时执行的增量迁移 |
| `crates/ryframe/src/boot/datasource.rs` | 连接全部节点，并在主库迁移后只校验主库/副本结构 |
| `sql/` | 由迁移基线对齐的只读审查快照，不作为运行时输入 |
| `crates/ryframe-service/` | 事务边界、业务校验和 Entity 到 Output 的转换 |

Handler 不得导入 Entity、Repository 或 SeaORM。数据库实体也不得直接作为公共 API 响应。

## 4. 主要表

### 系统表

| 表名 | 说明 |
| --- | --- |
| `sys_user` | 用户和会话版本 |
| `sys_role` | 角色和数据范围 |
| `sys_permission` | API/按钮权限码 |
| `sys_menu` | 前端菜单树和稳定 `route_key` |
| `sys_dept` | 部门树 |
| `sys_post` | 岗位 |
| `sys_config` | 系统参数 |
| `sys_dict_type`、`sys_dict_data` | 字典类型和数据 |
| `sys_notice` | 通知公告 |
| `sys_tenant` | 租户状态和配额 |
| `sys_file` | 上传文件元数据 |
| `password_reset_requests` | 一次性密码重置请求 |

### 关联与日志表

| 表名 | 说明 |
| --- | --- |
| `sys_user_role` | 用户与角色 |
| `sys_role_permission` | 角色与权限 |
| `sys_role_dept` | 角色自定义部门范围 |
| `sys_oper_log` | 操作日志 |
| `sys_login_info` | 登录日志 |

关联表对真实父记录建立外键并按业务需要级联删除。软删除实体间的关系由 Service 校验，避免数据库级联绕过审计和业务规则。

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
- 需要软删除的表使用统一 `del_flag` 常量。
- 时间统一存储 UTC，展示时由前端做时区转换。
- Entity 不派生或承诺 API 所需的序列化形状；HTTP ID 必须在 Output 中转为字符串。

## 6. Repository 约定

Repository 只处理持久化，不承载 HTTP 或 UI 语义：

```rust,ignore
use ryframe_core::{LoggedRepo, PageQuery, Repository};
use ryframe_db::ExampleRepository;

let repo = LoggedRepo::new(ExampleRepository);
let record = repo.find_by_id(&db, &actor.tenant_id, id).await?;
let page = repo
    .find_by_page(&db, &actor.tenant_id, PageQuery::new(1, 20))
    .await?;
```

新增 Repository 时必须保证：

1. 所有普通查询显式接收并应用 `tenant_id`，禁止从隐式上下文推断租户。
2. 软删除表默认过滤已删除记录。
3. 更新 ActiveModel 时重置变更状态，确保赋值真正生成 SQL `SET`。
4. 批量操作有明确上限，并在需要时使用事务。
5. 跨租户管理查询使用专用、命名清晰的方法，不能偷偷绕过过滤。
6. Repository 测试至少覆盖租户隔离、更新持久化和删除行为。

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

普通只读用例在入口调用一次 `let db = self.db.read();`，后续 Repository 共用该引用。命令、事务、权限/会话校验、唯一性校验和写后立即读取使用 `self.db.write()`。不要把 `DatabaseConnection` 添加到公开 Service 参数，也不要让 Handler 决定读写节点。

Service 应接收 Command/Query，而不是持续增加位置参数：

```rust,ignore
pub struct CreateExampleCommand {
    pub name: String,
    pub status: ExampleStatus,
}

pub async fn create(&self, command: CreateExampleCommand, actor: &ActorContext)
    -> AppResult<ExampleOutput>;
```

## 8. 租户和数据范围

- 认证后的租户只能来自已验证 Token/主体，不能被 `X-Tenant-Id` 覆盖。
- Service 显式接收 `ActorContext`，Repository 显式接收 `tenant_id`。
- task-local 只校验 HTTP 请求内的显式租户是否与认证上下文一致，不是数据查询输入。
- 后台任务必须从任务载荷或受信配置获得显式租户，并将其传入 Service/Repository；不需要伪造 HTTP task-local。
- 用户、部门、公告和日志查询还会叠加角色数据范围。
- 多角色范围取可见数据并集；任一角色拥有全部数据范围时不附加行级限制。

跨租户平台用例必须使用专用方法并校验系统超级管理员，不能通过省略租户参数绕过隔离。参见 [架构指南](architecture.md)。

## 9. 迁移与重置

应用启动时只在主库自动运行 `ryframe-db-migration`，完成后校验主库和全部副本的业务表。命名业务数据源只执行连接和健康检查，不执行系统迁移或系统表校验。复制延迟或外部迁移系统必须保证副本结构在应用接流量前就绪。新增结构变更时：

1. 新增迁移文件并注册到迁移器。
2. 同步 Entity 和 Repository。
3. 重新生成并校验 `sql/` 审查快照。
4. 添加空库、已有库和旧结构升级的 MySQL 迁移测试。
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

`GET /api/v1/monitor/db-pool` 当前展示主库池统计；`GET /api/v1/monitor/runtime` 分别展示主库、副本和业务数据源的连接状态及读取策略。数据库总连接预算为每个应用实例的主库池、全部副本池和全部业务数据源池之和，还要预留迁移任务和管理连接。活跃连接长期接近上限时，应先检查慢查询、长事务和并发模型，不要只按 CPU 公式扩大连接池。

## 11. 提交前检查

数据库相关改动至少运行：

```bash
docker compose -f docker-compose.test.yml up -d --wait
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p ryframe-db
cargo test -p ryframe-db-migration
cargo test -p ryframe-service
docker compose -f docker-compose.test.yml down
```

本机 MySQL 已提供 `ryframe_device` 时，可显式运行外部数据源验证：

```bash
cargo test -p ryframe-db --test named_datasource_mysql_test mysql_named_source_is_distinct_and_explicit -- --ignored --exact
```

涉及 API 输出时还必须运行 `cargo test -p ryframe-api`，并确认 OpenAPI 与前端字符串 ID 契约同步。
