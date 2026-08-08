# 对象存储与 RustFS 指南

> 最后核对：2026-07-23

## 1. 架构边界

对象存储是正式基础设施能力，不是上传模块里的工具函数：

| 位置 | 职责 |
| --- | --- |
| `crates/ryframe-storage` | `ObjectStorage` 端口、本地实现、S3 兼容实现、SigV4 和路径安全 |
| `crates/ryframe/src/boot/storage.rs` | 根据配置选择实现，启动时验证连接、凭据和业务 bucket |
| `crates/ryframe-service/src/system/file_service.rs` | 文件校验、上传编排与下载 |
| `crates/ryframe-service/src/system/file_service/upload_reservation.rs` | 配额预留、租约心跳、状态提交与失败补偿 |
| `crates/ryframe-db` | 只持久化 `sys_file` 元数据，不依赖存储实现 |
| `crates/ryframe-api` | multipart/下载 HTTP 适配，不持有对象存储客户端 |

支持 `local`、`rustfs`、`minio` 和 `s3` 四个配置后端。RustFS、MinIO 和 S3 共用经过测试的 S3 兼容适配器；`rustfs` 作为独立枚举值进入配置、日志、运行时监控和 CI，不再靠通用 `s3` 名称隐式表达。

应用启动时确保 `uploads` 和 `avatar` 两个私有 bucket 存在。端点不可达、凭据错误、bucket 检查失败，或发现匿名 `Principal`、`NotPrincipal` 等公开 bucket policy，都会阻止服务启动，不允许静默退回本地目录。

## 2. 本机启动 RustFS

开发配置 `config/app.dev.toml` 默认连接 `http://localhost:9000`。与 CI 一致的 Docker 启动方式：

```bash
docker run -d --name ryframe-rustfs \
  -p 9000:9000 \
  -p 9001:9001 \
  -e RUSTFS_ACCESS_KEY=rustfsadmin1 \
  -e RUSTFS_SECRET_KEY=rustfsadmin1 \
  -v ryframe-rustfs-data:/data \
  rustfs/rustfs:1.0.0-beta.8
```

- S3 API：`http://localhost:9000`
- 管理控制台：`http://localhost:9001`
- 开发账号：`rustfsadmin1`
- 开发密码：`rustfsadmin1`

检查容器和端口：

```bash
docker ps --filter name=ryframe-rustfs
curl http://localhost:9000/
```

启动后直接运行 `cargo run`。不需要手工创建 bucket，组合根会创建或验证它们。开发凭据只能用于本机；生产环境必须更换账号、启用 TLS、限制网络访问并从部署系统注入密钥。

## 3. 配置

```toml
[object_storage]
backend = "rustfs"
endpoint = "http://localhost:9000"
access_key = "rustfsadmin1"
secret_key = "rustfsadmin1"
use_ssl = false
region = "us-east-1"
```

生产部署可使用以下环境变量，不要把凭据写入仓库：

```text
APP_OBJECT_STORAGE_BACKEND
APP_OBJECT_STORAGE_ENDPOINT
APP_OBJECT_STORAGE_ACCESS_KEY_FILE
APP_OBJECT_STORAGE_SECRET_KEY_FILE
APP_OBJECT_STORAGE_USE_SSL
APP_OBJECT_STORAGE_REGION
```

两个 `_FILE` 变量分别指向只包含访问密钥和私密密钥的 Docker secret 文件；不要把凭据直接写入 Compose 环境或仓库。

v0.5 的上传响应只返回受认证的后端下载地址，bucket、普通文件和头像始终保持私有，不提供公共 URL 配置。前端展示头像时必须通过统一 HTTP 客户端携带 access token 下载 Blob，再创建浏览器对象 URL，不能把受保护地址直接绑定到原生 `img.src`。

切换成本地存储时使用：

```toml
[object_storage]
backend = "local"
local_base_dir = "uploads"
```

生产环境默认拒绝 `local`。只有单实例，或所有实例挂载同一个且已验证锁、原子重命名和
read-after-write 语义的共享持久卷时，才能显式设置：

```text
APP_OBJECT_STORAGE_ALLOW_LOCAL_IN_PRODUCTION=true
APP_OBJECT_STORAGE_LOCAL_BASE_DIR=/var/lib/ryframe/uploads
```

该开关只是风险确认，不会自动创建共享卷或备份。容器重建、滚动部署和跨节点读取都必须
看到同一对象集合；无法证明时使用 RustFS/MinIO/S3。生产远程对象存储必须同时设置
`APP_OBJECT_STORAGE_USE_SSL=true` 和 `APP_OBJECT_STORAGE_ENDPOINT=https://...`；显式
`http://` endpoint 即使 `use_ssl` 为 true 也会在启动时被拒绝。容器还必须信任服务端
证书链。

