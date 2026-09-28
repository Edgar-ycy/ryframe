# 后端开发指南

## 入口

所有后端任务从仓库根目录执行：

```powershell
cargo xtask dev
cargo xtask check
cargo xtask check --full
cargo xtask build
cargo xtask build --profile release
cargo xtask data migrate verify
```

`dev` 启动后端运行程序；`check` 执行 Workspace 全目标检查；`build` 构建 Workspace；`data migrate` 转发迁移操作。`--plan` 只显示将执行的后端 Cargo 命令。

## 代码与验证

后端运行时代码使用 Rust。修改 API、数据库、配置或迁移时，先执行最小相关检查，再执行 `cargo xtask check`。涉及生成文件时，必须使用项目现存的后端生成入口，并核对生成结果与源码提交一致。

文档、配置和数据维护命令的具体约束以 `AGENTS.md` 及[运维指南](operations.md)为准。失败检查不得通过删除测试、降低门禁或隐藏错误来处理。
