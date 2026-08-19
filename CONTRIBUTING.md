# 贡献指南

感谢你对 RyFrame 的关注！本指南将帮助你快速上手项目开发。

## 环境准备

### 系统要求

- **Rust**：由 `rust-toolchain.toml` 固定的 1.97.1（[rustup](https://rustup.rs/) 会自动选择）
- **Python**：3.11+（用于质量门禁和发布校验脚本）
- **数据库**：MySQL 8.0.16+；本地开发和部署演练可使用 Docker
- **Redis**：生产会话、撤销、幂等和分布式锁的强制依赖

### 快速开始

```bash
# 1. 克隆仓库
git clone <repo-url> && cd ryframe

# 2. 安装代码质量工具
rustup component add clippy rustfmt
cargo install cargo-audit


# 3. 编辑 config/app.dev.toml

# 4. 初始化或升级控制库与租户数据面（两者使用独立迁移账本）
cargo run -p ryframe --bin ryframe-migrate -- control up
cargo run -p ryframe --bin ryframe-migrate -- tenant-data up --all

# 5. 启动开发服务器
cargo run -p ryframe --bin ryframe
```

服务默认监听 `http://localhost:8080`，默认账号：`admin` / `123456`。

## 项目结构

```
ryframe/
├── crates/                  # 工作区 crate
│   ├── ryframe/             # 应用、迁移和独立 Worker 入口
│   ├── ryframe-api/         # HTTP API、OpenAPI 与消息 WebSocket
│   ├── ryframe-auth/        # 认证授权 (JWT/RBAC/权限)
│   ├── ryframe-kernel/      # 传输无关领域类型、错误码与主体上下文
│   ├── ryframe-http/        # HTTP 错误映射与统一响应信封
│   ├── ryframe-i18n/        # 语言协商、资源校验与文本渲染
│   ├── ryframe-utils/       # 雪花 ID、脱敏、差异与文件处理工具
│   ├── ryframe-captcha/     # 验证码生成与图像渲染
│   ├── ryframe-excel/       # Excel 导入导出
│   ├── ryframe-config/      # 配置管理 (多环境 TOML)
│   ├── ryframe-core/        # 基础设施 (缓存/Redis/锁/熔断)
│   ├── ryframe-db/          # 数据访问层与控制库迁移
│   ├── ryframe-generator/   # 代码生成器
│   ├── ryframe-macro/       # 过程宏
│   ├── ryframe-middleware/  # 中间件 (限流/CORS/日志/安全响应头)
│   ├── ryframe-monitor/     # 监控 (健康检查/服务器信息)
│   ├── ryframe-service/     # 业务服务层
│   └── ryframe-storage/     # 对象存储端口与本地/S3 实现
├── config/                  # 配置文件 (dev/prod)
├── sql/                     # 由 Migrator 生成和 CI 校验的只读 MySQL 快照
├── locales/                 # 国际化资源 (zh-CN / en-US)
└── docs/                    # 项目文档
```

## 开发工作流

### 代码规范

- 遵循 Rust 官方命名规范（snake_case 变量/函数，CamelCase 类型）
- 所有公共 API 需添加文档注释（`///`）
- 所有新增或修改的说明性注释、文档注释使用中文；协议名、命令、代码示例和必要技术专有名词可保留原样
- **禁止使用 `unsafe` 代码块**
- 生产源码不得包含测试模块、测试属性、doctest 或基准目标

### 提交前检查

```bash
# 格式检查
cargo fmt --all -- --check

# Clippy 检查（零警告）
cargo clippy --workspace --lib --bins -- -D warnings

# 生产代码编译检查
cargo check --workspace --lib --bins

# 文档检查
cargo doc --workspace --no-deps --document-private-items
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

### 结构体/配置字段变更检查清单

当修改结构体定义（新增/删除/重命名字段）或配置结构体时，必须执行以下步骤：

1. **全局搜索构造点**：使用 `cargo check --workspace --lib --bins` 编译生产目标
2. **检查以下位置**：
   - `src/` 中的生产代码
   - `examples/` 中的示例代码
3. **配置结构体**：为可选字段提供明确默认值，并在配置校验中覆盖边界条件
4. **AutoFill 规则**：新增 `FillSource` 变体时，确保 proc macro 的 `auto_fill.rs` 中 match 分支覆盖完整
5. **API 文档**：数据模型变更后同步更新 `openapi.rs` 和 `docs/api-guide.md`

### Commit 规范

使用 [Conventional Commits](https://www.conventionalcommits.org/) 格式：

```
<type>(<scope>): <description>

feat(auth): 添加 JWT 刷新令牌功能
fix(db): 修复分页查询空结果时的错误
docs(readme): 更新部署文档
refactor(core): 重构缓存抽象层
```

常用 type：`feat` `fix` `docs` `refactor` `perf` `chore` `ci`

## 架构约定

### 分层架构

```
Handler → Service → Repository → Database
  ↓         ↓          ↓
 DTO       VO/BO     Entity
```

- **Entity**（`ryframe-db`）：数据库表映射，不对外暴露
- **Repository**（`ryframe-db`）：数据访问封装，通过 `PageQuery` / `PageResult` 统一分页
- **Service**（`ryframe-service`）：业务逻辑编排，返回 VO
- **Handler**（`ryframe-api`）：HTTP 请求处理，参数校验，返回 `ApiResponse`
- **DTO**（`ryframe-api`）：请求/响应数据传输对象

### 关键约定

- 错误统一使用 `AppResult<T>` / `AppError`
- 分页上限 `MAX_PAGE_SIZE = 1000`
- 业务实体通常使用 `del_flag` 字段（`"0"` = 正常，`"2"` = 已删除）；`sys_message_recipient` 为保证每位收件人的独立删除状态，使用 `deleted_at`，所有收件箱查询都必须显式过滤该字段
- 主键使用 Snowflake ID（`snowflake::try_next_snowflake_id()?`），生成失败必须沿 `AppResult` 传播
- 数据库固定为 MySQL；数据库特定语义集中在迁移、Repository 和生成器边界
- 配置只在启动时加载、解密和校验，任何配置变更都要求重启进程

## 本地验证资产

测试、基准和验收资产仅保留在维护者本机的忽略目录中，不纳入 Git、提交或 CI。仓库只保留生产源码、静态门禁和构建验证。

## 问题反馈

- 提交 Issue 前请搜索是否已有相关问题
- Bug 报告需包含：环境信息、复现步骤、期望行为
- 功能建议请描述使用场景和预期效果
