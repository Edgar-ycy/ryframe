# RyFrame

[English](README.en.md)

RyFrame 是面向企业后台的 Rust 2024 框架，与 Vue 3 管理端配套使用。当前为 `0.x` 开发版本，请使用当前契约和全新隔离数据库，不能直接覆盖旧开发库。

提供用户、角色、权限、组织、产品套餐、多租户、配置迁移、跨库迁移、监控、消息、调度、导入导出，以及个人资料、密码、头像和登录会话管理。套餐可发布空能力版本；通知公告和租户主动消息发布可按套餐启停，停用不删除历史数据，也不关闭系统通知、调度、导入导出或备份恢复。

## 下载后首次运行

后端和前端是独立仓库，以下命令在后端仓库根目录执行。本地示例使用 Windows PowerShell；MySQL、Redis 和 RustFS/MinIO/S3 兼容存储可以在满足版本、协议和连接要求的任意平台或远程服务器运行，不要求 WSL。

1. 安装 rustup，在仓库内运行 `rustup show`，使用 `rust-toolchain.toml` 固定的正式 Rust、rustfmt 和 Clippy。
2. 准备 MySQL 8.0.16 或更高版本（不以 MariaDB 替代），以及具备当前数据库建库、迁移和业务读写权限的账号。Redis 使用 RESP 连接，当前验收基线为 Redis 7.x（CI 使用 7.4.9），按部署方式提供 AUTH、独立 database 和 TLS。文件存储默认使用本地目录，也支持 RustFS、MinIO 和 S3 兼容服务。
3. 使用数据库客户端创建空库，例如：

```sql
CREATE DATABASE ryframe_dev CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci;
```

