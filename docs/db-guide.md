# 数据库指南

## 技术栈

- **ORM**: SeaORM 2.0-rc
- **支持的数据库**: MySQL 8.0+, PostgreSQL 14+, SQLite 3.35+
- **迁移工具**: sea-orm-migration
- **连接池**: SQLx 连接池（tokio 异步）
- **驱动**: sqlx-mysql / sqlx-postgres / sqlx-sqlite

## 数据库配置

在 `config/app.toml` 中配置：

```toml
[database]
sql_log_level = "off"  # off | summary | full

[[database.connections]]
driver = "postgres"      # postgres | mysql | sqlite
host = "localhost"
port = 5432
database = "ryframe"
username = "postgres"
password = ""
max_connections = 10
min_connections = 1
```

### 字符集

MySQL 使用 `utf8mb4_general_ci`（`docker-compose.yml` 中已配置）：

```yaml
mysql:
  command: >
    --character-set-server=utf8mb4
    --collation-server=utf8mb4_general_ci
```

## 数据表

### 核心业务表 (system)

| 表名 | 实体文件 | 说明 |
|------|----------|------|
| `sys_user` | `entities/user.rs` | 用户信息 |
| `sys_role` | `entities/role.rs` | 角色 |
| `sys_permission` | `entities/permission.rs` | 权限（菜单+按钮+API） |
| `sys_menu` | `entities/menu.rs` | 菜单（树形） |
| `sys_dept` | `entities/dept.rs` | 部门（树形） |
| `sys_post` | `entities/post.rs` | 岗位 |
| `sys_config` | `entities/config.rs` | 参数配置 |
| `sys_dict_type` | `entities/dict_type.rs` | 字典类型 |
| `sys_dict_data` | `entities/dict_data.rs` | 字典数据 |
| `sys_notice` | `entities/notice.rs` | 通知公告 |

### 关联表

| 表名 | 实体文件 | 说明 |
|------|----------|------|
| `user_role` | `entities/user_role.rs` | 用户-角色关联 |
| `role_permission` | `entities/role_permission.rs` | 角色-权限关联 |
| `role_menu` | `entities/role_menu.rs` | 角色-菜单关联 |
| `role_dept` | `entities/role_dept.rs` | 角色-部门数据权限关联 |

### 日志表 (monitor)

| 表名 | 实体文件 | 说明 |
|------|----------|------|
| `sys_oper_log` | `entities/oper_log.rs` | 操作日志 |
| `sys_login_info` | `entities/login_info.rs` | 登录日志 |

### 任务表

| 表名 | 实体文件 | 说明 |
|------|----------|------|
| `sys_job` | `entities/job.rs` | 定时任务 |
| `sys_job_log` | `entities/job_log.rs` | 任务执行日志 |

## 实体定义

所有实体使用 SeaORM 派生宏，位于 `crates/ryframe-db/src/entities/`：

```rust
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sys_user")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    pub username: String,
    pub password_hash: String,
    pub nickname: String,
    pub email: String,
    pub phone: String,
    pub avatar: String,
    pub status: String,       // "0"=正常, "1"=禁用
    pub del_flag: String,     // "0"=正常, "2"=已删除（软删除）
    pub dept_id: Option<i64>,
    pub remark: Option<String>,
    pub login_ip: Option<String>,
    pub login_date: Option<chrono::NaiveDateTime>,
    pub created_at: chrono::NaiveDateTime,
    pub updated_at: chrono::NaiveDateTime,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::user_role::Entity")]
    UserRole,
    #[sea_orm(belongs_to = "super::dept::Entity", from = "Column::DeptId", to = "super::dept::Column::Id")]
    Dept,
}

impl ActiveModelBehavior for ActiveModel {}
```

## Repository 层

所有仓库实现在 `crates/ryframe-db/src/repositories/`（14 个文件）：

```rust
use ryframe_core::{Repository, LoggedRepo, PageQuery, PageResult};

// 基础仓库
let repo = UserRepository;
let user = repo.find_by_id(&db, 1).await?;

// 带操作日志的仓库（自动记录 DataDiff）
let logged_repo = LoggedRepo::new(UserRepository);
logged_repo.update_by_id(&db, 1, user.active_model).await?;
```

## 分页查询

```rust
use ryframe_db::pagination::{PageQuery, PageResult, paginate};

let page = PageQuery {
    page: 1,
    page_size: 10,
    sort_field: Some("id".into()),
    sort_order: Some("desc".into()),
};

let query = user::Entity::find()
    .filter(user::Column::Status.eq("0"));

let result: PageResult<user::Model> = paginate(query, page, &db).await?;
```

## 事务

```rust
use sea_orm::TransactionTrait;

let txn = db.begin().await?;

let result = user_repo.insert(&txn, user_model).await;
let result2 = role_repo.assign_role(&txn, user_id, role_id).await;

if result.is_ok() && result2.is_ok() {
    txn.commit().await?;
} else {
    txn.rollback().await?;
}
```

## 数据库迁移

`crates/ryframe-db/src/migration/` 包含迁移管理器。

### 命名规范

```
mYYYYMMDD_HHMMSS_<description>.rs
```

示例：`m20260101_000001_init_tables.rs`

### 命令

```bash
# 执行迁移
cargo run --bin ryframe-migration -- up

# 回滚
cargo run --bin ryframe-migration -- down

# 查看状态
cargo run --bin ryframe-migration -- status

# 重建数据库
cargo run --bin ryframe-migration -- fresh

# 生成新迁移文件
sea-orm-cli migrate generate <migration_name>
```

### 编程式迁移管理

