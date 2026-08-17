# 租户产品能力与业务数据路由

RyFrame 使用一套 Rust/Vue 构建产物服务所有租户。租户间的业务差异由产品套餐和 Capability
决定，租户业务数据的位置由控制库中的 placement 决定。两者都不是客户端可覆盖的参数。

## 授权与数据边界

受保护请求的最终访问条件为：

```text
部署依赖可用
∩ 租户 Capability 已启用
∩ 用户 RBAC 权限满足
∩ 数据范围满足
∩ 租户业务数据库状态允许
```

超级管理员只绕过普通 RBAC，不绕过 Capability 或业务数据库 fence。JWT、HTTP 参数、
WebSocket 消息和后台任务载荷均不得携带或选择 `target_key`；服务端始终使用签名身份中的
`tenant_id` 从控制库强一致解析 placement。

控制库保存用户、角色、权限、菜单、套餐、Capability、租户目录、平台任务和业务数据放置记录。
未来设备、工单、生产等表属于数据面，只能通过 `TenantDatabaseRouter` 访问。控制库与数据面
之间不建立外键，也不使用跨库事务。

## 产品套餐与 Capability

Capability 处理器只能在 Rust 编译期目录中注册。数据库记录只能引用该目录中的代码、变体和
Schema 版本，不能安装脚本、Vue 路径或任意后端处理器。首个能力为
`system.service_accounts`。

套餐由 `sys_product_plan` 和不可变的已发布版本组成。租户固定绑定一个已发布版本；
`sys_tenant_capability_override` 对单项能力执行完整覆盖，不进行 JSON 深度合并。套餐或覆盖
变化会递增 `sys_tenant.runtime_epoch`。发布和分配会验证：

- Capability、变体和配置 Schema 均已编译并通过 Rust 校验；
- 依赖完整且不存在冲突；
- 当前部署具备该能力要求的基础设施；
- 目标版本已发布且未退役；
- 应用请求的预览哈希和 `runtime_epoch` 仍然有效。

禁用能力不会删除历史菜单、角色关系或业务数据。重新启用时从受控模板补齐缺失资源，只给
`tenant_admin` 增加目录声明的默认管理权限，普通角色不会自动扩权。

租户配置包不包含套餐或能力授权。模块配置仅能应用到已经具备相同 Capability 且 Schema
兼容的目标租户。

## 会话上下文

登录、刷新和 `GET /api/v1/auth/context` 返回同一个原子 `session_context`：

```text
user
roles
permissions
authorization_epoch
runtime_epoch
capabilities
business_data
menus
```

菜单先按 Capability 过滤，再按 RBAC 过滤，并剪除空目录。浏览器不能在上下文读取失败时沿用
旧能力或默认启用功能。

所有受保护响应返回：

```text
X-Authorization-Epoch
X-Tenant-Runtime-Epoch
X-Tenant-Data-Generation
X-Tenant-Data-State
```

WebSocket/Redis 的 `tenant_context_changed` 只负责加速。浏览器发现任一值变化后合并成一次
`/auth/context` 强一致刷新；响应头是最终收敛机制。

## 数据目标配置

数据目标只从启动配置和 Secret 注入读取。平台 API 只公开目标键、模式、区域、健康状态、
Schema 指纹和连接池统计，不返回地址、数据库名、用户名、环境变量名或证书路径。

```toml
[tenant_data]
default_target = "shared-control"
max_open_targets = 32
max_total_connections = 200
idle_pool_secs = 600

[[tenant_data.targets]]
key = "shared-control"
mode = "shared"
kind = "control"

[[tenant_data.targets]]
key = "company-a-db"
display_name = "公司 A 专属业务库"
region = "cn-east"
mode = "dedicated"
kind = "mysql"
host = "mysql.internal"
port = 3306
database = "ryframe_company_a"
username = "ryframe_app"
password_env = "RYFRAME_TENANT_DB_COMPANY_A_PASSWORD"
```

增加或修改目标需要同时重启 API 和 Worker。在已经注册的目标之间迁移租户不需要重启。
`dedicated` 目标最多承载一个活动租户；即使是独立库，所有业务表仍必须直接包含
`tenant_id`。

目标池按需建立。每个进程同时限制打开目标数和连接总预算；同一目标的并发首次访问合并为
一次建池，空闲池按 LRU 回收。目标故障只影响映射到该目标的租户，不改变全局 `/readyz`。

## Router、Session 与 fence

业务用例在开始时调用一次：

```rust,ignore
let session = tenant_database_router.resolve(actor.tenant_id()).await?;
let read = session.select_read(ReadConsistency::Strong).await?;
let transaction = session.begin_write().await?;
```

`TenantDataSession` 固定保存租户、目标、placement generation、目标池和数据状态。事务中途不得
重新路由。

每个目标由独立迁移账本 `seaql_tenant_data_migrations` 管理，并包含
`biz_tenant_fence`。读请求在目标 writer 上校验 active fence 和 generation；写事务首先
`SELECT ... FOR UPDATE` 锁定 fence，并持锁到提交或回滚。目标不可用、Schema 不兼容、
generation 不匹配或 fence 冻结时一律拒绝，不回退到控制库、旧目标或其他租户数据库。

未来数据面表必须满足：

- 表名以 `biz_` 开头并直接包含 `tenant_id`；
- 唯一键和外键都包含租户范围；
- 加入编译期 `TenantDataCatalog`，声明复制顺序、主键游标、摘要列和外键依赖；
- Service 依赖 `TenantDatabaseRouter`，Repository 接受 `TenantDataSession` 或其事务，不能直接
  使用 `ControlDatabaseCluster`。