4. 先新建忽略目录 `.local-tests/dev-config`，把公共样例 `config/app.toml` 和开发样例 `config/app.dev.toml` 复制进去；目录已存在就停止，保留用户真实配置并另选新目录，具体复制步骤见[开发指南](docs/development.md#启动后端)。只在副本中填写真实值，不把凭据写回受版本控制的样例或提交到仓库；若另选目录，同步替换下列命令的配置目录参数。数据库密码、至少 32 字节的随机 JWT 密钥和初始管理员密码直接写入字段；管理员密码为 8～72 个可见 ASCII 字符，包含大小写字母、数字和符号。
5. 在编辑器中修改 `.local-tests/dev-config/app.dev.toml` 的地址、端口和用户。以下是该副本的相关字段片段；替换密码占位值，Redis 如需认证直接填写 `redis.password`。同一部署每个实例的根字段 `snowflake_worker_id` 必须不同：

```toml
scope_id = "my-development"
snowflake_worker_id = 1
[bootstrap]
admin_password = "Replace-With-Strong-Admin-Password1!"
[database.primary]
host = "127.0.0.1"
database = "ryframe_dev"
username = "ryframe_app"
password = "replace-with-mysql-password"
[auth]
jwt_secret = "replace-with-at-least-32-random-bytes"
[redis]
password = ""
[object_storage]
backend = "local"
local_base_dir = ".local-tests/uploads"
```

```powershell
$env:APP_ENV = "dev"
cargo migrate -- --config-dir .local-tests/dev-config control up
cargo migrate -- --config-dir .local-tests/dev-config control verify
cargo migrate -- --config-dir .local-tests/dev-config tenant-data up --target shared-control
cargo migrate -- --config-dir .local-tests/dev-config tenant-data verify --target shared-control
cargo serve -- --config-dir .local-tests/dev-config
```

`up` 初始化或升级明确选择的控制库，`verify` 只校验。新空库上直接运行 `control verify` 会报告迁移账本或结构不完整；它不能代替首次初始化。控制库和 `shared-control` 租户数据面使用独立账本，因此默认单租户也需要完成两条初始化与校验命令。API/Worker 启动时不执行 DDL。API 默认端口为 8080；非生产环境默认 `jobs.mode="embedded"`，由 `cargo serve -- --config-dir .local-tests/dev-config` 同时消费后台任务。若要单独运行 Worker，先把已配置的 `.local-tests/dev-config` 中两份文件复制到两个全新目录 `.local-tests/api-config` 与 `.local-tests/worker-config`，分别供 API／Worker 使用；在编辑器中将根字段设为 API `snowflake_worker_id=1`、Worker `snowflake_worker_id=2`，两份均设置 `jobs.mode="external"`，同一部署的 scope、数据库、Redis 与 JWT 保持一致。按[开发指南的双目录步骤](docs/development.md#启动后端)创建文件并在两个终端分别运行 `cargo serve -- --config-dir .local-tests/api-config` 与 `cargo worker -- --config-dir .local-tests/worker-config`，不共用默认配置目录。以上文件配置把开发配置中的 RustFS 改成 `.local-tests/uploads` 本地存储；若要使用 RustFS/MinIO/S3，在 `object_storage` 中填写对应 backend、HTTPS endpoint、明文凭据和区域，并确认服务协议与连接可用。若隔离开发用的 MySQL 未启用 TLS，可显式设置 `database.primary.tls_mode="disabled"`；远程生产必须使用 `verify_identity`。前端单独安装和启动，见前端 README。

普通初始用户 `user`、新建及批量导入的普通用户初始密码为 `123456`；管理员使用显式配置的密码。所有新账号首次登录必须改密，改密前不能操作业务，改密成功后重新登录。重复迁移不会重置已有密码或改密状态。已有开发版普通账号密码未知时，按[显式账号恢复流程](docs/api.md#异常账号恢复)生成一次性链接，由用户自行设置密码。

新配置默认单租户，身份固定使用系统租户，登录页不选租户。需要多租户时在配置文件中设置 `multi_tenancy.enabled = true`。已有部署的显式选择继续生效；单租户仍可使用配置迁移和数据维护。

## 数据库、配置与 SQL 导入

`APP_ENV=dev|test|prod` 是唯一项目配置环境入口。配置按可省略的公共 `app.toml`、必须存在的 `app.<environment>.toml` 依次递归合并；省略公共文件时，环境文件须完整提供必填配置，已存在但不可读或损坏的公共文件仍失败；密码、JWT、监控、Redis 与存储凭据直接使用明文字段。CLI 通过 `--config-dir <directory>` 选择配置目录，默认 `config`，本页本机示例统一显式使用忽略目录 `.local-tests/dev-config`；应用相对路径以当前工作目录解析，生成器相对路径以后端 Workspace 根目录解析。数组在环境文件中整体替换，不能只覆盖其中一项。多库的 primary、replicas、sources，租户共享控制库、共享业务库、独立库，placement、TLS、连接池和完整 TOML 示例见[数据指南](docs/data.md#租户路由)。`app.locales_dir` 在 TOML 中显式指定，默认 `locales`，普通应用相对进程当前工作目录解析；不读取 `APP_LOCALES_DIR`，生产缺少资源严格失败，开发环境可使用内嵌回退。共享 MySQL 测试使用后端根 `.local-tests/mysql/config` 的显式文件，详见[集成验证](docs/development.md#集成验证)。部署四角色模板与私有配置目录已集成，资产结构及 CI 工具测试通过；真实容器、镜像和部署整链仍未验收，局部验证不等于整体门禁通过。

```powershell
cargo migrate -- --config-dir .local-tests/dev-config control export-sql --output .local-tests/control-init.sql --write
```

推荐迁移初始化；需要直接导入时先导出上方完整 SQL，再选择空库，按[数据指南](docs/data.md#控制库初始化-sql)遇错即停导入，再执行 `cargo migrate -- --config-dir .local-tests/dev-config control verify`。正式导出包含密码哈希、后置约束、种子和迁移账本；`sql/ryframe_config.sql` 仅供源码审阅，不能代替正式文件。导入中断不自动重放或清库，也不通过 `control up` 猜测补齐。该 SQL 一次初始化所选物理控制库内的控制面与 `shared-control` 租户数据面，包含两账本及已注册的两侧业务迁移；导入成功后直接执行 `control verify` 与 `tenant-data verify --target shared-control`，无需补初始化 `up`。独立 MySQL 租户目标仍须另行执行 `tenant-data up` 和 `verify`，不会由一份 SQL 跨实例初始化。SQL 在登记每次业务迁移前核验实际结构，最后复核框架结构、业务结构和账本；不一致即停止，不能把合法但偏离声明的 DDL 记为完成。

租户业务库先创建空库并登记目标，再显式执行 `cargo migrate -- --config-dir .local-tests/dev-config tenant-data up --target <target-key>` 和 `verify`。`--target` 使用配置中的目标键，不是数据库名；`--all` 只处理已登记目标。非生产重建使用 `cargo reset -- <参数>`，先停止 API、Worker 和调度器，步骤见[运维指南](docs/operations.md)。

`cargo xtask dev --config-dir .local-tests/dev-config` 使用完整私有文件快照管理 API 与 Worker；保存端口或根实例编号后复用二进制切换新代次，失败恢复旧配置与旧端口。相对配置目录以后端 Workspace 根目录解析，也可在工具文件的 `[development].config_dir` 指定；`--plan` 不读配置或执行任务。该监督器改造仍待集成运行验证，不能据此声称完整验收。

## 开发业务

`new-business` 创建业务 crate 与 SDK/进程装配；`resource` 根据手写 Rust 模型生成 CRUD、迁移与受管资产。默认预览，仅 `--write` 写入；骨架、注册和锁文件一起更新，不升级已有锁定依赖。在资源生成前按[开发指南](docs/development.md#创建业务-crate)编写并登记模型：

租户业务模型使用 `database="tenant"` 和包含 `tenant_id` 的主键，表名保留 `biz_` 前缀；无 `tenant_id` 的全局模型使用 `database="control"`，表名可使用 `table1` 等安全标识，默认只允许系统租户访问。启用多租户不会自动共享全局表，共享读取需要明确的业务扩展。生成后审阅并执行 `cargo migrate -- --config-dir .local-tests/dev-config control up` 初始化业务菜单和权限，再执行 `control verify`；新租户管理员默认获得租户业务 CRUD，普通用户及扩展操作由管理员手工授权。

手写代码放在 `resources/` 和 `extensions/`；生成器默认离线，已有数据库只读导入才启用 `schema-import`。

```powershell
cargo generate -- new-business order-business --write
# 编写并登记模型后预览，再显式写入
cargo generate -- resource --package order-business
cargo generate -- resource --package order-business --model Order --write

# 已有数据库的可选只读导入，独立于上面的模型驱动流程
cargo generate --features schema-import -- import --package order-business --connection primary --database tenant --tables biz_order,biz_order_item --config-dir .local-tests/dev-config
```

`--connection` 只选择只读结构来源，可写 `primary`、`sources.<name>` 或 `tenant_data.targets.<target-key>`；`--database` 单独决定生成模型的运行范围，二者不会自动联动。导入整批生成模型、CRUD 和迁移，原始结构保存在 `.ryframe/imports/`，不修改数据库，也不接管已有表。字段 `owner_field` 声明模型内保存用户 ID 的 `i64` 或 `Option<i64>` 字段；生成器会在创建时写入当前用户，并在列表、详情、更新和删除中应用同一数据范围。开启 `generator.sync_frontend` 或 `--sync-frontend` 后，候选 OpenAPI、typed caller、列表表单和路由经过类型及 lint 检查，再统一安装两仓文件；预览只写隔离缓存，`--no-sync-frontend` 临时关闭同步。不支持的结构整批报错。字段 `sql_type` 保留受支持的物理类型、容量和时间精度；表与字符串列的排序规则参与核验。工具默认值及前端目录在 `config/tools.toml` 配置，本机覆盖位于 `.local-tests/tools.toml`，不通过环境变量覆盖；生成器的应用目录优先级为显式 `--config-dir <directory>` / Builder `.configuration_directory(path)`、本机工具文件、公共工具文件、`workspace/config`，可选 `[generator] config_dir` 的相对路径同样以后端 Workspace 根目录解析；`cargo generate -- run` 消费 `tables` 或 `models`。Rust 入口 `GenerationBuilder` 支持 `.include_tables(["biz_order", "biz_order_item"])`，可编辑运行[示例](crates/ryframe-generator/examples/generate.rs)，详细流程见开发指南；已有表必须先生成匹配模型，再使用显式接管计划，命令与失败核验见[数据指南](docs/data.md#已有表接管)。

精确 Decimal 字段须显式声明 `sql_type="DECIMAL(p,s)"`（1≤p≤28、0≤s≤p）；接口与表单使用十进制字符串，超出容量会拒绝。标准表单按字段类型提供文本或文本域、整数、开关、日期、日期时间、枚举单选和精确 Decimal 文本控件。JSON 使用可编辑文本域，提交前严格解析；标准查询拒绝 JSON 筛选，结构化条件须编写强类型扩展。模型和默认值写法见开发指南。

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
