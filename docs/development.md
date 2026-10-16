# 开发指南

产品套餐的能力选项来自后端编译目录和前端 feature manifest。基础管理功能保持可用并遵循 RBAC，通知公告与租户主动消息发布可以按套餐启停。首批能力使用 `standard` 变体和严格空对象配置；停用公告后管理与读取入口关闭，停用主动消息后拒绝新的发布，已有数据和已接受消息保留，系统通知继续运行。租户、产品套餐、数据目标及平台基础设施管理属于系统租户，不能通过普通租户的权限通配符、菜单授权或配置包开放。系统租户具备已部署的管理能力，仍受实际基础设施可用性校验。

## 环境与配置

本机开发验证使用 Windows。MySQL、Redis 和 RustFS 可以运行在任意满足版本、协议及连接要求的平台，也可以连接远程服务；不要求使用 WSL。安装 rustup 后，在项目目录运行 `rustup show` 安装并选择固定的 Rust 1.98.1、rustfmt 和 Clippy；项目使用 Rust 2024 edition，最低 Rust 版本为 1.98，本地、CI 和生产构建镜像使用同一固定工具链。
选择开发配置后再运行命令，例如 `$env:APP_ENV = "dev"`。配置文件位于 `config/`，环境变量使用 `APP_` 前缀覆盖对应字段；外部服务地址、密码、令牌和证书通过本机环境变量或密钥文件提供，所有可用字段和校验范围以配置结构及启动错误为准。
## 启动后端

本地初始化、管理员密码和 API/Worker/前端启动顺序见 [README](../README.md)。`cargo serve` 只启动 API，后台任务使用独立的 `cargo worker`，前端单独运行 `corepack pnpm dev`。

框架维护者用 `cargo xtask dev` 验证热切换；它只验证已登记数据库，不自动升级。常用排障从 `cargo xtask check doctor` 开始。

## 数据库迁移

控制库使用默认目标；租户数据可操作全部已登记目标或单个目标；新迁移必须用对应命令创建骨架：

```powershell
cargo migrate -- control status
cargo migrate -- control verify
cargo migrate -- control up
cargo migrate -- tenant-data verify --all
cargo migrate -- tenant-data verify --target <目标键>
```
生产部署和非生产重建步骤见[数据指南](data.md)与[运维指南](operations.md)。

初始化前设置 `RYFRAME_INIT_ADMIN_PASSWORD_FILE` 指向 UTF-8 密码文件，也可通过 `RYFRAME_INIT_ADMIN_PASSWORD` 提供；两者不能同时设置。密码须包含大小写字母、数字、符号，长度为 8～72 个可见 ASCII 字符。普通初始化用户 `user`、新建及批量导入的普通用户使用 `123456`，每个新账号首次登录都必须改密。已有正常账号不会被重复初始化重置。

## 创建业务 crate

使用正式脚手架创建业务 crate：

```powershell
cargo generate -- new-business order-business
cargo generate -- new-business order-business --write
```

默认在隔离副本预览骨架和锁文件，`--write` 才统一安装；离线解析只允许新增本次本地业务包，不升级、换来源或移除已有锁定依赖。原项目需有有效 Cargo.lock 和已安装的锁定依赖，缺失时明确报错且不安装半个 crate。创建事务更新业务 crate 的 `kind = "business"` 元数据和 SDK feature、工作区成员、组合根的可选依赖与 API/Worker/迁移/数据维护 feature，以及 `business_modules()` 注册；不会生成 CRUD 或操作数据库。命令和 Builder 均接受模块名 `order` 或包名 `order-business`，显式名称决定新包，不使用配置中的默认包。模块名须以小写字母开头。已有 crate、重复注册或计划生成后的并发修改均拒绝覆盖；中途失败尝试恢复原文件，回滚未完成时保留并指出恢复目录。Windows CRLF 输入按原始字节校验后输出 LF。

业务 crate 按职责分类，并把可重生成文件统一放入 `generated`：

