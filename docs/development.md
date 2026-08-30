# 开发指南
## 环境与配置

本地开发以 Windows 为准。MySQL 和 RustFS 在 Windows 运行，Redis 连接 WSL 中的实例；应用本身直接在 Windows 启动。
选择开发配置后再运行命令，例如 `$env:APP_ENV = "dev"`。配置文件位于 `config/`，环境变量使用 `APP_` 前缀覆盖对应字段；外部服务地址、密码、令牌和证书通过本机环境变量或密钥文件提供，所有可用字段和校验范围以配置结构及启动错误为准。
## 启动与热切换

首次启动先验证数据库结构，再启动开发进程：
```powershell
cargo migrate verify
cargo dev
```
`cargo dev` 启动 Vite、API 和独立 Worker。后端变化会先编译到按会话和代次隔离的新目录，并在隔离端口探活；探活成功后才切换正式端口，失败时继续使用上一个可用版本。再次启动时会清理上次崩溃留下的暂存目录，并校验清单、成对二进制与运行输入快照，恢复最近的完整版本后再后台构建当前源码。按 `Ctrl+C` 停止全部子进程；若 `xtask` 自身变化，命令以退出码 `75` 提示重新运行 `cargo dev`。

保存后的动作由整批路径共同决定；同一批包含多类变化时会合并为覆盖全部变化的计划：

| 变化范围 | `cargo dev` 动作 |
|---|---|
| 当前开发运行配置 | 不调用 Cargo；先比较去注释、脱敏后的配置快照与内存密钥投影。语义相同的保存保持当前 API/Worker 就绪；存在实际差异时复用上一可用二进制，以原子配置快照探活后成对切换 |
| 本地化 catalog | 重新构建 API 和 Worker，因为默认文案包含编译期资源 |
| API crate 或 API 组合入口 | 只构建 API，复用上一可用 Worker，随后成对探活和切换 |
| Worker 入口 | 只构建 Worker，复用上一可用 API，随后成对探活和切换 |
| application、kernel、auth、config、adapters、DB 或共享 boot | 重新构建 API 和 Worker |
| 控制库迁移或访问目录 | 只构建 migrate，复用上一可用 API/Worker，执行控制库 verify 后成对探活和切换；不自动升级 |
| 租户迁移 | 只构建 migrate，并验证当前配置中明确登记的本地租户目标；不重启服务 |
| migrate 入口 | 只构建 migrate 并独立 verify；不重启服务 |
| 资源清单或生成器 | 执行只读 `cargo resource --all --check`；发现漂移即失败，不重启服务 |
| Cargo、toolchain、`build.rs` 或本地 vendor | 重新规划并构建 API、Worker 和 migrate |
| 未识别的后端文件 | 保守地重新构建 API 和 Worker |
| `xtask` 自身 | 退出码 `75`，提示开发者重新运行 `cargo dev` |

编译、迁移验证或探活期间出现更新的源码代次时，旧周期会终止并回收完整 Cargo、rustc 与 build-script 进程树，再处理最新路径集合。候选版本开始切换后会先完成切换或恢复上一可用版本；期间的新事件进入队列，不会把过期代次提升为正式版本。配置与本地化资源使用原子运行快照，密钥只通过子进程环境传递，不写入代次清单。仅注释、格式等不改变快照语义的配置保存会重新校验文件结构和密钥投影后跳过重启；任一配置值或密钥投影变化仍按完整候选探活与回滚路径处理。

常用排障入口：
```powershell
cargo run --locked -p ryframe --no-default-features --features bin-api,runtime-swagger-ui --bin ryframe -- --probe
cargo run --locked -p ryframe --no-default-features --features bin-worker --bin ryframe-worker -- --probe
```
## 数据库迁移

控制库使用默认目标；租户数据可操作全部已登记目标或单个目标；新迁移必须用对应命令创建骨架：

```powershell
cargo migrate status
cargo migrate verify
cargo migrate up
cargo migrate verify tenant-data --all
cargo migrate verify tenant-data --target <目标键>
cargo migrate new control <迁移名>
cargo migrate new tenant-data <迁移名>
```
生产部署和非生产重建步骤见[数据指南](data.md)与[运维指南](operations.md)。
## 开发标准资源

