# 数据

## 数据边界

控制库保存身份、授权、产品、租户目录、任务、文件元数据和 placement。租户业务数据按目标路由进入 shared-control 或独立租户库。

控制库与租户库使用独立迁移 ledger：

- 控制库 baseline 由 `ryframe-db` 拥有。
- 租户 baseline 由 `ryframe-tenant-db` 拥有。

声明为 `tenant_data` 的标准资源由生成器同步登记到 `ryframe-tenant-db/src/generated/catalog.rs`，包含复制顺序、租户主键游标、校验列和完整 schema 描述。指纹由当前编译目录和基础设施结构统一计算，初始化与 verify 逐项比对 MySQL 实际结构；未登记、缺失或结构不一致的业务表均拒绝使用。空业务目录合法，控制面资源不会进入租户复制目录。

当前 `0.x` 版本面向全新数据库，只验证当前结构与任务载荷。当前控制库初始化结构已移除服务账号、凭据、委托和访问审计专属表；套餐、租户、任务、配置迁移、数据迁移和备份点继续保留。现有数据库与新基线不符时，启动或 verify 会拒绝，不会自动转换或删除数据。请另建隔离控制库以及共享、独立租户目标进行初始化。

源码中的基线经调整后，使用 `cargo xtask data migrate baseline --write` 显式刷新迁移锁，随后通过开发指南的正式命令导出 MySQL 快照。该操作只更新仓库基线证据，不连接或重建数据库；普通检查、预览和 `cargo xtask dev` 均不会重写基线。版本阶段读取 Workspace 版本，已出现稳定版本的完整 Git 历史和 tag 会阻止通过降版本重新锁定。`v1.0.0` 及之后的已发布迁移只通过新增迁移向前升级。

外部备份登记与恢复演练分别使用 `sys_backup_set`、`sys_backup_resource`、`sys_restore_run`。备份登记会同步原有租户备份点视图，验证通过的恢复演练同步备份点的演练时间。恢复校验使用当前编译期 schema、完整表内容摘要、租户 placement 与对象清单；资源 ownership 和备份登记元数据不参与业务数据摘要，隔离恢复必须保留目标自己的 ownership。

后台任务用单调 `claim_sequence` 关联 `sys_background_job_attempt` 中的每次领取。尝试保存当次可执行时间、领取周期起点、完成时间和闭合结果；人工重试与资源冲突退让不复用序号。租约失效只登记回收时的闭合时间，不虚构 Worker 完成时间；任务按原保留策略删除时，尝试记录随外键一同删除。

本地可使用以下命令查看并校验结构：

```powershell
cargo xtask data migrate status
cargo xtask data migrate verify
cargo xtask data migrate verify tenant-data --all
```

`status` 会以 `missing` 和 `unexpected` 列出缺失及额外的迁移版本；两者均为空且数量一致时才报告 `up_to_date=true`。

确认迁移内容后，使用 `cargo xtask data migrate up` 或 `cargo xtask data migrate up tenant-data --all` 更新对应目标。

独立目标采用按需连接。API 启动后，在“平台管理 → 数据目标”打开指定目标的详情，执行实时连接与 schema 探测；已验证且无占用冲突的目标才会出现在租户开通和迁移的可选项中。列表刷新只读取已知健康状态，不会遍历连接所有数据库。

## 一致性

授权、任务领取、导出申请、删除受理和配置迁移使用主库与显式事务。仅允许对可接受陈旧的列表读使用 eventual consistency；执行前校验、下载和状态转换使用 strong consistency。

列表排序必须带 ID tie-breaker。批量扫描使用 `id ASC` 游标和固定上界，不使用 offset 扫描大集合。

## 开发数据复制与性能 seed

开发复制先用 `cargo xtask check recovery clone plan --input <输入清单> --output <新计划> --write` 固定计划，再用 `cargo xtask check recovery clone verify --plan <计划>` 只读复核。已登记 fresh target 的准备、续作、初始化、复核和只读状态使用 `cargo xtask check recovery fresh-target ...`。输入、证据和运行目录必须位于当前后端 `.local-tests`，只接受已审查表、已登记新目标和完整 ownership；未完成任务、迁移历史、未知生成表、物理引用或未知写入都会失败关闭。来源调度必须先停用、复验并重新导出，离线校验不表示真实资源已就绪。

维护工具使用 `cargo xtask check recovery clone maintenance --operation build --output <新目录> --write` 构建，使用 `--operation verify --output <build.json>` 只读复核。来源通过统一 recovery 的 source 阶段捕获。两者绑定实际 API、Worker、维护二进制、配置、源码和工具指纹；生产者必须停止，数据库关系与对象清单在采集前后保持一致。对象按 HEAD、条件 GET、再次 HEAD 的顺序核验内容和元数据，目标只允许精确 key 的条件创建；失败、竞争或未知结果保留证据，不退回无条件覆盖。

复制统一使用同一私有状态机、运行目录和账本：`cargo xtask check recovery clone init` 登记来源、目标与可复用导出；`clone status` 只读展示最后成功阶段、未收尾状态和下一合法动作；`clone stage` 每次只运行明确阶段。中断后先以 `clone stage --mode reconcile` 对照原账本和完整前后像，再显式 `resume`；死亡控制器只通过绑定创建身份的 `clone recover` 回收，回收本地锁不表示远端写入成功。正式维护入口只展示 `cargo xtask check recovery` 的阶段，不把私有脚本名作为命令契约。

RustFS、Redis 与 API/Worker 的重启分别使用同一运行目录下的 `storage`、`cache` 和 `runtime` 操作，绑定原配置、数据目录、产物、凭据引用、端口与进程创建身份。`status` 始终只读，变更操作要求 `--write`；无法证明归属、阶段不允许、端口被占用或出现未知资源时拒绝接管。缓存只重建登记的 ownership 与验收 sentinel，不恢复会话或锁。

fresh target 的准备、续作、初始化、复核和只读状态统一从 `cargo xtask check recovery fresh-target ...` 进入。xtask 固定当前后端目录，不接受调用方传入其他 `--backend-dir`；`status` 不接受 `--write`，其他阶段仍由私有状态机要求显式 `--write` 并核对登记、前后像与控制器身份。

复制成功后，`post-copy` 依次登记数据集、准备目标、停用调度并复验实际记录和对象。性能 seed 在同一账本中核对十一个明确租户的配额，创建固定身份并再次验证权限；身份开始后不再调整配额。API 与 Worker 的启动和停止使用登记收据，任务处理完成且全部生产者停止后才生成新的 seed 导出。任何阶段成功都不能替代最终业务、数据或正式恢复验证。

产品构建输入与验收工具来源分开记录。产品输入未变时可复核并复用原产物，工具变化仍需运行对应工具测试；正式恢复始终要求精确干净 SHA。Windows 复制阶段会保护已登记的工具和运行二进制，复制中的数据库和对象写入保持串行，对象前像与回读最多四并发。详细参数以各入口的 `--help` 和账本返回的唯一下一动作准，不复制或改写历史 attempt。

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