```text
order-business/
  src/
    lib.rs
    resources/                 # 手写 ResourceModel
    extensions/                # 手写复杂业务
    generated/
      entities/ dto/ repositories/ services/ handlers/ openapi/ models/ resource_catalog/
  migrations/                  # 本模块迁移
  .ryframe/generated.toml      # 生成文件 ownership
```

脚手架已生成 `src/lib.rs` 的模块装配；模块名中的中划线在 API 路径、operation ID 和页面路径中统一为下划线；权限段及菜单路由键统一使用中划线，例如 `sales-orders.order-item`。业务菜单标题键由资源名转换为小驼峰，例如 `order_item` 对应 `orderItem`；与现有翻译键冲突时停止组合，需修改资源名重新生成。资源只需在 `resources/mod.rs` 中登记描述符：

```rust
pub fn descriptors() -> Vec<ryframe_sdk::ResourceDescriptor> {
    vec![<Order as ryframe_sdk::ResourceModel>::descriptor()]
}
```

模型级 `#[resource(primary_key = ["tenant_id", "id"])]` 可明确索引顺序，与字段顺序独立，列表须恰好包含全部主键标记字段，省略时沿用字段顺序。反向导入从 PRIMARY 索引读取顺序，将业务主键映射到 Rust `id` 并用 `column="order_id"` 等属性保留物理列名；与已有 `id` 列冲突时拒绝生成。实体按主键顺序声明字段，DDL 保留原物理列顺序。租户主键必须以 `tenant_id` 开头，其他复合主键明确阻断，不自动修改原库。审计时间列组可省略，此时物理删除或软删除均不补写时间列；已声明的时间及操作者字段仍严格校验并由 Service 管理，请求级操作审计继续遵循原有事务策略。

模型使用 `#[derive(ryframe_sdk::ResourceModel)]`。在 `resources/mod.rs` 定义模型后，将其描述符加入同文件的 `descriptors()`。`database="tenant"` 用于租户业务，表以 `biz_` 开头并把 `tenant_id` 放入联合主键；`database="control"` 用于无 `tenant_id` 的全局业务，Service 与持久化只允许系统租户访问，页面资产携带同样的访问限制。启用多租户不会使全局业务共享，跨租户共享读取须编写明确的用例扩展。ID 默认由应用生成雪花值；数据库自增主键使用 `#[resource(primary_key, auto_increment)] pub id: i64`（无符号列使用 `u64`），不声明默认值或表单写入。生成器为租户复合主键补充以 id 开头的非唯一索引，插入时省略 ID，在同一事务中取得实际自增值。然后运行：

```powershell
cargo generate -- resource --package order-business
cargo generate -- resource --package order-business --model Order --write
```

命令可从工作区根目录或该 crate 的子目录执行。预览在隔离缓存编译模型、生成 CRUD 与迁移并报告差异；`sync_frontend=true` 或 `--sync-frontend` 还会导出本次候选 OpenAPI、同步 typed caller、生成列表表单和路由，并执行前端类型与 lint 检查。需先安装前端依赖；`--no-sync-frontend` 可临时关闭。全部成功后，`--write` 统一安装两仓受管文件；并发改动拒绝覆盖，失败回滚未完成时报告备份路径。默认页面使用生成的 page.vue，模型的 `frontend_page="@/views/…/index.vue"` 可指定手写页面。`resource` 不写手写目录或覆盖历史迁移。描述符 reader 离线使用 Workspace 锁定版本、来源、校验和及 patch，不修改项目锁文件。配置支持 `package`、`models`、`tables`、`source`、`storage`、`strip_table_prefix` 和 `sync_frontend`；`resource --models Order,OrderItem` 覆盖模型列表，`import --tables biz_order,biz_order_item` 选择完整表批次。`run` 根据 tables 是否为空选择导入或模型生成，导入需 `--features schema-import`；`--plan` 只列同一执行计划，不编译、不连接数据库、不落盘。

