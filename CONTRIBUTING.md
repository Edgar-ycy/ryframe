# 贡献指南

感谢你对 RyFrame 的关注！本指南将帮助你快速上手项目开发。

## 环境准备

### 系统要求

- **Rust**：stable toolchain（推荐通过 [rustup](https://rustup.rs/) 安装）
- **数据库**：MySQL 8.0+ / PostgreSQL 15+ / SQLite 3（任选其一）
- **Redis**（可选）：缓存、分布式锁、会话管理

### 快速开始

```bash
# 1. 克隆仓库
git clone <repo-url> && cd ryframe

# 2. 安装代码质量工具
rustup component add clippy rustfmt
cargo install cargo-nextest cargo-llvm-cov cargo-audit

# 3. 配置数据库
# 复制并编辑对应环境的配置文件
cp config/app.toml config/app.dev.toml
# 编辑 config/app.dev.toml 中的数据库连接信息

# 4. 初始化数据库
# 导入基础表结构
mysql -u root -p ryframe < sql/ryframe_config.sql
# 或使用迁移工具
cargo run --bin ryframe-migrate -- up

# 5. 启动开发服务器
cargo run
```

服务默认监听 `http://localhost:8080`，默认账号：`admin` / `123456`。

## 项目结构

```
ryframe/
├── crates/                  # 工作区 crate
│   ├── ryframe/             # 启动入口 (bin)
│   ├── ryframe-api/         # HTTP API 层 (handler/dto/router)
│   ├── ryframe-auth/        # 认证授权 (JWT/RBAC/权限)
│   ├── ryframe-common/      # 公共工具 & 错误定义
│   ├── ryframe-config/      # 配置管理 (多环境 TOML)
│   ├── ryframe-core/        # 基础设施 (缓存/事件/队列/锁)
│   ├── ryframe-db/          # 数据访问层 (entities/repositories)
│   ├── ryframe-generator/   # 代码生成器
│   ├── ryframe-macro/       # 过程宏
│   ├── ryframe-middleware/  # 中间件 (限流/XSS/CORS/日志)
│   ├── ryframe-monitor/     # 监控 (健康检查/服务器信息)
│   ├── ryframe-service/     # 业务服务层
│   └── ryframe-task/        # 定时任务
├── config/                  # 配置文件 (dev/prod/test)
├── sql/                     # 数据库初始化脚本
├── locales/                 # 国际化资源 (zh-CN / en-US)
├── examples/                # 示例项目
└── docs/                    # 项目文档
```

## 开发工作流

### 代码规范

- 遵循 Rust 官方命名规范（snake_case 变量/函数，CamelCase 类型）
- 所有公共 API 需添加文档注释（`///`）
- **禁止使用 `unsafe` 代码块**
- 测试代码放在各 crate 的 `tests/` 目录下

### 提交前检查

```bash
# 格式检查
cargo fmt --all -- --check

# Clippy 检查（零警告）
cargo clippy --workspace --all-targets -- -D warnings

# 编译检查（含 tests/benches/examples）
cargo check --workspace --all-targets

# 运行测试
cargo nextest run --workspace

# 文档检查
cargo doc --workspace --no-deps --document-private-items
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

### 结构体/配置字段变更检查清单

当修改结构体定义（新增/删除/重命名字段）或配置结构体时，必须执行以下步骤：

1. **全局搜索构造点**：使用 `cargo check --workspace --all-targets` 编译整个项目
2. **检查以下位置**：
   - `src/` 中的生产代码
   - `tests/` 中的测试代码（测试文件中的结构体初始化也需补全新字段）
   - `benches/` 中的基准测试
   - `examples/` 中的示例代码
3. **配置结构体**：优先为配置结构体实现 `Default` trait，让测试用 `..Default::default()` 自动填充
4. **AutoFill 规则**：新增 `FillSource` 变体时，确保 proc macro 的 `auto_fill.rs` 中 match 分支覆盖完整
5. **API 文档**：数据模型变更后同步更新 `openapi.rs` 和 `docs/api-guide.md`

### Commit 规范

使用 [Conventional Commits](https://www.conventionalcommits.org/) 格式：

```
<type>(<scope>): <description>

feat(auth): 添加 JWT 刷新令牌功能
fix(db): 修复分页查询空结果时的错误
docs(readme): 更新部署文档
test(service): 补全菜单服务单元测试
refactor(core): 重构缓存抽象层
```

常用 type：`feat` `fix` `docs` `test` `refactor` `perf` `chore` `ci`

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
- 软删除使用 `del_flag` 字段（`"0"` = 正常，`"2"` = 已删除）
- 主键使用 UUID v7（`snowflake::next_snowflake_id()`）
- 数据库无关：不写数据库特定 SQL，通过 SeaORM 抽象

## 测试

```bash
# 运行所有测试
cargo nextest run --workspace

# 运行特定 crate 测试
cargo nextest run -p ryframe-service

# 覆盖率报告
cargo llvm-cov --workspace --html
```

测试文件命名：`tests/{module}_test.rs`，对应 `src/{module}.rs`。

## 问题反馈

- 提交 Issue 前请搜索是否已有相关问题
- Bug 报告需包含：环境信息、复现步骤、期望行为
- 功能建议请描述使用场景和预期效果
