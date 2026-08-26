# 架构与扩展位置

本文帮助业务开发者理解一次请求如何经过 RyFrame，并选择合适的扩展位置。

## 运行结构

```text
浏览器 / 客户端
       │ HTTP / WebSocket
       ▼
ryframe-api（路由、DTO、认证提取、OpenAPI）
       │
       ▼
ryframe-application（业务用例、事务、状态机、端口）
       │
       ├── ryframe-db / ryframe-tenant-db（控制库与租户库）
       └── ryframe-adapters（Redis、对象存储、表格、限流、遥测）
       │
       ▼
ryframe（API、Worker、迁移与依赖装配）
```

HTTP 层把请求解析为明确的 DTO，再调用 application 用例。用例通过端口访问数据库、Redis 和对象存储，因此业务流程可以在不依赖具体连接实现的情况下测试。API 与 Worker 使用同一组应用服务。

## 模块定位

| 模块 | 开发时用于 |
|---|---|
| `ryframe-kernel` | 通用 ID、分页、错误和值对象 |
| `ryframe-config` | 配置结构、环境覆盖和校验 |
| `ryframe-auth` | 密码、JWT 与 RBAC 决策 |
| `ryframe-application` | 业务用例、事务、状态机和出站端口 |
| `ryframe-db` | 控制库查询、写入与迁移 |
| `ryframe-tenant-db` | 租户目标路由、查询、写入与迁移 |
| `ryframe-adapters` | Redis、对象存储、表格、限流、本地化和遥测 |
| `ryframe-api` | Axum 路由、DTO、OpenAPI、extractor 和传输中间件 |
| `ryframe` | API、Worker、迁移和重建的启动装配 |
| `ryframe-generator` | 标准资源的离线生成 |

`ryframe-application::system` 按业务分为四个入口：

- `identity`：用户、角色、权限、部门、档案、导入、验证码和 WebSocket ticket；
- `platform`：租户、产品、服务账号和授权诊断；
- `content`：配置、字典、公告、文件、选项和标准内容资源；
- `operations`：消息、导出、审计日志、登录日志、在线用户、监控和保留策略。

查找现有能力时，先从对应业务域的公开服务开始，再进入具体用例。

## 选择开发方式

字段、筛选、排序和普通 CRUD 行为可由资源清单表达时，使用 `cargo resource`。Post 与 Notice 展示了完整链路；生成结果包含后端持久化、应用服务、API、权限资产和前端标准页面。

需要事务编排、外部连接、异步任务或特殊状态机时，使用自定义用例：

1. 在 application 的对应业务域定义请求、结果和业务流程。
2. 需要外部能力时定义端口，在 DB 或 adapters 中实现。
3. 在组合根构造实现并注入应用服务。
4. 在 API 层增加 DTO、路由和 OpenAPI 描述；后台执行则由 Worker 调用同一用例。
5. 同步前端契约并完成联调。

标准资源也可以保留一个强类型扩展，例如 Post 导出或 Notice 消息发布；其余常规 CRUD 继续由资源清单生成。

## 数据与事务

控制库保存身份、授权、租户目录和平台任务；租户业务数据通过目标路由进入 shared-control 或独立租户库。涉及多步写入时，由 application 用例开启并提交事务，同一流程中的 Repository 调用接收同一事务上下文。

列表展示可按场景选择 eventual consistency；权限校验、任务领取、下载和状态转换使用 strong consistency。租户切换和后台任务应继续传递明确的租户与作用域信息。

## 访问控制与契约

路由使用 `Public`、`Authenticated`、`Permission` 或 `Capability` 访问策略。菜单、权限、页面键和 capability 来自 `catalog/access.toml`；业务路由在 API 层关联对应策略。

接口的请求与响应进入 OpenAPI 快照，前端从快照生成 operation descriptor。接口变更后的同步步骤见[开发指南](development.md#api-与前后端联调)。
