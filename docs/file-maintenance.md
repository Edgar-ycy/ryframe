# FILE-A 文件摘要与上传预留单向维护

本手册只适用于从旧 MD5 / `del_flag = '3'` 文件模型切换到 SHA-256 / `upload_status`
模型的维护窗口。命令默认不参与 API、Worker 或普通迁移工具构建，必须显式启用
`file-maintenance` feature。

## 安全边界

- 必须显式设置 `APP_ENV`，并通过 `--database` 同时校验配置数据库名与实际连接数据库名。
- 所有写入操作必须同时使用 `apply` 和
  `--confirm-apply APPLY-FILE-A-MAINTENANCE`。
- `dry-run` 只读取数据库和对象；不会创建存储桶、修改元数据或删除对象。
- 回填逐行校验对象大小与旧 MD5，每条记录只在全部校验通过后以 CAS 写入 SHA-256；任何缺失、
  格式错误、摘要不一致或并发变更都会立即失败。
- 上传预留清理可重入。清理对象时会持有对应元数据行锁；对象已经删除但事务提交失败时，
  再次执行仍可安全收敛。

## 执行顺序

先备份数据库与对象存储，并在维护窗口停止旧版 API 和 Worker。不要在旧版进程仍可创建
`del_flag = '3'` 记录时执行最终清理。

以下命令以 PowerShell 为例，数据库名必须替换为实际配置值：

```powershell
$env:APP_ENV = 'prod'

cargo run -p ryframe --features file-maintenance --bin ryframe-file-maintenance -- `
  backfill-sha256 dry-run --database ryframe_config

cargo run -p ryframe --features file-maintenance --bin ryframe-file-maintenance -- `
  backfill-sha256 apply --database ryframe_config `
  --confirm-apply APPLY-FILE-A-MAINTENANCE
```

回填成功的必要条件是汇总中的 `remaining=0`。随后检查并清理旧上传预留：

```powershell
cargo run -p ryframe --features file-maintenance --bin ryframe-file-maintenance -- `
  drain-legacy-reservations dry-run --database ryframe_config

cargo run -p ryframe --features file-maintenance --bin ryframe-file-maintenance -- `
  drain-legacy-reservations apply --database ryframe_config `
  --confirm-apply APPLY-FILE-A-MAINTENANCE
```

第一次 `apply` 可能只把已过期的 `pending` 记录移入带宽限期的 `cleanup` 状态，并以非零
状态退出。等待输出中的最晚到期时间后原样重试，直至汇总显示 `remaining=0`。这是为防止
取消请求后延迟完成的对象 PUT 被漏删而保留的安全步骤。

`--batch-size` 允许范围为 1–1000，默认 100。`--start-after` 只用于根据日志中的游标恢复
扫描；命令结束前仍会全局检查剩余记录，因此不会把跳过的数据误报为完成。

## 完成判定

只有同时满足以下条件，才可执行最终数据库迁移并部署新运行时：

1. SHA-256 回填 `remaining=0`；
2. 旧上传预留清理 `remaining=0`；
3. 旧 API 与 Worker 已停止，期间没有重新创建旧状态记录；
4. 最终迁移已删除旧 MD5 索引与列、将 SHA-256 收紧为非空，并建立新的摘要索引；
5. 新 API 与 Worker 的生产目标编译、契约快照和文件链路校验全部通过。
