# 架构

## 依赖原则

RyFrame 固定为 12 个 workspace package。所有依赖必须是有向无环图，`architecture/crate-boundaries.toml` 是机器门禁；新增 crate 必须替换现有 crate 并重新确认边界。

```text
kernel {}
macro {}
config { kernel }
auth { kernel }
application { kernel, auth }

adapters { kernel, config, application }
db { kernel, config, macro, application }
tenant-db { kernel, config, db, application }

api { kernel, auth, application, macro }
ryframe { kernel, config, auth, application, adapters, db, tenant-db, api }

generator { kernel, config, db, tenant-db }
xtask { kernel, config, db, tenant-db, generator }
```

白名单表示允许上限，crate 可以少依赖，但不能越过方向或形成反向边。

## Crate 职责

| Crate | 唯一职责 |
|---|---|
| `ryframe-kernel` | ID、分页、错误和值对象 |
| `ryframe-config` | 配置结构、加载、环境覆盖和校验 |
| `ryframe-auth` | 密码、JWT、RBAC 决策与安全类型 |
| `ryframe-application` | 用例、授权、事务边界、业务端口与状态机 |
| `ryframe-adapters` | Redis、对象存储、表格、限流、本地化和遥测等非 SQL 实现 |
| `ryframe-db` | 控制库实体、Repository、应用端口实现和控制库 baseline |
| `ryframe-tenant-db` | 目标注册、placement、fence、session、租户 Repository 和 tenant baseline |
| `ryframe-api` | Axum、OpenAPI、DTO、路由、extractor 与 HTTP middleware |
| `ryframe-macro` | proc-macro |
| `ryframe` | API、Worker、迁移、重建的组合根与依赖装配 |
| `ryframe-generator` | 离线代码生成 CLI 与模板 |
| `xtask` | 架构、契约、生成和发布检查 |

## 运行边界

HTTP 请求先在 API 层解析为严格 DTO，再调用 application 用例。用例只依赖端口；控制库和租户库分别在 DB crate 实现，Redis、存储和表格能力在 adapters 实现，组合根负责注入。

事务由 application 用例开始和提交。Repository 不自行提交单次 CRUD，也不向 application 暴露 SeaORM 事务、实体或查询构造器。

权限、菜单、页面键和 capability 的唯一事实源是 `catalog/access.toml`。构建脚本生成类型化目录、OpenAPI 扩展和迁移种子；每条路由必须显式声明 `Public`、`Authenticated`、`Permission` 或 `Capability`。

## 模块尺度

手写生产源码默认不超过 1000 行。复杂模块按连接、placement、fence、migration、cleanup、session、metrics 等职责拆分；拆分优先使用 crate 内模块，不为薄抽象增加 crate。

## 变更规则

- 先定义 application-owned 值对象和端口，再移动实现并翻转依赖。
- 反向依赖必须在同一可编译提交中原子切换。
- 不保留旧 crate 名、alias、兼容 re-export、双读或旧任务 decoder。
- 共享服务只显式 `Arc::clone`；普通参数优先借用或移动所有权。
