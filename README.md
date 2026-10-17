# RyFrame

[English](README.en.md)

RyFrame 是面向企业后台的 Rust 2024 框架，与 Vue 3 管理端配套使用。当前为 `0.x` 开发版本，请使用当前契约和全新隔离数据库，不能直接覆盖旧开发库。

提供用户、角色、权限、组织、产品套餐、多租户、配置迁移、跨库迁移、监控、消息、调度、导入导出，以及个人资料、密码、头像和登录会话管理。套餐可发布空能力版本；通知公告和租户主动消息发布可按套餐启停，停用不删除历史数据，也不关闭系统通知、调度、导入导出或备份恢复。

业务运行时使用 Rust。API、Worker、迁移、reset 和生成器各有直接入口；`cargo xtask` 用于维护框架本身。

## 下载后首次运行

后端和前端是独立仓库，以下命令在后端仓库根目录执行。本地示例使用 Windows PowerShell；MySQL、Redis、RustFS 可以在满足版本和协议要求的任意平台或远程服务器运行，不要求 WSL。

1. 安装 rustup，在仓库内运行 `rustup show`，使用 `rust-toolchain.toml` 固定的正式 Rust、rustfmt 和 Clippy。
2. 准备 MySQL 8.0.16 或更高版本（不以 MariaDB 替代）、Redis，以及数据库用户的建表、迁移和业务读写权限。文件存储默认使用本地目录，也支持 RustFS、MinIO 和 S3。
3. 使用数据库客户端创建空库，例如：

```sql
CREATE DATABASE ryframe_dev CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci;
```

4. 在忽略目录 `.local-tests/secrets/` 创建 UTF-8 密钥文件：`mysql-password.txt` 保存数据库密码，`admin-password.txt` 保存管理员初始密码，`jwt-secret.txt` 保存至少 32 字节的随机 JWT 密钥。管理员密码为 8～72 个可见 ASCII 字符，包含大小写字母、数字和符号。
5. 配置并初始化。按实际服务修改地址、端口和用户；下列命令假定 Redis 可在默认端口访问，如需认证另设 `APP_REDIS_PASSWORD_FILE`：

```powershell
$env:APP_ENV = "dev"
$env:APP_SCOPE_ID = "my-development"
$env:APP_DATABASE_HOST = "127.0.0.1"
$env:APP_DATABASE_NAME = "ryframe_dev"
$env:APP_DATABASE_USERNAME = "ryframe_app"
$env:APP_DATABASE_PASSWORD_FILE = ".local-tests/secrets/mysql-password.txt"
$env:APP_AUTH_JWT_SECRET_FILE = ".local-tests/secrets/jwt-secret.txt"
$env:RYFRAME_INIT_ADMIN_PASSWORD_FILE = ".local-tests/secrets/admin-password.txt"
cargo migrate -- control up
cargo migrate -- control verify
cargo serve
```

`up` 初始化或升级明确选择的控制库，`verify` 只校验。先初始化再启动，API/Worker 不自动升级数据库。API 默认端口为 8080；需要后台任务时，在继承相同配置的另一终端运行 `cargo worker`。前端单独安装和启动，见前端 README。

