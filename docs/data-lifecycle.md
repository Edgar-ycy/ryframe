# 数据生命周期、异步导入与运维诊断

> 最后核对：2026-08-11

本文说明数据保留、异步用户导入、权限生效诊断和租户运维总览的运行边界。接口字段、状态枚举和权限码仍以 `openapi/openapi.json` 为唯一契约来源。

## 1. 数据保留策略

数据保留采用永久硬删除，不提供在线撤销或归档存储。部署环境如有审计、合规或诉讼保全要求，必须在首次运行前提高相应保留期，并先执行预览确认。

默认策略如下：

| 资源 | 默认保留期 | 自动清理条件 |
| --- | ---: | --- |
| 后台任务 | 30 天 | 仅 `succeeded` 且已经完成 |
| Outbox | 30 天 | 仅 `published` 且已经发布 |
| 定时任务执行历史 | 180 天 | 关联任务已成功或已按成功任务策略清理 |
| 导出任务历史 | 180 天 | 已进入终态 |
| 操作日志 | 180 天 | 超过截止时间 |
| 登录日志 | 180 天 | 超过截止时间 |
| 用户导入历史与异常行 | 180 天 | 导入任务已过期，异常行级联删除 |
| 用户导入源文件和错误报告 | 168 小时 | 导入已进入终态 |
| 数据保留运行记录 | 730 天 | 超过截止时间 |

`pending`、`running`、`dead` 后台任务和未成功发布的 Outbox 不会被自动删除；死信固定永久保留。所有截止时间都使用 MySQL 当前 UTC 时间计算，不以 API 或 Worker 主机时钟为准。

清理按稳定 ID 分批执行，每个资源单次最多处理 `max_rows_per_resource_per_run` 条。达到上限且仍有候选数据时，运行状态为 `partial`，下一次运行继续。不同资源的批次分别提交；某一资源失败不会回滚已经成功提交的其他资源，后台任务会沿用现有退避和重试流程。

### 配置

```toml
[data_retention]
cleanup_batch_size = 500
max_rows_per_resource_per_run = 50000
background_job_succeeded_days = 30
outbox_published_days = 30
schedule_execution_days = 180
export_job_history_days = 180
operation_log_days = 180
login_log_days = 180
user_import_history_days = 180
user_import_artifact_hours = 168
retention_run_days = 730
```

配置支持对应的 `APP_DATA_RETENTION_*` 环境变量。API 与 Worker 必须使用同一组值，修改后同时重启，不支持运行中热切换。

### 运行方式

- 系统租户使用 `GET /api/v1/monitor/retention` 查看生效策略。
- 使用 `POST /api/v1/monitor/retention/preview` 读取截止时间和预计删除数量；预览不修改数据。
- 使用 `POST /api/v1/monitor/retention/run` 人工入队，必须携带 `Idempotency-Key`。
- 使用 `GET /api/v1/monitor/retention/runs` 查看运行记录。
- 默认系统计划每天 `03:30 UTC` 触发 `system.data_retention_cleanup`，升级后从下一个正常时刻开始，不立即删除历史。

保留接口只允许 `system` 租户访问，并且只返回全平台资源汇总，不返回其他租户的任务、日志或用户明细。关闭 Cron 后自动清理停止，人工运行仍通过普通后台任务队列工作。

## 2. 异步用户导入

旧同步接口 `POST /api/v1/system/users/import` 已删除。模板下载仍使用现有用户模板接口；实际导入改为 `/api/v1/system/user-imports` 资源。

默认限制：

```toml
[user_import]
max_file_bytes = 10485760
max_rows = 20000
batch_size = 100
max_active_per_tenant = 1
hash_parallelism = 2
```

创建导入时使用 `multipart/form-data`，只允许一个名为 `file` 的真实 `.xlsx` 文件，并必须提供 `Idempotency-Key`。原始幂等键不会保存，数据库只保存 SHA-256。源文件和错误报告位于内部私有 `imports` bucket，客户端不能指定 bucket 或对象路径。

必须从当前租户下载最新版用户导入模板。第一工作表固定且只允许以下五列，列名和顺序不能改变：

1. 用户名
2. 昵称
3. 邮箱
4. 手机号
5. 部门完整路径

