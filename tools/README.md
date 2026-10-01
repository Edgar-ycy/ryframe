# 工具目录说明

RyFrame 的产品运行时使用 Rust 实现。`tools/` 是 Rust workspace 的维护工具层，不是业务运行时目录，也不是用户 API 的实现目录。

所有面向开发者的维护入口统一由 `cargo xtask` 暴露：

- `cargo xtask check`：格式、架构、契约、测试、CI 和安全检查；
- `cargo xtask build`：后端与配套前端构建；
- `cargo xtask generate`：OpenAPI 和标准资源生成；
- `cargo xtask data`：迁移、备份、恢复和明确授权的数据维护；
- `cargo xtask check recovery`：恢复演练、隔离 fixture 和全栈验收；
- `cargo xtask check perf`：性能与资源测量。

目录内的 Python 与 Node 文件只用于以下辅助职责：

- 环境、架构、依赖和部署资产检查；
- Rust 无法直接承担的浏览器、MySQL、Redis、对象存储和进程树验收编排；
- 恢复演练、全栈 fixture、证据账本和 CI 报告处理；
- Rust 生成器调用的受控策略和快照校验。

辅助脚本不得作为新的公开命令使用。新增维护能力应优先加入 `xtask` 的任务图；只有外部协议、浏览器或环境编排确实需要时，才在本目录增加辅助文件。删除文件前必须检查 Rust 调用、CI、动态导入、测试和证据引用。