## 4. 运行时检查

受保护接口 `GET /api/v1/monitor/runtime` 返回对象存储后端、端点和动态连接状态。`connected` 会实际检查 `uploads` 与 `avatar`，不是静态配置回显。

文件链路通过以下接口进入同一个 `FileService`：

- `POST /api/v1/common/upload`
- `POST /api/v1/common/upload/image`
- `POST /api/v1/common/upload/avatar`
- `GET /api/v1/common/file/download`

上传使用持久化状态机保证对象和元数据的最终一致性：

1. 短数据库事务锁定租户行，在同一个锁内完成内容去重、存储配额检查和 `pending` 预留，然后提交事务。
2. 对象存储 `PUT` 在事务提交后执行，不占用数据库连接或租户行锁；长上传每 30 秒续租一次。
3. `PUT` 成功后，以预留令牌执行 compare-and-set，将元数据从 `pending` 切换为 `ready`。只有 `ready` 文件能被列表、去重快速路径和下载查询看见。
4. 失败或请求取消时，进程内 guard 会尽快将预留切换为 `cleanup` 并删除对象；即使进程直接退出，持久化预留也会由启动后常驻的全局 janitor 限批回收，不依赖该租户再次上传。

`pending` 会计入租户配额，因此并发上传不能重复使用同一份剩余额度。相同内容的并发请求由租户行锁串行预留，不会生成两条有效元数据。数据库提交响应丢失时，Service 会从主库复核预留或 `ready` 状态；重试若发现对象已写入，会校验长度和 SHA-256 后完成原预留。

`pending`、`ready` 和 `cleanup` 都使用 `del_flag = '0'`，文件可见性只由 `upload_status` 判定；`del_flag = '2'` 专用于软删除，不能复用。配额统计覆盖所有 `del_flag = '0'` 的状态，避免预留和待清理对象漏计。升级期间不运行新旧版本混部，必须先执行文件维护命令并完成终态迁移，再启动新版本。

过期回收分两阶段进行：第一轮只把 `pending` 改成带新截止时间的 `cleanup` tombstone；grace 到期后，第二轮再次删除对象并硬删除 tombstone。这样即使被取消的远端 `PUT` 延迟完成，也会被第二次删除覆盖。初始预留、续期、过期判断和 grace 起点都使用同一主数据库 UTC 时钟，避免租户锁等待或多应用节点时钟偏差误删活跃上传。janitor 每批最多处理 32 条，任务失败从 5 秒开始指数退避到 5 分钟；单个对象删除失败会把该 tombstone 延后 60 秒，让后续记录能进入下一批，避免固定失败项饿死队列。对象删除不进入正常上传请求链路。

cleanup grace 至少为 5 分钟，并且不小于存储实现声明的“取消后最晚提交时间”两倍。生产 S3 客户端单请求超时为 30 秒；新增对象存储实现必须通过 `late_put_completion_bound` 声明更大的上界。下载会同时校验租户文件元数据，不能仅凭对象路径跨租户读取。

头像上传完成后，`sys_user.avatar_file_id` 会保存稳定的 `sys_file` 关联；历史仅保存 URL 的头像保留空关联，下一次更新后才进入可计数回收链路。替换头像时，旧文件只有在没有任何有效用户引用时才会变为 5 分钟的 `cleanup` 墓碑。宽限期内的并发去重复用会恢复该文件，因此不会因并发头像更新误删对象。

普通文件上限为 10 MiB，头像上限为 5 MiB，上传超时为 120 秒。Nginx、Axum 请求体、multipart 和业务校验使用同一配置，固定长度或 chunked 超限都返回 `413`。

## 5. 构建与静态门禁

托管后端 CI 不启动 RustFS，也不运行测试或验收脚本，避免依赖外部运行环境。对象存储变更必须通过生产目标 Clippy、配置校验、OpenAPI/MySQL 快照一致性检查和本地 Docker 链路验收；维护者的本地验证资产不纳入 Git。

## 6. 二次开发规则

- 新业务只依赖 `Arc<dyn ObjectStorage>` 或拥有它的领域 Service，不直接构造 RustFS/S3 客户端。
- bucket 必须是明确的业务常量；对象 key 必须保持相对路径，禁止 `..`、反斜杠和空路径段。
- 对象与数据库同时变化时，由 Service 定义补偿或事务外一致性策略。
- 新后端必须在组合根注册、配置校验、运行时监控和静态门禁中同时闭环。
- 不在 Repository 生成公开 URL，不在 Handler 读取对象存储配置。
