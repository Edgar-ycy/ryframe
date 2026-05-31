# RyFrame Conventional Commits 规范

本项目遵循 [Conventional Commits 1.0.0](https://www.conventionalcommits.org/) 规范，
所有提交信息均需符合以下格式约定。

## 格式

```
<type>(<scope>): <description>

[body]

[BREAKING CHANGE: ...]
```

### 字段说明

| 字段 | 必填 | 格式 | 说明 |
|------|------|------|------|
| `type` | ✅ | 小写英文 | 变更类型，见下方类型表 |
| `scope` | ❌ | 小写英文 | 变更范围（crate 名、模块名） |
| `description` | ✅ | 中文/英文 | 简短描述（≤72 字符） |
| `body` | ❌ | 多行文本 | 详细说明（每行 ≤72 字符） |
| `BREAKING CHANGE` | ❌ | 固定前缀 | 不兼容变更声明 |

### 规则

- `description` 必须以**非首字符大写**（英文）开始，**不加句号**
- `type` + `scope` + `:` + 空格 + `description` 整行 **≤ 72 字符**
- `body` 每个段落之间空一行
- `BREAKING CHANGE:` 必须放在 body 或 footer 中，后跟变更说明

---

## Type 类型

| Type | 说明 | 影响版本 |
|------|------|----------|
| `feat` | 新功能 | MINOR |
| `fix` | Bug 修复 | PATCH |
| `perf` | 性能优化 | PATCH |
| `refactor` | 重构（不改变功能） | — |
| `style` | 代码风格（格式、空格等） | — |
| `docs` | 文档变更 | — |
| `test` | 添加/修改测试 | — |
| `build` | 构建系统或外部依赖 | PATCH |
| `ci` | CI/CD 流水线变更 | — |
| `chore` | 杂项（更新工具、配置等） | — |
| `revert` | 回滚之前的 commit | PATCH |

> **版本影响**基于 [Semantic Versioning 2.0.0](https://semver.org/)：
> - MINOR = 版本号中间位 +1
> - PATCH = 版本号末位 +1
> - `—` = 不触发版本变更

---

## Scope 范围

范围使用 **crate 名称** 或 **功能模块**：

| Scope | 对应路径 | 说明 |
|-------|----------|------|
| `common` | `crates/ryframe-common/` | 通用基础库 |
| `config` | `crates/ryframe-config/` | 配置管理 |
| `core` | `crates/ryframe-core/` | 核心抽象 |
| `db` | `crates/ryframe-db/` | 数据访问层 |
| `service` | `crates/ryframe-service/` | 业务逻辑 |
| `auth` | `crates/ryframe-auth/` | 认证授权 |
| `middleware` | `crates/ryframe-middleware/` | 中间件 |
| `api` | `crates/ryframe-api/` | API 接入层 |
| `task` | `crates/ryframe-task/` | 定时任务 |
| `generator` | `crates/ryframe-generator/` | 代码生成器 |
| `monitor` | `crates/ryframe-monitor/` | 监控 |
| `macro` | `crates/ryframe-macro/` | 过程宏 |
| `app` | `crates/ryframe/` | 主应用入口 |
| `deploy` | `deploy/` | 部署配置 |
| `deps` | `Cargo.toml` | 依赖变更 |
| `workspace` | 根目录 | 工作区级别变更 |

> 跨 crate 的变更使用 `workspace` 或不写 scope。

---

## BREAKING CHANGE

不兼容变更必须标记，并说明迁移方式：

```
feat(auth)!: 重构 JWT 令牌格式，支持 refresh token

新增 refresh_token 字段，access_token 过期时间从 24h 缩短为 2h。

BREAKING CHANGE: `Claims` 结构新增 `token_type` 字段，
所有依赖 JWT 解码的代码需要更新。迁移方式：
将 `jwt::decode::<Claims>` 改为 `jwt::decode::<NewClaims>`。
```

> 在类型后加 `!` 表示 BREAKING CHANGE（如 `feat!:`），与 footer 中的 `BREAKING CHANGE:` 等效。

---

## 示例

### ✅ 正确示例

```
feat(auth): 添加登录失败锁定机制

连续5次登录失败后锁定账号15分钟。
通过 auth_config.max_login_attempts 和 lockout_duration_minutes 配置。

Closes #42
```

```
fix(middleware): 修复限流在 Redis 断开时 panic

使用 fallback 策略，Redis 不可用时自动降级为内存限流。
同时添加连接重试逻辑，3次重试后永久切换。
```

```
refactor(core): 统一缓存 trait 为 async_trait 模式

将 Cache trait 的方法签名从同步改为 async，
消除内部 tokio::spawn 的开销。
```

```
perf(db): 批量查询优化，N+1 问题修复

- 用户列表查询从 N+1 改为 JOIN 一次查询
- 部门树查询添加递归 CTE
- 基准：1000用户列表从 350ms → 45ms
```

```
ci(workspace): 添加 cargo audit 安全扫描到 CI 流水线

在 push 和 PR 时自动运行 cargo audit，
阻止包含已知漏洞的依赖进入主分支。
```

```
docs(api): 补充 OpenAPI 文档注释

为 AuthController 所有端点添加 utoipa 文档注释，
包括请求体 schema 和响应示例。
```

```
chore(deps): 升级 rand 0.9 → 0.10

适配 aes-gcm API 变更：
- rand::rng() 替代 rand::thread_rng()
- fill_bytes() 替代 fill()
```

### ❌ 错误示例

| 错误写法 | 原因 |
|----------|------|
| `feat(auth): 添加登录失败锁定机制.` | description 不能以句号结尾 |
| `Fix Bug` | 缺少 scope 且首字母大写 |
| `feat:` | 缺少 description |
| `feat(auth):添加登录失败锁定机制` | `:` 后缺少空格 |
| `更新代码` | 缺少 type |

---

## 工具集成

### Commitlint（推荐）

```bash
# 安装 commitlint
npm install -g @commitlint/cli @commitlint/config-conventional

# commitlint.config.js
module.exports = {
  extends: ['@commitlint/config-conventional'],
  rules: {
    'type-enum': [2, 'always', [
      'feat', 'fix', 'perf', 'refactor', 'style',
      'docs', 'test', 'build', 'ci', 'chore', 'revert'
    ]],
    'scope-enum': [2, 'always', [
      'common', 'config', 'core', 'db', 'service', 'auth',
      'middleware', 'api', 'task', 'generator', 'monitor',
      'macro', 'app', 'deploy', 'deps', 'workspace'
    ]],
    'header-max-length': [2, 'always', 72],
    'body-max-line-length': [2, 'always', 72],
  },
};
```

### Git Hooks

项目已配置 pre-commit（[.pre-commit-config.yaml](.pre-commit-config.yaml)），
自动执行 `fmt` + `clippy` + `check`。建议同时配置 commit-msg hook：

```yaml
# .pre-commit-config.yaml 追加
  - repo: https://github.com/compilerla/conventional-pre-commit
    rev: v3.4.0
    hooks:
      - id: conventional-pre-commit
        stages: [commit-msg]
        args: [] # 使用默认检测规则
```

### 自动生成 CHANGELOG

使用 [git-cliff](https://github.com/orhun/git-cliff) 从 commit 历史自动生成变更日志：

```bash
# 安装
cargo install git-cliff

# 生成 CHANGELOG
git cliff -o CHANGELOG.md
```

`cliff.toml` 示例配置：
```toml
[changelog]
header = "# Changelog\n"
body = """
{% for group, commits in commits | group_by(attribute="group") %}
### {{ group | upper_first }}
{% for commit in commits %}
- {{ commit.message | split(pat="\n") | first | trim }}
{%- endfor %}
{% endfor %}
"""
```

---

## GitHub PR 标题规范

PR 标题也应遵循 Conventional Commits 格式。
合并时使用 **Squash Merge**，PR 标题作为最终 commit 信息：

```
feat(auth): 添加登录失败锁定机制 (#42)
```

> PR 编号自动追加到标题末尾。

---

## 快速参考

```
# 新功能
git commit -m "feat(scope): 简短描述"

# Bug 修复
git commit -m "fix(scope): 简短描述"

# 不兼容变更
git commit -m "feat(scope)!: 简短描述" -m "BREAKING CHANGE: 变更说明"

# 多行详细说明
git commit -m "feat(scope): 简短描述" -m "详细说明段落1" -m "" -m "详细说明段落2"

# 文档变更
git commit -m "docs(scope): 更新 API 文档"

# CI/CD 变更
git commit -m "ci(workspace): 添加 audit 安全扫描"
```