资源入口默认只预览差异；`--check` 只读比较并在存在差异时返回失败，`--write` 才写入生成结果，`--explain` 可查看从资源清单到页面的调用链：

```powershell
cargo resource --help
cargo resource post
cargo resource post --check
cargo resource --all --check
cargo resource post --write
cargo resource post --explain
```
`cargo resource --all --check` 会校验全部受管后端、前端资产和 ownership 清单，适合在提交前确认重复生成零差异。该命令不会创建临时生成文件、刷新 OpenAPI 或连接数据库；发现差异后，先按资源预览，再显式执行对应的 `--write`。

开发新的标准资源时：

1. 在 `catalog/resources/` 增加或修改资源清单。
2. 预览生成差异，确认字段、校验、筛选、排序和权限。
3. 使用 `--write` 更新后端、OpenAPI 和前端派生文件。
4. 运行 `cargo resource --all --check` 确认资源目录零差异。
5. 在前端补充资源需要的业务交互。
6. 运行 `cargo verify`，再用浏览器验证新增、查询、编辑和删除流程。

Post 和 Notice 可作为标准 CRUD 示例。导出、发布等特殊动作适合保留为自定义强类型用例。
## 开发自定义业务

不能由标准资源表达的流程按以下顺序实现：

1. 在 `ryframe-application::system` 的对应业务域增加用例请求、结果和流程。
2. 若需要数据库操作，在相应 DB 模块实现 Repository；若需要 Redis、对象存储或其他连接，在 adapters 实现端口。
3. 在 `ryframe` 的启动装配中构造并注入实现。
4. 在 `ryframe-api` 增加 DTO、路由和 OpenAPI 描述，或让 Worker 调用应用用例。
5. 增加覆盖业务成功和失败路径的测试。
6. 同步前端契约并联调；模块选择和请求流见[架构说明](architecture.md)。
## API 与前后端联调

接口变化后运行 `cargo api-sync`，从当前后端代码生成候选 OpenAPI 并刷新前端 operation descriptor；随后进入前端项目执行消费者检查和浏览器 smoke，确认请求、权限、菜单与页面行为一致。只重新导出后端快照时运行：
```powershell
cargo run --locked -p ryframe-api --bin export_openapi -- openapi/openapi.json
cargo run --locked -p ryframe-db --features migration --bin export_mysql_snapshot -- sql/ryframe_config.sql
```
## 测试与检查

日常修改使用智能检查，联调完成后使用完整检查：

```powershell
cargo verify
cargo verify --full
```
只运行后端或前端主要检查时可添加 `--scope backend` 或 `--scope frontend`。

Rust 函数长度检查使用固定版本的 tree-sitter AST。当前 rollout 采用 `changed` 模式，只对本次新增或修改触及的函数执行 150/100/80 行分类上限；已登记的存量例外无论是否被本次修改都会校验到期时间与“只能缩短”约束。只有 `architecture/crate-boundaries.toml` 中的函数例外全部清零后，策略才允许切换为 `all`，提前切换会直接使架构检查失败。

运行某个模块的测试：
```powershell
cargo test --locked -p ryframe-application
cargo test --locked -p ryframe-api
```

真实 MySQL 和 Redis 协议测试默认不连接外部服务。准备好隔离的 schema 或 namespace 后显式启用：

```powershell
$env:RYFRAME_MYSQL_INTEGRATION = "1"
$env:RYFRAME_MYSQL_TLS_INTEGRATION = "1"
cargo test --locked -p ryframe-db --test mysql_real_protocol -- --nocapture

$env:RYFRAME_REDIS_INTEGRATION = "1"
cargo test --locked -p ryframe-adapters --test redis_real_protocol -- --nocapture
```