模板的“可用部门”工作表会根据当前申请人的数据范围列出可填写路径，例如 `RyFrame 科技 / 研发部 / 后端组`。模板不包含部门 ID、用户 ID 或其他数据库 ID，导入也不会把纯数字解释为部门 ID。部门路径必须从根部门开始完整填写；路径不存在、对应多个同名层级、部门已停用或超出申请人数据范围时，该行安全失败，不会猜测或选择第一条记录。部门改名、移动、删除或权限变化后，以 Worker 处理该批次时的主库状态为准，应重新下载模板。

导入采用部分成功语义：

- 第一工作表最多 20,000 行，每批默认 100 行。
- 已存在用户名固定跳过，不覆盖任何已有字段。
- 新用户为 `pending_activation`，不自动分配角色，并使用唯一随机激活秘密的 Argon2 哈希。
- 每批开始前重新校验申请人状态、导入权限和数据范围。
- 部门完整路径必须在当前租户唯一命中、处于启用状态并位于申请人可管理范围内。
- 每批的用户、异常行、进度和计数在同一事务中提交。
- 中断或重试从已提交游标继续，不重复处理已经提交的批次。
- 取消只在批次边界生效，已经提交的用户保留。
- 跳过或失败行可以分页查看，并在任务结束后生成私有 Excel 报告。

任务载荷只保存导入任务 ID，不保存 Excel 行、密码、用户资料或对象存储地址。源文件与报告默认在终态后保留 168 小时；任务和异常行默认保留 180 天。

部署顺序必须为“迁移 → 新 Worker → API → 前端”。先升级 Worker 可以保证 API 开始入队 `system.user.import` 时，消费端已经注册对应处理器。Embedded、External 和 `ryframe-worker --once` 共用同一处理器装配。

## 3. 权限生效诊断

权限诊断接口为：

```text
GET /api/v1/system/authorization-diagnostics/users/{id}
```

调用者需要 `system:authorization-diagnostic:list`，并且只能诊断当前租户和自身数据范围内可见的用户。诊断始终从主库重算，展示已分配角色、实际参与授权的角色、有效权限及来源、菜单可见性、最终数据范围、授权纪元、用户授权版本和缓存版本状态。

Redis 或授权缓存不可用不会阻止主库诊断。响应只返回缓存版本，不返回完整缓存快照，也不提供修改权限、提升纪元或清空缓存的操作。WebSocket 字段仅表示当前配置是否具备实时通知能力，不表示目标用户此刻一定在线；响应头授权纪元回退始终保留。

## 4. 租户运维总览

运维总览接口为：

```text
GET /api/v1/monitor/overview
GET /api/v1/monitor/overview/trends?range=6h|24h|7d
```

调用者需要 `monitor:overview:list`。快照包含依赖状态、进程资源、数据库连接池、后台任务和调度状态；趋势包含后台任务创建、调度结果、登录成功/失败以及操作成功/失败。趋势按 MySQL UTC 时间补齐固定时间桶：6 小时使用 15 分钟桶，24 小时使用 1 小时桶，7 天使用 6 小时桶。

数据严格按当前租户聚合。`system` 租户只额外包含 `tenant_id IS NULL` 的平台后台任务，不能借此统计其他普通租户。Redis、对象存储或消息中心不可用时返回降级状态，主库不可用时返回 `503`。总览不返回任务载荷、日志正文、用户名或完整错误文本，也不代理 Prometheus 查询接口。

## 5. 上线检查

1. 在隔离数据库完成全新迁移和 0.9.x 升级迁移。
2. 使用代表性数据对清理和趋势查询执行 `EXPLAIN`，确认命中新索引。
3. 同时向 API 与 Worker 注入一致的 `APP_DATA_RETENTION_*` 和 `APP_USER_IMPORT_*`。
4. 按“迁移 → Worker → API → 前端”部署。
5. 在系统租户先执行数据保留预览，核对截止时间、候选数量和合规要求。
6. 确认默认保留计划只会在下一个 `03:30 UTC` 触发。
7. 验证普通租户不能访问保留接口，所有租户之间的导入、诊断和总览严格隔离。
8. 验证对象存储故障时导入创建返回 `503`，不会回退到未受控临时文件。
9. 验证 Worker 中断、取消和重试后导入从已提交游标继续。
10. 观察后台任务、对象存储和数据库增长，按容量报告调整保留期和导入并发。
