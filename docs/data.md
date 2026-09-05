# 数据

## 数据边界

控制库保存身份、授权、产品、租户目录、任务、文件元数据和 placement。租户业务数据按目标路由进入 shared-control 或独立租户库。

控制库与租户库使用独立迁移 ledger：

- 控制库 baseline 由 `ryframe-db` 拥有。
- 租户 baseline 由 `ryframe-tenant-db` 拥有。

当前版本面向全新数据库，不兼容旧数据库结构、旧任务载荷或历史增量迁移。控制库初始化结构已移除服务账号、凭据、委托和访问审计专属表；套餐、租户、任务、配置迁移和数据迁移继续保留。现有数据库与当前基线不符时，启动或 verify 会拒绝，不会自动转换或删除数据。真实生产旧库不要直接部署当前版本，应先制定并验证非破坏升级方案。

本地可使用以下命令查看并校验结构：

```powershell
cargo xtask data migrate status
cargo xtask data migrate verify
cargo xtask data migrate verify tenant-data --all
```

`status` 会以 `missing` 和 `unexpected` 列出缺失及额外的迁移版本；两者均为空且数量一致时才报告 `up_to_date=true`。

确认迁移内容后，使用 `cargo xtask data migrate up` 或 `cargo xtask data migrate up tenant-data --all` 更新对应目标。

## 一致性

授权、任务领取、导出申请、删除受理和配置迁移使用主库与显式事务。仅允许对可接受陈旧的列表读使用 eventual consistency；执行前校验、下载和状态转换使用 strong consistency。

列表排序必须带 ID tie-breaker。批量扫描使用 `id ASC` 游标和固定上界，不使用 offset 扫描大集合。

## 导出快照

导出任务保存版本、规范化筛选、请求指纹、授权指纹、`snapshot_at`、`upper_id`、匹配数和已导出数。申请时新增的上界保证队列等待期间新增记录不会混入结果。

默认限制：50 万行、每批 1000 行、最长 1800 秒、产物最大 512 MiB、每租户最多两个运行中导出。XLSX 使用增量临时文件或流式 sink，不同时保留业务对象、第二份行数组和完整字节缓冲。

## 删除与保留

用户删除先在事务内写 `delete_pending_at`，再在事务外幂等删除对象、独占文件元数据和导出记录。对象不存在视为成功，存储失败保留内部 tombstone 并重试告警。过期清理与用户删除复用同一清理用例。

## 资源作用域

每个部署配置一个稳定且唯一的 `scope_id`：

- Redis key 和 channel 使用 `ryframe:{scope_id}:...`。
- 对象 key 使用 `{scope_id}/...`。
- MySQL、Redis 和对象存储保存并校验 ownership marker。

对象逻辑目录固定为 uploads、avatar、exports、imports 和 config-packages。禁止 `FLUSHDB`、`KEYS`、模糊 bucket 删除或自动扫描数据库服务器。

## 租户路由

租户目标注册、placement、fence、session、migration 和 cleanup 是 `ryframe-tenant-db` 的内部模块。shared-control 目标必须去重，独立目标必须校验物理身份、只读状态和 schema fingerprint。

菜单、权限和能力种子可在 `catalog/access.toml` 查询，配置默认值以配置结构为准。
