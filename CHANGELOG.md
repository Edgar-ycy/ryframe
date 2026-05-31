# Changelog

All notable changes to RyFrame will be documented in this file.

## [0.5.0] - 2026-05-28

### Added
- **弹性容错**：指数退避重试 + 熔断器（`ryframe-core::resilience`）
- **Prometheus Metrics**：HTTP 请求计数/延迟直方图/并发量，`/api/v1/monitor/metrics` 端点
- **Swagger UI**：交互式 API 文档，访问 `/api/v1/swagger-ui`
- **Redis 缓存全面集成**：菜单树/部门树 读缓存 + 写自动失效
- **性能基准测试**：Auth (密码哈希/JWT) + DB (CRUD) benchmark
- **Handler 集成测试**：9 个集成测试覆盖 CRUD/错误/校验场景
- **README.md / CHANGELOG.md**

### Fixed
- Clippy 零警告（collapsible_if、manual_inspect、useless_format 全部修复）
- `cargo doc` 零警告（intra-doc-link 修复）

## [0.4.0] - 2026-05-22

### Added
- 文件上传/下载
- 验证码功能（字母数字 + 数学计算）
- 个人中心（个人信息/密码修改/头像）
- 在线用户管理
- Excel 导入导出

## [0.3.0] - 2026-05

### Added
- 完整 CRUD 覆盖全部系统表
- 中间件管道（限流/CORS/XSS/日志/超时/压缩）
- 操作日志自动记录中间件

## [0.2.0] - 2026-05

### Added
- JWT 认证登录/登出/刷新
- RBAC 权限模型
- 认证 & 权限中间件

## [0.1.0] - 2026-05

### Added
- 项目骨架搭建
- 配置管理（TOML 多环境）
- 数据库连接池 + 事务 + 分页
- Repository/Service 抽象层
- Axum HTTP 服务启动