脚手架已经在 `crates/ryframe/Cargo.toml` 添加 `order-business` 依赖，并在 `crates/ryframe/src/business.rs` 的 `business_modules()` 中加入 `order_business::module()`。API、Worker 和迁移程序会读取同一列表。模块名、资源名、表名、路由、OpenAPI operation/schema 和依赖循环在启动前校验。

反向导入只读读取选中表，整批预检名称、模型、文件与 `descriptors()` 显式 vec 列表，在候选工作区编译模型并继续生成 CRUD 与迁移；发生冲突整批停止，不覆盖手写模型。`--strip-table-prefix biz_` 仅改变资源、模块及类型名，保留物理表名；租户目录要求表名带该前缀。原始列类型、默认值、索引、外键及 schema 保存在 `.ryframe/imports/`。`source=primary`、`sources.legacy` 或 `tenant_data.targets.<键>` 决定只读来源，`storage=control|tenant` 决定运行范围，二者独立。开启前端同步时也校验本批契约和页面；生成结构与来源不一致（如尚未支持的外键）会整批停止，不安装半批模型。生成不建表或接管原表，仍须审阅用途并显式迁移或 adopt。未知类型、视图和不可读列拒绝，`tinyint(1)` 不猜测布尔；日期、JSON 等控件继续校验支持范围。Rust [示例](../crates/ryframe-generator/examples/generate.rs) 可用 `cargo run --locked -p ryframe-generator --example generate` 或 IDE 运行，导入需 `--features schema-import`；Builder 的 `plan()` 只读，`execute()` 才执行，`.write(true)` 才更新文件。

模型级 `#[resource(collation="utf8mb4_0900_ai_ci")]` 声明表排序规则；字符串字段可用 `#[resource(collation="utf8mb4_bin")]` 覆盖。未声明的表使用 `utf8mb4_general_ci`，未声明的列继承表规则。导入读取实际表、列规则并保存在 Rust 模型中，DDL、结构指纹、复制目录和接管校验共用声明。只接受安全的 utf8mb4 规则名称，实际可用性由目标 MySQL 校验；其他字符集须先审阅迁移或使用扩展。既有模型改变排序规则时整批停止自动迁移，先核对比较语义、唯一索引和存量数据，不自动转换。

### 模型结构演进

修改 Rust 模型后继续运行相同的 `cargo generate -- resource --package order-business` 预览，审阅后加 `--write`。生成器保留全部历史迁移原字节及摘要，只追加稳定编号的新迁移，`src/generated/migrations/history.json` 与 ownership 一起保存前后结构链；这一步不连接或修改数据库。

字段可用 `#[resource(sql_type="VARCHAR(32)")]` 保留物理类型；导入自动写入该属性，DDL、复制目录和接管校验共用它。当前支持与 Rust 类型匹配的 CHAR/VARCHAR、TEXT 系列、有符号整数及 `u8/u16/u32/u64` 无符号整数、布尔、DATE、DATETIME（0～6 位小数）和 JSON；推导字符长度、文本字节容量及整数范围，矛盾校验、未知声明或 SQL 子句在写入前报错。整数 CRUD 保留 Rust 位宽及符号，默认值和枚举必须在该类型范围内；物理 SQL 容量须完整适配模型，增量转换按物理范围检查。`u64` 普通字段保留完整无符号容量并使用字符串传输，超过 i64 的默认值使用十进制字符串；主键支持 `i64/u64`，自增及雪花策略、URL 和持久化保持类型一致；Decimal 必须显式声明 `DECIMAL(p,s)`（1≤p≤28、0≤s≤p），使用 SDK 的 `rust_decimal::Decimal`；正文、查询、响应及文本表单均以字符串传输。超出列容量或小数位数会拒绝，API 和 SDK 用例均不做自动舍入；日期、JSON 等控件限制继续生效。

