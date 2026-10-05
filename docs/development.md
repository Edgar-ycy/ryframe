# 开发指南

当前契约生成使用 `core/system/platform/monitor` 四个领域。套餐能力目录可以为空，开发新业务能力时仍需明确登记 capability，未知能力继续被拒绝。标准 CRUD 的生成与幂等检查保持不变。

产品套餐的能力选项来自后端编译目录和前端 feature manifest。用户、角色、菜单、部门等租户管理功能可以按套餐启停；停用后管理 API 和页面关闭，已有数据、认证和内部只读投影保留。租户、产品套餐、数据目标及平台基础设施管理属于系统租户，不能通过普通租户的权限通配符、菜单授权或配置包开放。系统租户具备已部署的管理能力，仍受实际基础设施可用性校验。

## 环境与配置

本地开发以 Windows 为准，MySQL 和 RustFS 在 Windows 运行，Redis 连接 WSL 中的实例，应用直接在 Windows 启动。安装 rustup 后，在项目目录运行 `rustup show` 安装并选择固定的 Rust 1.98.1、rustfmt 和 Clippy；项目使用 Rust 2024 edition，最低 Rust 版本为 1.98，本地、CI 和生产构建镜像使用同一固定工具链。
选择开发配置后再运行命令，例如 `$env:APP_ENV = "dev"`。配置文件位于 `config/`，环境变量使用 `APP_` 前缀覆盖对应字段；外部服务地址、密码、令牌和证书通过本机环境变量或密钥文件提供，所有可用字段和校验范围以配置结构及启动错误为准。
## 启动后端

首次启动先验证数据库结构，再启动开发进程：
```powershell
cargo migrate -- control verify
cargo serve
```
`cargo serve` 只启动 API，不会同时启动 Worker 或前端。需要后台任务时另开终端运行 `cargo worker`；需要联调页面时，在 `ryframe-vue3` 仓库中运行 `corepack pnpm dev`。`cargo xtask dev` 保留给 RyFrame 框架维护者验证热切换状态机，不是使用者开发业务的启动入口。

框架维护者需要热切换验证时使用 `cargo xtask dev`；它按实际变更选择 API、Worker 或迁移目标，只验证已登记数据库，不自动升级。常用排障从 `cargo xtask check doctor` 开始。
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
## 创建业务 crate

使用普通 Cargo 命令创建业务 crate，并在清单中声明 RyFrame 元数据：

```powershell
cargo new crates/order-business --lib
```
```toml
[package.metadata.ryframe]
kind = "business"
module = "order"

[features]
default = ["api", "migration"]
catalog = []
persistence = ["ryframe-sdk/persistence"]
api = ["persistence", "ryframe-sdk/api"]
migration = ["persistence", "ryframe-sdk/migration"]

[dependencies]
ryframe-sdk = { path = "../ryframe-sdk", default-features = false }
```

推荐目录沿用原 `example` 中易读的职责分类，并把可重生成文件统一放入 `generated`：

```text
order-business/
  src/
    lib.rs
    resources/                 # 手写 ResourceModel
    extensions/                # 手写复杂业务
    generated/
      entities/ dto/ repositories/ services/ handlers/ openapi/ models/
  migrations/                  # 本模块迁移
  .ryframe/generated.toml      # 生成文件 ownership
```

`src/lib.rs` 只需公开描述符和模块：

```rust
pub mod resources;
#[cfg(any(feature = "api", feature = "migration"))]
pub mod generated;

pub fn resource_descriptors() -> Vec<ryframe_sdk::ResourceDescriptor> {
    vec![<resources::Order as ryframe_sdk::ResourceModel>::descriptor()]
}

#[cfg(any(feature = "api", feature = "migration"))]
pub fn module() -> ryframe_sdk::RyFrameBusinessModule {
    let builder = ryframe_sdk::BusinessModuleBuilder::new("order")
        .resources(&generated::RESOURCES);
    #[cfg(feature = "api")]
    let builder = builder.routes(generated::routes).openapi(generated::openapi);
    #[cfg(feature = "migration")]
    let builder = builder.migrations(generated::migrations::migrations());
    builder.build()
}
```

模型使用 `#[derive(ryframe_sdk::ResourceModel)]`。租户表以 `biz_` 开头，并把 `tenant_id` 放入联合主键。然后运行：

```powershell
cargo generate -- resource --package order-business
cargo generate -- resource --package order-business --model Order --write
```

命令可从工作区根目录或该 crate 的任意子目录执行。默认预览只报告差异，`--write` 才原子更新生成文件；生成器不会写入 `resources/` 和 `extensions/`，也不会覆盖已经生成的迁移。数据库反向导入只用于首次创建模型草案。

