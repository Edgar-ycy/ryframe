# Changelog

## [Unreleased]

## [v0.4.0] - 2026-07-17

### Added

- 新增显式 `ActorContext`，统一 Service 的租户、操作者和数据范围输入，并为代码生成器增加对应模板契约
- 新增租户生命周期、在线会话租户隔离、缓存策略、LRU 淘汰、空值保护和并发击穿保护测试
- 新增独立 `ryframe-storage` crate，集中对象存储端口、本地/RustFS/MinIO/S3 后端、路径验证和 SigV4 签名
- 新增对象存储安全路径、URL 编码、签名确定性，以及文件元数据失败补偿测试
- 新增 `DatabaseCluster` 主库/命名只读副本拓扑、无副本回退、原子轮询，以及三个独立数据库的真实读写路由测试
- 新增命名业务数据源配置与显式解析，恢复本机 `ryframe_device`，并让代码生成器从该库读取表结构
- 新增外部 MySQL 数据源测试及 CI `ryframe_device` 端到端生成器验证
- 新增一等 RustFS 配置后端、启动 bucket 校验、动态运行时健康状态和本机运维指南
- 新增验证码挑战、字形和渲染模块测试，覆盖算术符号、UTF-8 布局与非法输入
- 新增 `PrincipalResolver` 和 `DatabaseMonitor` 窄端口，以及 SeaORM 数据库监控适配器测试
- 新增基于 `syn` 的编译期路由权限目录，发布产物内置完整 `#[perm(...)]` 权限码
- 新增 Redis 游标扫描和批量模式删除封装，生产缓存与在线会话不再调用阻塞式 `KEYS`
- 新增确定性 `export_openapi` 工具和规范 `openapi/openapi.json` 快照，CI 校验差异并上传独立契约产物
- 新增 OpenAPI 契约测试，覆盖 89 条路径、119 个操作、155 个 schema、34 个查询操作、成功响应和写请求体
- 新增 OpenAPI `x-ryframe-menu-routes` 契约；CI 同时校验默认菜单 SQL、route-key 回填迁移和前端页面注册表
- 新增 OpenAPI `x-ryframe-password-policy` 契约和生成式前端策略，CI 校验所有新密码入口使用同一规则
- 新增 MySQL、Redis、RustFS 真实服务运行时 CI job，初始化默认数据后执行健康、认证、权限、菜单、监控、对象上传下载和 OpenAPI 冒烟测试

### Changed

- CI 安全审计改为安装并严格执行 `cargo audit --deny warnings`，移除 Node 20 action 和额外 Checks 写权限依赖
- CI Rust 构建缓存改用官方 `actions/cache@v5` 和按操作系统、架构、job、Cargo 清单隔离的显式缓存键，消除第三方缓存 Action 的 Node 弃用警告
- CI 显式设置 Git 初始化默认分支，消除 `actions/checkout` 创建临时仓库时输出的默认分支 warning 提示
- Release 工作流按触发标签精确提取对应版本说明，保留空的 `Unreleased` 区段且不再发布错误章节
- 统一要求所有 `pnpm` 命令在独立前端目录 `ryframe-vue3` 中执行，并由后端源码门禁拒绝根目录 `.pnpm-store`
- Repository 全面改为显式接收 `tenant_id`；task-local 只保留为 HTTP 请求内的一致性校验
- 数据库配置改为显式 `[database.primary]` 与 `[[database.replicas]]`；命令和一致性敏感读取固定主库，普通查询在只读副本间轮询
- 启动流程只在主库执行迁移，并在接收流量前连接、探活和校验全部已配置副本；副本失败不再被静默跳过
- 将用户 Service/Handler 按命令、查询、角色、密码重置、CRUD 和导入导出拆分，将租户初始化事务下沉到专用 Repository
- 将 `ryframe-core` 缓存实现拆为后端、权限缓存、保护条目、策略、击穿保护和预热子模块，公共导出保持集中
- 在线用户、强制退出和会话黑名单统一使用租户作用域键；密码与 `auth_version` 在同一事务内更新
- 文件公开 URL 的选择从 Repository 移入 `FileService`；应用组合根显式构造存储后端，无效 RustFS/S3 配置不再静默降级为本地存储
- 角色权限和数据范围改为资源化整体替换接口，数据范围字段与部门关联在同一事务内更新
- 用户资料、角色和状态接口按职责拆分；创建用户与初始角色同事务提交，用户角色替换统一为原子 Repository 操作
- 权限类型改为可序列化枚举，API 对非法权限类型和用户状态执行严格校验
- 验证码实现按挑战、字形和图像渲染拆分，公共模块不再承载单一超大实现文件
- 认证中间件将主体解析委托给 `AuthService`；监控模块通过组合根注入数据库监控端口，`ryframe-auth` 与 `ryframe-monitor` 不再依赖 `ryframe-db` 或 SeaORM
- `AuthService` 按会话、身份授权、主体解析和暴力破解防护拆分；登录、刷新和当前用户接口复用同一授权装载流程
- 权限模型、树构建和用例编排拆分为独立模块；权限同步由 API 显式传入编译期目录，不再在运行时读取 Rust 源码
- 菜单按模型与层级校验拆分，部门按 command/query/model 拆分；菜单类型改为 `M/C/F` 强类型枚举
- 配置缓存清理由 `ConfigService` 负责，Handler 不再直接操作 Redis；部门引用检查下沉到 Repository
- 稳定 Service 输出、监控状态、文件上传和 multipart 表单全部进入 OpenAPI 组件；分页查询宏统一生成 `IntoParams`
- JSON 响应中的 Snowflake ID 统一序列化为字符串，前端可直接从规范快照生成查询、请求体和响应类型
- 分页查询宏同时生成 `ListQuery` 和 `FilterQuery`，列表、全量与导出统一映射到命名 Service 查询参数
- 菜单分页下沉到 Repository，代码生成器表筛选与分页移入 Service，Handler 不再加载全量集合后切片
- 登录后修改密码、重置密码和租户管理员初始密码统一使用 8-72 位可见 ASCII 强密码策略；个人修改密码会递增 `auth_version` 使旧会话失效