## 新租户 Saga

创建租户必须提供 `plan_version_id`、`data_target_key`、管理员用户名和密码，并携带 16–128 位可见
ASCII 的 `Idempotency-Key`。原始幂等键和管理员明文密码都不落库；后端以幂等键和非敏感请求字段
计算 64 位 token，并用 Argon2 单独保存创建时的密码验证摘要，二者持久化在
`sys_tenant_provision_request`。同一请求可在 Redis 缓存失效、placement 后续迁移或进程重启后继续
返回成功，不同键、不同非敏感字段或不同密码都会稳定返回 `409`。流程为：

1. 控制库创建 `provisioning` 租户、管理员、套餐分配和 generation 1 placement。
2. 在目标数据库幂等创建 active fence。
3. 按有效 Capability 同步菜单、权限和默认管理员授权。
4. 控制库将 placement 与租户同时切换为 active/enabled。

任一步失败都保留 `provisioning_failed`，不允许登录，并可使用相同输入幂等重试。不得在目标
步骤失败后把租户直接标为 enabled。

## 平台产品 API

- `GET /api/v1/platform/capabilities` 返回编译期 Capability 目录。
- `GET /api/v1/platform/product-plans?page=&page_size=` 返回标准分页信封；
  `GET /api/v1/platform/product-plans/{id}` 返回套餐及完整版本时间线。
- `POST /api/v1/platform/product-plans` 只创建套餐元数据；版本由
  `POST /{id}/versions` 创建，草稿只通过 `PUT /{id}/versions/{version}/draft` 修改。
- 发布与退役分别使用 `POST /{id}/versions/{version}/publish|retire`。已发布版本不可修改。
- 租户上下文、预览与提交分别是
  `/platform/tenants/{id}/product-context`、`product-change-previews` 和 `product-changes`。

所有 `id`、`runtime_epoch`、`preview_runtime_epoch` 均为十进制 JSON 字符串。提交必须回传预览的
`plan_hash` 和 `preview_runtime_epoch`；预览响应包含 capability、菜单、权限的增删改及 warnings。
`overrides` 是完整目标集合，空数组表示清空，仍需 `tenant:capability:override` 权限。

## 停写迁移

迁移使用单租户统一操作租约，固定锁序为 `sys_tenant → sys_tenant_operation_lease → 具体资源`。
同一租户不能同时执行套餐变更、配置包应用、数据迁移或 finalize。

状态机为：

```text
prechecking → queued → quiescing → frozen → copying → verifying
→ cutting_over → activating → succeeded → retention_pending → finalized
```

进入维护窗口后，认证和系统管理仍可用，业务数据接口返回
`423 tenant_data_maintenance`。源 fence 冻结会等待已有写事务完成并阻止新写。复制按
`TenantDataCatalog` 和外键顺序分批进行，每批持久化游标；校验包含逐表行数、主键有序
SHA-256、外键和 Schema 指纹。

切换时先在目标写入 generation `N+1` 的冻结 fence，再原子更新控制库 placement，最后激活
目标 fence。源数据保持冻结只读 168 小时。`cutting_over` 开始后不能取消；目标接受新写后如需
返回原库，必须创建新的反向迁移。

保留期结束且目标已有验证通过的恢复点后，平台管理员才能显式 finalize，并按反向外键顺序
删除源租户数据。

## Schema 与恢复点

生产 API 和 Worker 只验证 Schema，不执行 DDL。部署顺序为：迁移所有活动目标、验证指纹、
发布后端、发布同版本前端。

```text
ryframe-migrate control up|verify|status
ryframe-migrate tenant-data up|verify|status --target <key>
ryframe-migrate tenant-data up|verify|status --all
```

`shared-control` 需要分别执行控制库和 tenant-data 命令，两者使用不同账本。

RyFrame 不创建或恢复 MySQL 备份。数据库平台完成备份后，用纯 Rust 运维命令登记不可解释的
恢复点引用：

```text
ryframe-tenant-data backup-register \
  --target <key> \
  --provider-ref <opaque-ref> \
  --captured-at <utc> \
  --schema-fingerprint <hash> \
  --checksum <sha256> \
  --retention-until <utc> \
  [--expires-at <utc>]
```

`retention-until` 和可选 `expires-at` 必须来自数据库平台的真实保留策略，命令不会猜测或延长
有效期。执行登记即表示操作者已在平台侧完成摘要与可恢复性校验；provider reference 只存控制库，
不会返回浏览器或被 RyFrame 解引用。

专属目标可以登记租户级整库恢复点；共享目标只能登记分片级恢复点，不能宣称可以直接恢复单个
租户。完整灾备必须同时覆盖控制库、业务数据目标和对象存储。

## 稳定错误

| HTTP | `error_key` | 含义 |
| --- | --- | --- |
| 501 | `capability_unavailable` | 部署环境缺少能力依赖 |
| 403 | `tenant_capability_denied` | 租户未开通能力 |
| 403 | `permission_denied` | 用户 RBAC 不满足 |
| 409 | `stale_runtime_epoch` | 产品预览基于旧运行时纪元 |
| 409 | `stale_placement_generation` | 数据迁移基于旧 placement |
| 409 | `tenant_operation_conflict` | 同一租户存在互斥操作 |
| 423 | `tenant_data_maintenance` | 业务数据处于维护窗口 |
| 503 | `tenant_data_target_unavailable` | 目标不可用或 Schema 不兼容 |

可重试的 423 和 503 同时返回 `Retry-After`。