最后在 `crates/ryframe/Cargo.toml` 添加 `order-business` 依赖，并在 `crates/ryframe/src/business.rs` 的 `business_modules()` 中加入 `order_business::module()`。API、Worker 和迁移程序会读取同一列表。模块名、资源名、表名、路由、OpenAPI operation/schema 和依赖循环在启动前校验。

Post 和 Notice 仍是 RyFrame 框架自身的标准资源。框架维护者继续使用内部 `cargo xtask generate resource` 检查它们；使用者业务不写 `catalog/resources/*.toml`。

## 开发自定义业务

不能由标准资源表达的流程按以下顺序实现：

1. 在自己的业务 crate 的 `extensions/` 中编写模型、用例、Repository 和 Handler；业务复杂后可自行拆成多个 crate。
2. 通过 `ryframe-sdk` 使用事务、数据库和 HTTP 扩展接口，在该 crate 的 `module()` 中注册路由、OpenAPI 和迁移。
3. 覆盖业务成功和失败路径的测试，同步前端契约并联调；模块选择和请求流见[架构说明](architecture.md)。
## API 与前后端联调

接口变化后运行 `cargo xtask generate api --write`，从当前后端代码生成候选 OpenAPI 并刷新前端 operation descriptor；随后进入前端项目执行消费者检查和浏览器 smoke，确认请求、权限、菜单与页面行为一致。OpenAPI 与数据库结构快照均由对应的生成或迁移维护入口产生，不直接运行内部二进制。
## 生产构建

`cargo xtask build` 从同一构建计划依次运行三个任务：关闭默认 feature 后分别定向构建 API 和 Worker，再通过 Corepack 构建前端生产目录。每项完成后输出 Cargo 实际报告的可执行文件或完整前端目录的路径、字节数和 SHA-256；`--profile dev` 只改变构建 profile，任务边界保持不变。

`cargo xtask build --plan` 渲染这份计划的依赖、有效参数、输入范围、编译覆盖和允许写入，不启动 Cargo、Corepack 或服务，也不创建目录、缓存和报告。实际执行在首个任务前和全部任务后核对前后端工作树指纹；任一来源在构建期间变化都会失败，已生成文件不能作为同源成功产物。
## 测试与检查

日常修改使用智能检查，联调完成后使用完整检查：

```powershell
cargo xtask check
cargo xtask check --full
```
只运行后端或前端主要检查时可添加 `--scope backend` 或 `--scope frontend`。

模块测试使用 `cargo test --locked -p <包名>`，例如 `ryframe-application` 或 `ryframe-api`。

真实 MySQL 和 Redis 协议测试默认不连接外部服务；需要单独诊断时，显式启用对应的 `RYFRAME_MYSQL_INTEGRATION` 或 `RYFRAME_REDIS_INTEGRATION`，再运行 `mysql_real_protocol` 或 `redis_real_protocol` 测试目标。完整集成使用下面的统一入口。

完整集成门禁会验证 MySQL required TLS，以及 Redis TLS、S3 HTTPS 和 OTLP HTTPS 三个 AWS-LC 真实出站测试。Windows 本地需有 `python`、`openssl`，并在 `127.0.0.1` 启动隔离的 MySQL 与 Redis；Redis 建议使用 WSL 独立实例和数据库 15。门禁只接受回环 Redis，不执行 `KEYS`、`SCAN` 或 `FLUSH`，测试 key 由唯一 `scope_id` 隔离并精确删除。运行入口与 CI 相同：

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

框架维护者通过 `cargo xtask check` 执行按变更规划的检查，通过 `cargo xtask check --full` 执行浏览器以外的完整门禁。性能、恢复、发布和资源门禁均属于框架维护入口，具体参数以 `cargo xtask check --help` 为准；报告写入 `.local-tests/` 并绑定源码、环境和运行收据。

需要真实 MySQL、Redis、对象存储或浏览器的检查必须使用已登记的隔离环境。性能对比保持相同数据、配置和冷热条件，保存全部有效与失败样本；全栈恢复从初始化到业务核验连续运行。生产数据、共享数据库和未登记服务不得作为开发夹具。
## 常见问题

- 启动提示迁移不一致：运行 `cargo migrate -- control status` 与 `cargo migrate -- control verify`，确认后再执行 `cargo migrate -- control up`；Worker 未消费任务时，确认 API/Worker 使用 `APP_JOBS_MODE=external`，检查健康端口、lease 和数据库连接。
- 前端请求与后端不一致：重新运行 `cargo xtask generate api --write` 和消费者检查；Redis 或对象存储不可用时，检查 `scope_id`、连接模式、TLS、ownership marker 与服务端口，详见[运维指南](operations.md)。