### Fixed

- 修复完整 SQL 已创建菜单索引但迁移历史为空时 MySQL 启动迁移重复创建索引的问题，并增加预置索引回归测试
- 修复 Rust 1.97.1 检出的限流键格式化冗余借用，并为 `proc-macro-error2 2.0.1` 应用可审计补丁以消除 future incompatibility 警告
- 升级 `crossbeam-epoch`、`calamine`/`quick-xml` 和 `spin`，修复新披露的内存安全与 XML 拒绝服务漏洞并移除撤回版本警告
- 修复非字符串值经过缓存策略后无法回读、本地缓存容量不生效以及过期键仍被 `exists`/`keys` 返回的问题
- 修复 SeaORM 非自增租户主键更新未持久化、角色分配 N+1 查询和密码重置前后端租户契约不一致的问题
- 修复本地对象键目录穿越风险、S3 region 未生效和签名时间可能不一致的问题；文件元数据写入失败时补偿删除已上传对象
- 修复验证码减号缺失、乘号 UTF-8 宽度计算错误和空尺寸输入可能触发 panic 的问题
- 修复部门/菜单写入后异步失效缓存导致的短暂脏读、菜单 `route_key` 校验值与持久化值不一致的问题
- 修复验证码 Redis 取值与删除非原子导致同一答案可能并发复用，以及缓存写入失败被静默忽略的问题
- 修复公告创建人、个人资料部门和上传文件 ID 在 OpenAPI 中被建模为 JavaScript `number` 的契约偏差
- 修复字典、岗位、角色和公告筛选在 `/all` 或导出链路中被忽略，以及全量操作错误暴露分页参数的问题
- 修复菜单和代码生成器在 HTTP Handler 中执行内存分页、导致分层职责泄漏的问题
- 修复个人中心、密码重置和租户创建分别维护不同密码规则，以及修改密码后旧令牌仍可继续使用的问题
- 修复部署冒烟脚本仍访问旧 `/system/permissions/tree`，并使数据库初始化发生 SQL 或默认账号更新错误时返回失败状态
- 修复预期的 4xx 认证/权限响应被遥测记为 warning、导致运行时零告警门禁误报的问题；5xx 继续按 error 记录

### Security

- 验证码答案不再写入日志；操作日志递归脱敏密码、token、验证码和客户端密钥等字段
- 用户和租户 Service 集中阻止自禁用、自删除、超级管理员修改与越权授予超级角色
- RustFS/MinIO/S3 后端默认保持 bucket 私有，不再在初始化时自动写入公开读取策略
- 新密码统一要求大小写字母、数字和特殊字符，拒绝空格、控制字符和前后端长度语义不一致的非 ASCII 输入

### Validation

- 源码卫生、权限路由、架构边界、`cargo fmt`、全 workspace `check`/`clippy -D warnings` 全部通过
- 全 workspace 测试与 `RUSTDOCFLAGS=-Dwarnings` 文档测试通过；仅保留明确白名单的外部 MySQL 与 RustFS/S3 集成测试
- `cargo llvm-cov --workspace --fail-under-lines 55` 通过，行覆盖率为 69.17%
- `cargo audit --no-fetch --deny warnings` 与 `cargo deny check licenses bans sources` 本地检查通过；CI 联网获取最新 advisory 且将警告视为错误