复合索引用结构体属性 `#[resource(index(name="idx_code",fields=["code","id"]))]` 声明，唯一索引加 `unique`；导入保留索引名、字段顺序和唯一性，拒绝前缀、降序、函数、不可见及非 BTREE 索引。仅为自增联合主键补充必要的 id 首列索引，不给主键已覆盖字段补冗余索引。

字段的 `#[resource(sql_comment="数据库注释")]` 独立于界面标签；导入、DDL、模型摘要及追加迁移保留该值，结构指纹仍只核对结构。引号、反斜杠、换行原样保存；字段注释不能含 NUL 或超过 1024 个字符。

字面量默认值使用字段 `default="值"`，数据库时钟使用 `sql_default="CURRENT_TIMESTAMP(6)"`、`sql_on_update="CURRENT_TIMESTAMP(6)"`；时钟只支持与 DATETIME 列一致的 0～6 位精度，不接受任意 SQL，`default` 与 `sql_default` 不能并用。导入保留这两类默认值及更新时间表达式；应用显式写入及可空替换语义仍生效。日期、Decimal 的字面量默认值须符合当前 CRUD 支持范围，其他表达式使用明确迁移扩展。外键仍需核对导入元数据。

- 新增可空列、带显式默认值的非空列、整数或字符串扩宽、放宽非空约束及新增索引可生成向前 DDL；唯一索引仍须先核对存量重复值。
- 删除或重命名列、收窄类型、收紧非空约束、改变主键/表名/存储范围、调整已有列顺序或删除/修改索引只输出审阅原因，整批停止，不覆盖初始迁移或发布半批注册。非空新列没有默认值时，先用可空列分阶段回填。
- 审阅迁移后停止生产者，再显式执行 control 或所选 tenant-data 目标的 `up` 和 `verify`。数据库被直接改动时 verify 报告漂移，生成器不会反向覆盖模型或猜测已执行状态。
- 可空可写字段使用完整替换：创建时省略或传 null 均为空，更新时省略或传 null 均清空该字段；非空可选更新省略时保留原值。需要三态补丁时编写明确扩展。业务 `i64/u64` 默认字符串传输，正文、查询、返回值及文本表单保持精确十进制；不使用 number 控件。空文本提交为可空字段的 null 或可选字段的省略值，必填字段仍由表单与服务端校验；有限枚举保留字符串值，不转换为浮点数。

Post 和 Notice 仍是 RyFrame 框架自身的标准资源。框架维护者继续使用内部 `cargo xtask generate resource` 检查它们；使用者业务不写 `catalog/resources/*.toml`。

租户业务资源同时生成 `generated/tenant_catalog.rs` 与 `generated/tenant_catalog.json`，包含完整列、类型、复制游标及规范化 schema。模块通过 `tenant_data_tables()` 声明生成描述，组合根在连接数据库前统一装配、核验依赖并冻结目录；同一进程再次登记只能使用相同描述。重复表、缺失描述或无效 schema 会阻止启动，不会忽略业务表。复制和恢复校验使用完整目录，并保留业务迁移账本。生成迁移同时注册模型指纹与每张表的 before/after 物理结构；手写 `SeaOrmBusinessMigration` 也必须提供 `BusinessMigrationSchema`。单条 `verify_ledger` 仅校验历史账本，整组 `verify_business_migrations(database, scope, migrations)` 接收该范围的全部注册迁移，拒绝未知账本和归属冲突，再按每张表最新状态校验实际结构，避免合法追加迁移被旧结构误判。修改模型后先通过正式生成入口更新受管资产，再显式执行业务迁移；不要手改目录或 ownership 摘要。已有表须先用 `cargo migrate -- control adopt --module <模块>` 只读预览并按[数据指南](data.md)保存、审阅、执行和核验计划；租户接管必须选择单个目标，不用 up 猜测接管。

从模型到页面可按以下顺序联调：

