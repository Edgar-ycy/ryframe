# 数据说明

控制库和租户库的迁移由后端迁移二进制负责。普通检查和预览不得修改数据库；生产数据操作必须使用隔离目标、明确计划和 ownership 核验。

## 迁移命令

```powershell
cargo xtask data migrate status
cargo xtask data migrate verify
cargo xtask data migrate up
```

`cargo xtask data migrate` 会把参数转发给 `ryframe-migrate`。未知参数或未知操作应直接失败，不通过重试掩盖状态不明。

## 数据安全

备份、恢复和复制只能使用已登记的绝对路径及隔离资源。禁止扫描共享数据库、使用模糊匹配删除资源，或在未知写入结果下重放操作。密钥只通过本机密钥管理或忽略目录提供。