---

## [v0.3.1] - 2026-07-15

### Added

- 新增源码卫生和架构边界检查，覆盖 workspace 依赖、Handler/Service 分层、路由宏和 OpenAPI 注册
- 新增 OpenAPI 路径覆盖、Canonical 路径和唯一 `operationId` 测试
- 新增代码生成器输出路径安全、重复文件、写入报告、Rust 语法和 Golden Hash 契约测试
- 恢复并补齐 Repository 租户隔离、更新持久化和关联关系测试
- 将 API 与过程宏示例改为可编译文档测试，源码门禁禁止新增 ignored doctest

### Fixed

- 修复 SeaORM 更新模型未重置 ActiveValue 导致字段未写入的问题
- 修复用户、角色和字典 Repository 的租户过滤及跨租户访问边界
- 修复数据库表外键和菜单权限同步，完善数据完整性
- 修复无效 ID 被静默丢弃、认证错误状态不准确和 CORS 配置过宽的问题
- 修复 CI 中 Codecov、依赖审计和构建警告被吞掉的问题
- 修复代码生成器混用 MySQL 元数据字段的问题，按 MySQL、PostgreSQL、SQLite 分别读取表、列、约束、自增和注释
- 修复分页默认页为 `0`、部署冒烟脚本仍使用旧路径和 camelCase 参数的问题
- 修复 AES-GCM 配置加密调用已弃用 API 的问题，不再通过 lint 属性隐藏弃用警告
- 修复配置页 `name`/`key` 查询未进入后端过滤、配置值被调试日志明文输出的问题

### Changed

- 统一认证链路为单一 `RequestPrincipal`，一次解析用户、角色、权限和数据范围
- 受保护路由统一通过认证主体执行租户限流，删除请求头驱动的重复租户配额状态
- 拆分公共与受保护监控路由，集中受保护路由策略组装
- 数据库配置改为唯一 `[database.connection]`，启动时连接或迁移失败会直接终止
- API 使用复数资源和统一路径：分页列表为资源根、全量列表为 `/all`，不再保留旧路径别名
- HTTP 64 位 ID 统一序列化为字符串，写入 DTO 默认拒绝未知字段
- Handler 不再导入数据库实体或 Repository，Service 的 Repository 字段改为私有
- 数据库连接在组合根统一注入 Service，Handler 和公开用例方法不再逐次传递连接
- 文件服务持有数据库和对象存储依赖，`AppState` 不再向 HTTP 层暴露对象存储实现
- 无对应 trait 的 `*ServiceImpl` 统一改名为 `*Service`，删除误导性的实现层命名
- 用户、登录日志、操作日志和文件上传改为命名 Command/Query，删除未调用的用户查询方法
- `AppState` 移除原始数据库连接，API 生产依赖不再包含数据库实现；操作日志中间件通过 `OperLogService` 写入
- 分页参数统一为 `page`/`page_size` 并拒绝未知字段，前后端和部署脚本不再接受旧 camelCase 写法
- 监控 OpenAPI 注解归位到真实 Handler，限流器改为公开外观和私有策略；生产 Rust 源码不再包含 `#[allow(...)]`
- 前端分页基类移除任意字段索引，配置和字典查询使用显式契约；后端查询 DTO 统一拒绝未知字段
- 恢复个人中心和代码生成器 API 集成测试，本地 SQLite API 套件不再包含忽略项
- `AppState` 移入独立模块，运行时监控只展示数据库、Redis、对象存储和上传熔断器等真实能力
- 代码生成器改为结构化选项、受限相对输出路径和可审计写入结果；生成 Service 持有数据库依赖，生成 Handler 自动包含路由宏、OpenAPI 注解且不访问 `state.db`
- 公告响应字段统一为 `notice_type`，与查询和写入契约保持一致
- 重构 CI 工作流：全部事件执行源码卫生、架构检查、全 targets check/clippy、测试、文档测试、覆盖率和安全检查
- Rustdoc 警告在 CI 中按错误处理，文档测试不再静默跳过示例
- 将允许的间接依赖重复版本设为显式允许，保留 bans/source 检查且消除 `cargo deny` 非操作性警告

### Removed

- 删除没有生产消费者的事件总线、消息队列、任务队列、gRPC 和硬编码功能开关
- 删除伪多数据源、读写连接选择、重复用户上下文中间件和无效运行时状态
- 删除旧 API 路径别名和未使用的中间产物；前后端不再兼容旧接口写法
- 删除未使用的分页提取器和 OpenAPI 监控占位函数