普通初始用户 `user`、新建及批量导入的普通用户初始密码为 `123456`；管理员使用显式配置的密码。所有新账号首次登录必须改密，改密前不能操作业务，改密成功后重新登录。重复迁移不会重置已有密码或改密状态。已有开发版普通账号密码未知时，按[显式账号恢复流程](docs/api.md#异常账号恢复)生成一次性链接，由用户自行设置密码。

新配置默认单租户，身份固定使用系统租户，登录页不选租户。需要多租户时设置 `multi_tenancy.enabled = true` 或 `APP_MULTI_TENANCY_ENABLED=true`。已有部署的显式选择继续生效；单租户仍可使用配置迁移和数据维护。

## 数据库、配置与 SQL 导入

配置按基础文件、环境文件、环境变量覆盖；运行环境由 `APP_ENV` 选择，秘密支持对应 `_FILE`。多库的 primary、replicas、sources，租户共享控制库、共享业务库、独立库，TLS、连接池和数组替换规则见[数据指南](docs/data.md#租户路由)。

默认推荐迁移初始化。需要数据库工具直接导入时，生成完整 SQL：

```powershell
cargo migrate -- control export-sql --output .local-tests/control-init.sql --write
```

选择空库，按[数据指南](docs/data.md#控制库初始化-sql)遇错即停导入，再执行 `cargo migrate -- control verify`。正式导出包含密码哈希、后置约束、种子和迁移账本；`sql/ryframe_config.sql` 仅供源码审阅，不能代替正式文件。导入中断不自动重放或清库。

租户业务库先创建空库并登记目标，再显式执行 `cargo migrate -- tenant-data up --target <目标键>` 和 `verify`。非生产重建使用 `cargo reset -- <参数>`，步骤见[运维指南](docs/operations.md)。

## 开发业务

`new-business` 创建业务 crate 与 SDK/进程装配；`resource` 根据手写 Rust 模型生成 CRUD、迁移与受管资产。默认预览，仅 `--write` 写入；骨架、注册和锁文件一起更新，不升级已有锁定依赖。在资源生成前按[开发指南](docs/development.md#创建业务-crate)编写并登记模型：

租户业务模型使用 `database="tenant"` 和包含 `tenant_id` 的主键，表名保留 `biz_` 前缀；无 `tenant_id` 的全局模型使用 `database="control"`，表名可使用 `table1` 等安全标识，默认只允许系统租户访问。启用多租户不会自动共享全局表，共享读取需要明确的业务扩展。生成后审阅并执行 `cargo migrate -- control up` 初始化业务菜单和权限，再执行 `control verify`；新租户管理员默认获得租户业务 CRUD，普通用户及扩展操作由管理员手工授权。

```powershell
cargo generate -- new-business order-business --write
cargo generate -- resource --package order-business
cargo generate -- resource --package order-business --model Order --write
```

手写代码放在 `resources/` 和 `extensions/`。默认生成器离线；`import --tables biz_order,biz_order_item` 按需启用 `schema-import`，整批生成模型、CRUD 和迁移，原始结构保存在 `.ryframe/imports/`，不修改数据库。开启 `generator.sync_frontend` 或 `--sync-frontend` 后，候选 OpenAPI、typed caller、列表表单和路由经过类型及 lint 检查，再统一安装两仓文件；预览只写隔离缓存，`--no-sync-frontend` 临时关闭同步。不支持的结构整批报错，不接管原表。字段 `sql_type` 保留受支持的物理类型、容量和时间精度；表与字符串列的排序规则参与核验。工具默认值及前端目录在 `config/tools.toml` 配置，本机覆盖位于 `.local-tests/tools.toml`，命令行优先；`cargo generate -- run` 消费 `tables` 或 `models`。Rust 入口 `GenerationBuilder` 支持 `.include_tables(["biz_order", "biz_order_item"])`，可编辑运行[示例](crates/ryframe-generator/examples/generate.rs)，详细流程见开发指南；已有表使用显式接管计划，命令与失败核验见[数据指南](docs/data.md)。

精确 Decimal 字段须显式声明 `sql_type="DECIMAL(p,s)"`，目前支持完整数值域可容纳的 1≤p≤28、0≤s≤p。接口与表单使用十进制字符串，超出容量会拒绝，具体模型和默认值写法见开发指南。

## 检查、契约与构建

后端日常检查不需要前端。跨仓完整检查显式选择 all；前端目录改名后更新工具配置：

```powershell
cargo xtask check
cargo xtask check --scope all --full
cargo xtask generate api --write
cargo xtask build --plan
cargo xtask build
```

契约同步使用本次后端候选 OpenAPI 并原子更新前端派生文件；正式来源绑定与候选同步有不同验证要求。构建核对开始、结束的源码指纹，`--plan` 不执行任务。完整恢复、浏览器与性能验收仍需当前源码证据，不能把历史测量当作改造后的结果；当前不代表已具备正式版发布条件。

## 文档

- [架构与扩展](docs/architecture.md)：模块边界、SDK 和业务装配。
- [开发指南](docs/development.md)：工具链、运行、模型和生成流程。
- [API 指南](docs/api.md)：会话、认证、错误和契约。
- [数据指南](docs/data.md)：初始化、多库、租户路由、复制与恢复。
- [运维指南](docs/operations.md)：部署、升级、reset 和排障。

字段、菜单、权限与默认值以 OpenAPI、访问目录、配置结构和命令帮助为准。MQTT、邮件、乐观锁等按需插件属于后续路线，本轮不增加占位接口或额外服务要求。
