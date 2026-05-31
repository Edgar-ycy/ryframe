# 贡献指南

感谢你对 RyFrame 的关注！本文档将帮助你了解如何参与项目开发。

## 行为准则

- 尊重所有贡献者，保持专业和友善的沟通
- 基于技术事实讨论，避免人身攻击
- 接受建设性批评，专注于改进代码质量

## 如何贡献

### 报告 Bug

1. 在 GitHub Issues 中搜索是否已有相同问题
2. 提供清晰的标题和描述
3. 包含以下信息：
   - Rust 版本 (`rustc --version`)
   - 操作系统
   - 复现步骤
   - 期望行为 vs 实际行为
   - 相关日志（脱敏后）

### 提交代码

#### 开发环境

```
# 确保使用 stable toolchain
rustup default stable

# 安装开发工具
rustup component add clippy rustfmt
cargo install cargo-audit cargo-tarpaulin sea-orm-cli

# 启动开发数据库
docker-compose up -d mysql redis

# 编译并运行
cargo run
```

#### 分支策略

```
main          # 稳定分支，仅通过 PR 合并
├── develop   # 开发分支
│   ├── feat/xxx        # 新功能
│   ├── fix/xxx         # Bug 修复
│   ├── refactor/xxx    # 重构
│   ├── docs/xxx        # 文档更新
│   └── chore/xxx       # 工具/依赖
└── release/x.y.z       # 发布分支
```

#### Commit 规范

遵循 [Conventional Commits](https://www.conventionalcommits.org/) 规范。

> 📖 完整规范参考：[COMMIT_CONVENTION.md](COMMIT_CONVENTION.md)

```
<type>(<scope>): <description>

[optional body]

[optional footer]
```

**类型 (type)**：

| Type | 说明 |
|------|------|
| `feat` | 新功能 |
| `fix` | Bug 修复 |
| `refactor` | 代码重构 |
| `docs` | 文档更新 |
| `test` | 测试相关 |
| `chore` | 构建/工具/依赖 |
| `perf` | 性能优化 |
| `style` | 代码风格（不影响逻辑） |
| `ci` | CI/CD 变更 |

**示例**：

```
feat(auth): 添加登录失败锁定机制

连续5次登录失败后锁定账号15分钟。

Closes #42
```

```
fix(middleware): 修复限流中间件在 Redis 断开时的 panic

使用 fallback 策略，Redis 不可用时降级为内存限流。
```

#### 提交前检查清单

- [ ] `cargo check --workspace --all-targets` 零错误
- [ ] `cargo clippy --workspace -- -D warnings` 零警告
- [ ] `cargo fmt --check --all` 格式一致
- [ ] `cargo test --workspace` 所有测试通过
- [ ] 新增代码包含对应单元测试
- [ ] 更新相关文档

#### Pull Request 流程

1. Fork 仓库并创建功能分支
2. 编写代码 + 测试
3. 运行提交前检查清单
4. 提交 PR，描述变更内容和原因
5. 等待 Code Review，根据反馈修改
6. 合并后删除分支

### 代码风格

#### Rust 代码规范

- 遵循 `cargo fmt` 和 `cargo clippy` 规则
- 使用 `rustfmt.toml` 配置（如有）
- 变量命名使用 `snake_case`，类型使用 `PascalCase`
- 优先使用 `&str` 而非 `&String`
- 避免不必要的 `.clone()`
- 异步函数命名不加 `_async` 后缀
- 公共 API 必须包含文档注释 (`///`)

#### 模块分层规范

```
crates/
├── ryframe-common/     # 通用基础（不依赖任何业务 crate）
├── ryframe-config/     # 配置管理（仅依赖 common）
├── ryframe-core/       # 核心抽象（依赖 common + config）
├── ryframe-db/         # 数据访问（依赖 core）
├── ryframe-service/    # 业务逻辑（依赖 db + core）
├── ryframe-auth/       # 认证授权（依赖 core）
├── ryframe-middleware/ # 通用中间件（依赖 core）
├── ryframe-api/        # API 层（依赖 service + auth + middleware）
└── ryframe/            # 入口（依赖所有）
```

**严禁反向依赖**：下层 crate 不得依赖上层 crate。

### 测试规范

- 单元测试放在 `src/` 同级模块（`#[cfg(test)] mod tests`）
- 集成测试放在 `tests/` 目录
- 使用 `mockall` 或手动 Mock trait 进行隔离测试
- 测试覆盖目标：≥ 70%

### 文档规范

- 所有公共 API 需要文档注释
- 复杂逻辑需要行内注释说明意图
- 架构变更需要同步更新 `docs/architecture.md`
- API 变更需要同步更新 `docs/api-guide.md`

## 项目结构

详见 [架构设计文档](docs/architecture.md)。

## 联系方式

- GitHub Issues: 报告 Bug 或提出功能建议
- 讨论: 使用 GitHub Discussions
