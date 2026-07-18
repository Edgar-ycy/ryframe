# 对象存储与 RustFS 指南

> 最后核对：2026-07-18

## 1. 架构边界

对象存储是正式基础设施能力，不是上传模块里的工具函数：

| 位置 | 职责 |
| --- | --- |
| `crates/ryframe-storage` | `ObjectStorage` 端口、本地实现、S3 兼容实现、SigV4 和路径安全 |
| `crates/ryframe/src/boot/storage.rs` | 根据配置选择实现，启动时验证连接、凭据和业务 bucket |
| `crates/ryframe-service/src/system/file_service.rs` | 文件校验、去重、配额、对象与元数据一致性、下载 |
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
APP_OBJECT_STORAGE_ACCESS_KEY
APP_OBJECT_STORAGE_SECRET_KEY
APP_OBJECT_STORAGE_USE_SSL
APP_OBJECT_STORAGE_REGION
```

v0.5 的上传响应只返回受认证的后端下载地址，bucket、普通文件和头像始终保持私有，不提供公共 URL 配置。前端展示头像时必须通过统一 HTTP 客户端携带 access token 下载 Blob，再创建浏览器对象 URL，不能把受保护地址直接绑定到原生 `img.src`。

切换成本地存储时使用：

```toml
[object_storage]
backend = "local"
local_base_dir = "uploads"
```

## 4. 运行时检查

受保护接口 `GET /api/v1/monitor/runtime` 返回对象存储后端、端点和动态连接状态。`connected` 会实际检查 `uploads` 与 `avatar`，不是静态配置回显。

文件链路通过以下接口进入同一个 `FileService`：

- `POST /api/v1/common/upload`
- `POST /api/v1/common/upload/image`
- `POST /api/v1/common/upload/avatar`
- `GET /api/v1/common/file/download`

对象写入成功但数据库元数据写入失败时，Service 会补偿删除对象；下载会同时校验租户文件元数据，不能仅凭对象路径跨租户读取。

普通文件上限为 10 MiB，头像上限为 5 MiB，上传超时为 120 秒。Nginx、Axum 请求体、multipart 和业务校验使用同一配置，固定长度或 chunked 超限都返回 `413`。

## 5. 测试与 CI

本地适配器和签名测试：

```bash
cargo test -p ryframe-storage
cargo test -p ryframe-service --test file_service_test
```

已经启动 RustFS 后，可显式运行外部集成测试：

```bash
cargo test -p ryframe-storage --test object_storage_test test_s3_integration_put_get_delete -- --ignored --exact
```

后端 CI 使用固定 RustFS 镜像，先执行适配器写入/读取/删除测试，再启动完整应用。运行时冒烟还会上传文本对象、通过受保护下载接口取回并逐字节比较，因此配置存在但实际链路不可用时 CI 必然失败。

## 6. 二次开发规则

- 新业务只依赖 `Arc<dyn ObjectStorage>` 或拥有它的领域 Service，不直接构造 RustFS/S3 客户端。
- bucket 必须是明确的业务常量；对象 key 必须保持相对路径，禁止 `..`、反斜杠和空路径段。
- 对象与数据库同时变化时，由 Service 定义补偿或事务外一致性策略。
- 新后端必须在组合根注册、配置校验、运行时监控、单元测试和 CI 中同时闭环。
- 不在 Repository 生成公开 URL，不在 Handler 读取对象存储配置。
