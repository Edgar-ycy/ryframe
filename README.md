# RyFrame 后端

RyFrame 后端提供 API、Worker、迁移和数据维护能力。前端不属于本仓库的构建或任务入口。

## 常用命令

```powershell
cargo xtask dev
cargo xtask check
cargo xtask build --profile release
cargo xtask data migrate verify
```

`cargo xtask check --plan` 和 `cargo xtask build --plan` 只显示后端命令，不执行任务或写入文件。

## 文档

- [开发指南](docs/development.md)
- [API 说明](docs/api.md)
- [数据说明](docs/data.md)
- [运维指南](docs/operations.md)

详细维护约束见工作区根目录的 `AGENTS.md`。