### Security

- 修复已知依赖漏洞忽略配置（cargo-audit）
- 密码重置改为一次性请求和完成流程，状态或权限变化会使既有会话失效

### Validation

- `python scripts/check_source_hygiene.py`
- `python scripts/check_architecture.py`
- `cargo fmt --all -- --check`
- `python scripts/check_permission_routes.py`
- `cargo check --workspace --all-targets`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo llvm-cov --workspace --fail-under-lines 55`

---

## [v0.3.0] - 2026-07-01

### Added

- 新增租户级请求频率限制能力
- 为租户管理路由接入权限校验中间件
- 为权限编码字段补充数据库索引

### Changed

- 更新项目依赖及许可证校验配置
- 更新开发环境 CORS 配置、API 文档和项目说明
- 更新 CI 使用的 pnpm 版本

### Fixed

- 优化用户令牌失效与权限缓存刷新逻辑

---

## [v0.2.0] - 2026-06-17

### Added

- 权限资源 CRUD
- 接口权限自动扫描和同步
- CI 校验新增接口是否遗漏权限码
- 前端 `v-permission` 支持通配符
- 租户 ID 真正落到实体和查询层
- 角色分配时增加越权防護
- 权限变更后自动刷新菜单和按钮权限

### Fixed

- 修复系统权限页面路由和菜单不可见问题
- 补齐权限与菜单的初始化 SQL
- 修复前端权限变更后的刷新逻辑

### Validation

- `cargo check --workspace --all-targets`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace --lib --tests`
- `cd ryframe-vue3 && pnpm build`
- `vue-tsc -p ryframe-vue3/tsconfig.json --noEmit`

---

## [v0.1.0] - 2026-06-08

### 首个稳定版本

基于 Rust + Axum 的现代化企业级后端框架，提供开箱即用的认证授权、系统管理、监控运维等完整能力。

### Features

- **认证授权**：JWT 登录/刷新/黑名单 + RBAC 权限模型 + 数据权限 DataScope
- **系统管理**：用户 / 角色 / 权限 / 菜单 / 部门 / 岗位 / 参数 / 字典 / 通知 完整 CRUD
- **安全防護**：XSS 过滤、多层限流（全局 / 用户级 / 接口级）、防重放攻击、幂等性、安全响应头
- **缓存体系**：Redis 缓存（配置 / 字典 / 菜单树 / 部门树），读缓存 + 写失效 + 缓存击穿保护，无 Redis 时自动降级内存模式
- **监控运维**：服务器信息、增强健康检查（DB+Redis 连通性）、DB 连接池、在线用户、缓存统计、Prometheus Metrics
- **链路追踪**：OpenTelemetry 分布式追踪（可配置采样率）
- **定时任务**：Cron 调度 + 任务管理 + 执行历史 + 内置清理任务
- **代码生成器**：读取表结构自动生成 Entity / Repository / Service / Handler / DTO 五层 CRUD 代码
- **弹性容错**：重试（指数退避）+ 熔断器 + 降级
- **数据访问**：MySQL / PostgreSQL / SQLite 三数据库支持，多数据源动态切换 + 读写分离
- **配置热加载**：运行时自动检测并应用配置变更
- **消息队列**：Kafka 集成 + 进程内内存队列
- **分布式锁**：Redis 分布式锁
- **事件总线**：进程内异步事件发布 / 订阅
- **多租户**：租户隔离（数据库级 / Schema 级）+ 租户配额
- **gRPC**：Tonic 服务端 / 客户端
- **WebSocket**：WebSocket 连接管理与消息广播
- **对象存储**：本地 / MinIO / S3 多后端动态切换
- **文件处理**：文件上传 / 下载 + 图片压缩 + Excel 导入导出
- **国际化**：i18n 多语言支持（中 / 英文）
- **Swagger UI**：交互式 API 文档

### Architecture

- Cargo Workspace 分层架构，12 个 crate
- 五层分层模型：基础共享 -> 基础设施 -> 领域 -> 接入 -> 入口
- 面向 trait 编程，Service / Repository 可 Mock 测试
- 构造函数注入 + AppState 集中管理依赖

### CI/CD

- GitHub Actions CI（fmt / clippy / test / coverage / security-audit）
- 代码覆盖率上传 Codecov（门禁 50%）
- Nightly 自动发布（push main）
- Stable 版本通过 `v*` tag 手动触发发布

所有值得注意的项目变更都将记录在此文件中。
格式基于 [Keep a Changelog](https://keepachangelog.com/zh-CN/1.0.0/)，
版本号遵循 [语义化版本](https://semver.org/lang/zh-CN/)。