完整集成门禁会验证 MySQL required TLS，以及 Redis TLS、S3 HTTPS 和 OTLP HTTPS 三个 AWS-LC 真实出站测试。Windows 本地需有 `python`、`openssl`，并在 `127.0.0.1` 启动隔离的 MySQL 与 Redis；Redis 建议使用 WSL 独立实例和数据库 15。门禁只接受回环 Redis，不执行 `KEYS`、`SCAN` 或 `FLUSH`，测试 key 由唯一 `scope_id` 隔离并精确删除。运行入口与 CI 相同：

```powershell
$env:RYFRAME_MYSQL_INTEGRATION = "1"
$env:RYFRAME_MYSQL_TLS_INTEGRATION = "1"
$env:RYFRAME_REDIS_INTEGRATION = "1"
$env:RYFRAME_REDIS_DATABASE = "15"
$env:RYFRAME_INTEGRATION_RUN_ID = "local-aws-lc"
cargo xtask ci integration
```

TLS fixture 会生成两日有效的临时 CA，在动态回环端口启动 Redis TLS 代理和 HTTPS 服务；相关测试结束或失败后都会停止监听并删除临时证书。日志默认保存在 `.local-tests/integration/tls/<run-id>/`，历史目录不会覆盖；可用 `RYFRAME_TLS_ARTIFACT_DIR` 指定日志根目录。CI 对成功和失败运行都上传 14 天，失败摘要会输出每个已运行测试的最近日志。`RYFRAME_REDIS_HOST` 不是 `127.0.0.1`、`::1` 或 `localhost` 时门禁直接拒绝启动，避免误连共享 Redis。

## 开发反馈性能测量

需要测量保存反馈、编译、资源门禁或前端构建时，先用 `cargo xtask devex --help` 选择适用的 suite、variant、运行次数与冷暖缓存，再通过统一入口执行和汇总：

```powershell
cargo xtask devex run --suite <suite> --variant <name> --runs <次数> --cache <cold|warm>
cargo xtask devex paired --base-backend <基线目录> --candidate-backend <候选目录> --suite <suite> --variant <name> --runs <次数> --cache <cold|warm>
cargo xtask devex summarize <日期/run-id>
cargo xtask devex compare --base <日期/run-id> --candidate <日期/run-id>
```

测量产物写入 `.local-tests/devex/<日期>/<run-id>/`，其中包含运行环境与源码指纹、逐次样本以及 P50/P95 摘要；涉及编译缓存的 suite 还保存前后统计。runner 会验证 suite 与缓存语义、源码和工具指纹、样本完整性及基线/候选是否可比较；前置依赖未就绪、输入无效、源码在测量中变化、缓存报错或证据缺失时都会失败关闭，不把不完整结果判为达标。`cargo-dev-save` 使用真实 watcher、只读迁移验证和探活流程，不会自动升级数据库。

## 资源门禁

`cargo xtask ci resource-gate --frontend-dir ../ryframe-vue3` 直接从 CI 的 base/head SHA 计算资源变化、关系闭包和 ownership，不接收调用方拼接的资源名。缺少合法 base、变更面过大、删除或重命名无法归属，以及 Cargo、toolchain、模板、CI 或架构策略变化时会自动执行完整门禁。

定向模式当前默认关闭；只有经过独立后端与前端提交回放、证明定向结果与完整门禁一致后，CI 才能使用受控激活标记。激活证据缺失、过期或无法验证时继续完整回退，不会把完整回退误报为定向通过。CI 的 Rust 编译使用 sccache 并保存统计，但不缓存整个 Cargo target；缓存配置或统计异常不得替代真实门禁结果。

## 常见问题

- 启动提示迁移不一致：运行 `cargo migrate status` 和 `cargo migrate verify`，本地确认迁移内容后再执行 `cargo migrate up`。
- Worker 未消费任务：确认 API 与 Worker 都使用 `APP_JOBS_MODE=external`，再检查 Worker 健康端口、lease 和数据库连接。
- 前端请求与后端不一致：重新运行 `cargo api-sync`，再执行前端消费者检查。
- Redis 或对象存储不可用：检查 `scope_id`、连接模式、TLS、ownership marker 和服务端口；详细步骤见[运维指南](operations.md)。
