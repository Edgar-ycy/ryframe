# 后端运维指南

## 迁移

先执行只读核验，再执行明确的迁移操作：

```powershell
cargo xtask data migrate verify
cargo xtask data migrate up
```

生产数据操作必须使用隔离目标、明确的 ownership 和可复核的计划。禁止对共享数据库执行模糊清理或破坏性重建。

## 构建与检查

```powershell
cargo xtask check --plan
cargo xtask check
cargo xtask build --profile release
```

构建和检查输出只作为当前源码的本地证据；远端 CI、发布和恢复结论必须分别记录真实来源与提交 SHA。

## 故障处理

先保留失败日志和运行目录，确认进程、端口、数据库和对象存储的实际状态，再决定是否重试。未知写入结果不得盲目重放。密钥和环境绑定数据只放在忽略目录或本机密钥管理中。