```rust
use ryframe_db::migration::MigrationManager;

let up_to_date = MigrationManager::is_up_to_date(&db).await?;
let pending = MigrationManager::pending_count(&db).await?;
let applied = MigrationManager::status(&db).await?;
```

## 软删除

所有业务表使用 `del_flag` 字段实现软删除：

- `"0"` = 正常
- `"2"` = 已删除

查询时自动过滤已删除数据（Repository 层默认添加 `.filter(Column::DelFlag.eq("0"))`）。

## 数据权限

基于 RBAC 的数据权限隔离：

- **全部数据权限**：不过滤
- **自定义数据权限**：通过 `role_dept` 关联表控制可见部门
- **本部门数据权限**：仅查询当前用户所在部门
- **本人数据权限**：仅查询本人创建的数据

实现方式：Repository 层通过 `DataScope` 注解注入过滤条件。

## 多数据源

通过 `DataSourceManager` 支持读写分离和多数据源动态路由：

```rust
use ryframe_core::datasource::{DataSourceManager, current_db, get_db};

// 获取当前上下文的数据源
let db = current_db();           // 默认（主库）
let read_db = get_db("replica"); // 指定从库
```

## 连接管理

- 支持多数据源（`DataSourceManager`）
- 连接池自动管理（基于 SQLx），无需手动释放
- SQL 日志通过 `sql_log_flag` 控制（off / summary / full）
- 慢查询告警通过 `slow_query_threshold_ms` 配置（默认 0 不启用）
- 环境变量覆盖：`APP_DATABASE_PRIMARY_HOST` 等

## 连接池调优

### 核心参数说明

| 参数 | 默认值 | 说明 | 建议值 |
|------|--------|------|--------|
| `max_connections` | 10 | 最大连接数 | `(CPU核心数 * 2) + 磁盘数`，通常 10~50 |
| `min_connections` | 1 | 最小空闲连接 | 1~4，防止冷启动延迟 |
| `acquire_timeout_secs` | 10 | 获取连接超时(秒) | 5~30，取决于流量峰值 |
| `idle_timeout_secs` | 600 | 空闲连接存活(秒) | 300~600，小于 MySQL `wait_timeout` |
| `max_lifetime_secs` | 1800 | 连接最大生命(秒) | 1800~3600，定期轮换防泄漏 |
| `connect_timeout_secs` | 10 | TCP 建连超时(秒) | 3~10，网络不稳定时可提高 |

### 配置示例

**开发环境** (`config/app.dev.toml`):

```toml
[[database.connections]]
driver = "mysql"
host = "localhost"
port = 3306
database = "ryframe_config"
username = "root"
password = "123456"
max_connections = 5
min_connections = 1
acquire_timeout_secs = 10
idle_timeout_secs = 300
max_lifetime_secs = 1800
connect_timeout_secs = 5
```

**生产环境** (`config/app.prod.toml`):

```toml
[[database.connections]]
driver = "mysql"
host = "prod-db.internal"
port = 3306
database = "ryframe_config"
username = "app_user"
password = "${DB_PASSWORD}"
max_connections = 30
min_connections = 4
acquire_timeout_secs = 15
idle_timeout_secs = 600
max_lifetime_secs = 3600
connect_timeout_secs = 10

# 读写分离：添加只读副本
[[database.connections]]
driver = "mysql"
host = "prod-db-replica.internal"
port = 3306
database = "ryframe_config"
username = "readonly_user"
password = "${DB_REPLICA_PASSWORD}"
max_connections = 20
min_connections = 2
```

### 调优策略

1. **初始值设定规则**：
   - `max_connections` = `(CPU 核心数 × 2) + 有效磁盘数`
   - 例如 4 核 CPU + SSD → max_connections = 10~12
2. **监控驱动调优**：
   - 通过 `/api/v1/monitor/db-pool` 端点查看当前活跃/空闲连接数
   - 活跃连接持续接近 `max_connections` → 增加上限
   - 空闲连接长期为 0 → 增加 `min_connections`
3. **慢查询监控**：
   - 设置 `database.slow_query_threshold_ms = 100`
   - 超过 100ms 的查询会通过 stderr 输出 WARN
   - 生产环境建议阈值 200~500ms
4. **常见问题**：
   - **连接池耗尽**：增加 `max_connections`，检查是否有未释放的连接
   - **获取超时**：增加 `acquire_timeout_secs` 或减少 `max_connections`（数据库端连接数不足）
   - **空闲断开**：确保 `max_lifetime_secs` < MySQL `wait_timeout`

## 慢查询日志

通过 `database.slow_query_threshold_ms` 配置慢查询告警：

```toml
[database]
sql_log_level = "summary"
slow_query_threshold_ms = 100  # 超过 100ms 的查询输出 WARN
```

慢查询输出格式：
```
[SLOW QUERY WARN]  SELECT * FROM sys_user WHERE ...  [耗时: 235.42ms > 阈值: 100ms]
```

生产环境建议配合日志采集系统（如 ELK/Loki）收集分析慢查询。

## 最佳实践

1. **索引优化**: 为常用查询字段（username、status、dept_id、parent_id）建索引
2. **软删除**: 始终使用 `del_flag` 字段标记删除，非物理删除
3. **密码安全**: 使用 Argon2 哈希存储密码，不可逆
4. **时间字段**: 使用 `chrono::NaiveDateTime`，UTC 时间
5. **批量操作**: 批量删除/更新使用事务包裹
6. **SQL 注入防护**: SeaORM 参数化查询，无 SQL 注入风险
7. **连接池调优**: 生产环境建议 max_connections=20-50，根据数据库规格调整
8. **数据迁移**: 所有表结构变更通过 sea-orm-migration 管理，确保可复现