1. 创建业务 crate、登记 `ResourceModel` 后预览生成，再用 `--write` 写入；再次预览应没有差异。
2. 为业务模型和用例添加测试，执行 `cargo test --locked -p order-business`。先通过控制库与明确租户目标的迁移状态检查，再显式应用本模块迁移，命令见[数据指南](data.md)。
3. 分别运行 `cargo serve` 与 `cargo worker`，确认模块路由和任务正常装配。接口变化后从后端执行 `cargo xtask generate api --write` 同步候选契约。OpenAPI 与数据库结构快照通过正式生成或迁移维护入口产生，不直接运行内部二进制。
4. 在权限管理中登记生成契约声明的业务权限码，并在角色管理中授予业务角色；业务接口的生成不会自动给已有租户增加权限。需要页面入口时，在菜单管理中绑定对应权限和前端页面标识。新增权限或变更角色授权后，确认会话已取得新的授权投影，再验证接口；未授权时接口应返回 403。
5. 在前端通过生成的 typed caller 接入业务 API；页面登记路由与 i18n namespace，查询和变更绑定当前会话 scope，方法见[前端架构说明](https://github.com/Edgar-ycy/ryframe-vue3/blob/main/ARCHITECTURE.md)。
6. 运行前端 `corepack pnpm check --full`，再用本地 Chrome 验证业务成功、权限拒绝、租户切换和取消。只在隔离开发环境准备数据，完成后核验自己创建的数据库与对象资源。

框架的自动化业务夹具使用同一脚手架，在忽略目录中生成使用者业务 crate 并核验重复生成；可通过 `cargo xtask check recovery fixture --output-dir <绝对隔离目录> --write` 创建，不会向框架产品加入内置业务模块。

## 开发自定义业务

不能由标准资源表达的流程按以下顺序实现：

1. 在自己的业务 crate 的 `extensions/` 中编写模型、用例、Repository 和 Handler；业务复杂后可自行拆成多个 crate。
2. 通过 `ryframe-sdk` 使用事务、数据库和 HTTP 扩展接口，在该 crate 的 `module()` 中注册路由、OpenAPI 和迁移。生成骨架已通过 `.control_tables(generated::CONTROL_TABLES)` 装配当前控制库业务结构；手写模块用 `BusinessControlTableDescriptor` 提供完整规范化结构，必须与控制库资源一一对应，不能只登记表名。
3. 覆盖业务成功和失败路径的测试，同步前端契约并联调；模块选择和请求流见[架构说明](architecture.md)。
## 生产构建

`cargo xtask build` 从同一构建计划依次运行三个任务：关闭默认 feature 后分别定向构建 API 和 Worker，再通过 Corepack 构建前端生产目录。每项完成后输出 Cargo 实际报告的可执行文件或完整前端目录的路径、字节数和 SHA-256；`--profile dev` 只改变构建 profile，任务边界保持不变。

`cargo xtask build --plan` 渲染这份计划的依赖、有效参数、输入范围、编译覆盖和允许写入，不启动 Cargo、Corepack 或服务，也不创建目录、缓存和报告。实际执行在首个任务前和全部任务后核对前后端工作树指纹；任一来源在构建期间变化都会失败，已生成文件不能作为同源成功产物。
## 集成验证

真实 MySQL 和 Redis 协议测试默认不连接外部服务；需要单独诊断时，显式启用对应的 `RYFRAME_MYSQL_INTEGRATION` 或 `RYFRAME_REDIS_INTEGRATION`，再运行 `mysql_real_protocol` 或 `redis_real_protocol` 测试目标。完整集成使用下面的统一入口。

完整集成门禁会验证 MySQL required TLS，以及 Redis TLS、S3 HTTPS 和 OTLP HTTPS 三个 AWS-LC 真实出站测试。本地需有 `python`、`openssl`，并在回环地址提供隔离的 MySQL 与 Redis；服务按版本和协议要求选择平台，测试显式选择独立 Redis 实例和数据库。门禁只接受回环 Redis，不执行 `KEYS`、`SCAN` 或 `FLUSH`，测试 key 由唯一 `scope_id` 隔离并精确删除。运行入口与 CI 相同：

```powershell
$env:RYFRAME_MYSQL_INTEGRATION = "1"
$env:RYFRAME_MYSQL_TLS_INTEGRATION = "1"
$env:RYFRAME_REDIS_INTEGRATION = "1"
$env:RYFRAME_REDIS_DATABASE = "15"
$env:RYFRAME_INTEGRATION_RUN_ID = "local-aws-lc"
cargo xtask check ci integration
```

TLS fixture 会生成两日有效的临时 CA，在动态回环端口启动 Redis TLS 代理和 HTTPS 服务；相关测试结束或失败后都会停止监听并删除临时证书。日志默认保存在 `.local-tests/integration/tls/<run-id>/`，历史目录不会覆盖；可用 `RYFRAME_TLS_ARTIFACT_DIR` 指定日志根目录。CI 对成功和失败运行都上传 14 天，失败摘要会输出每个已运行测试的最近日志。`RYFRAME_REDIS_HOST` 不是 `127.0.0.1`、`::1` 或 `localhost` 时门禁直接拒绝启动，避免误连共享 Redis。

真实全栈的 API 与 external Worker 每次分别由私有长驻监督进程托管。Windows 监督进程先把自身加入启用关闭即终止的私有 Job Object，Unix 监督进程先建立独立 session 和进程组，再启动产品代码；进程树收据绑定 scope、启动操作以及监督进程和产品进程的创建身份。停止与故障注入使用不同控制收据，保留产品真实退出码；正常停止先请求产品退出并等待宽限，随后才回收完整 Job 或进程组。Windows 当前没有可继承的温和控制台通道，正常停止触发的 `TerminateProcess` 会如实记录为强制终止。控制操作只有在完整进程树停止且监听端口释放后才完成，不会按 PID 猜测或影响未登记进程。
## 框架维护检查

框架维护者通过 `cargo xtask check` 执行按变更规划的后端检查，通过 `cargo xtask check --full` 执行完整后端门禁，均不要求检出前端。跨仓契约和资源 Workspace 消费验证使用 `cargo xtask check --scope all --full`；单独检查前端使用 `--scope frontend`。这些命令不隐式运行真实浏览器。性能、恢复、发布和资源门禁均属于框架维护入口，具体参数以 `cargo xtask check --help` 为准；报告写入 `.local-tests/` 并绑定源码、环境和运行收据。

前端改名或移动后，在后端 `config/tools.toml` 设置路径；仅本机使用的配置放在忽略目录 `.local-tests/tools.toml`：

```toml
[workspace]
frontend_dir = "../web"
```

优先级为命令行 `--frontend-dir`、本机工具配置、公共工具配置；相对路径统一以后端 Cargo Workspace 根目录解析，与命令在哪个子目录运行无关。工具配置不放服务连接密码，也不影响 `cargo serve` 的运行时配置。后端范围只检查后端，显式跨仓范围找不到前端时会输出实际路径与配置位置。

需要真实 MySQL、Redis、对象存储或浏览器的检查必须使用已登记的隔离环境。性能对比保持相同数据、配置和冷热条件，保存全部有效与失败样本；全栈恢复从初始化到业务核验连续运行。生产数据、共享数据库和未登记服务不得作为开发夹具。
## 常见问题

- 启动提示迁移不一致：运行 `cargo migrate -- control status` 与 `cargo migrate -- control verify`，确认后再执行 `cargo migrate -- control up`；Worker 未消费任务时，确认 API/Worker 使用 `APP_JOBS_MODE=external`，检查健康端口、lease 和数据库连接。
- 前端请求与后端不一致：重新运行 `cargo xtask generate api --write` 和消费者检查；Redis 或对象存储不可用时，检查 `scope_id`、连接模式、TLS、ownership marker 与服务端口，详见[运维指南](operations.md)。
