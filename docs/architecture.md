# 后端架构说明

后端由 API、Worker、迁移和数据维护组成。业务用例位于 application 层，数据库访问由 db 与 tenant-db 实现，传输层由 api 负责，`ryframe` 作为组合根装配运行时。

依赖方向、crate 边界、配置事实源和生成约束以 `architecture/crate-boundaries.toml`、Workspace 清单及 `AGENTS.md` 为准。新增能力必须先确认所属边界，再补充实现和针对性检查。

当前 `xtask` 只提供后端开发、检查、构建和迁移转发入口，不承载其他项目的构建、契约或浏览器任务。
