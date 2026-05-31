# 部署指南

## 环境要求

| 组件 | 最低版本 | 说明 |
|------|----------|------|
| Rust | 1.85+ | 编译环境（Docker 构建无需本地安装） |
| Docker | 20.10+ | 容器运行时 |
| Docker Compose | 2.0+ | 容器编排 |
| MySQL | 8.0+ | 主数据库 |
| Redis | 7.0+ | 缓存/会话/限流/黑名单 |

## 开发环境

### 1. 使用 Docker Compose 启动依赖服务

```bash
# 启动 MySQL + Redis
docker-compose up -d mysql redis
```

### 2. 配置文件

```bash
# 默认读取 config/app.toml
# 通过 APP_ENV 环境变量切换环境配置
cp config/app.toml config/app.dev.toml
# 编辑 config/app.dev.toml 修改数据库连接等
```

### 3. 运行数据库迁移

```bash
cargo run --bin ryframe-migration -- up
```

### 4. 启动开发服务器

```bash
cargo run
```

服务默认监听 `http://0.0.0.0:8080`。

### 5. 初始化数据

```bash
# SQL 初始化脚本位于 sql/ryframe_config.sql
# docker-compose 启动 MySQL 时会自动执行 sql/ 目录下的脚本
docker exec -i ryframe-mysql mysql -u root -p < sql/ryframe_config.sql
```

默认管理员账号：`admin` / `admin123`

## 生产环境部署

### 使用 deploy.sh 一键部署

项目提供自动化部署脚本 `deploy.sh`：

```bash
chmod +x deploy.sh
./deploy.sh prod    # 生产环境
./deploy.sh dev     # 开发环境
./deploy.sh test    # 测试环境
```

部署流程：前置检查 → 加载环境变量 → 代码更新 → 数据备份 → Docker 构建 → 启动服务 → 状态检查。

### Docker 部署

项目使用多阶段构建，`Dockerfile`：

- **Stage 1 (Builder)**: `rust:1.85-slim` 编译 release 二进制
- **Stage 2 (Runtime)**: `debian:bookworm-slim` 复制二进制 + 配置

```bash
# 构建镜像
docker build -t ryframe:latest .

# 运行
docker run -d \
  --name ryframe \
  -p 8080:8080 \
  -e APP_ENV=prod \
  -e APP_DATABASE_PRIMARY_HOST=mysql \
  -e APP_DATABASE_PRIMARY_PORT=3306 \
  -e APP_DATABASE_PRIMARY_USERNAME=root \
  -e APP_DATABASE_PRIMARY_PASSWORD=yourpassword \
  -e APP_REDIS_HOST=redis \
  -e APP_AUTH_JWT_SECRET=your-secret-key \
  -v $(pwd)/config:/app/config \
  ryframe:latest
```

### docker-compose 完整部署

`docker-compose.yml` 包含以下服务：

| 服务 | 容器名 | 端口 | 说明 |
|------|--------|------|------|
| ryframe | ryframe-app | 8080 | 应用服务 |
| mysql | ryframe-mysql | 3306 | MySQL 8.0 + utf8mb4 |
| redis | ryframe-redis | 6379 | Redis 7 Alpine + AOF 持久化 |
| nginx | ryframe-nginx | 80 | Nginx 反向代理（可选） |

```bash
# 创建 .env 文件（首次）
cat > .env <<'EOF'
MYSQL_ROOT_PASSWORD=ryframe2024
JWT_SECRET=your-64-char-random-string
REDIS_PASSWORD=
EOF

# 构建并启动所有服务
docker-compose up -d

# 查看日志
docker-compose logs -f ryframe

# 重启
docker-compose restart ryframe

# 停止
docker-compose down
```

### Kubernetes 部署

项目提供 `deploy/k8s/all-in-one.yaml` 单文件部署清单，包含：

- Deployment + Service + ConfigMap + Secret
- liveness / readiness probe 健康检查
- 资源限制（requests/limits）

```bash
kubectl apply -f deploy/k8s/all-in-one.yaml
```

## Nginx 反向代理

`deploy/nginx.conf` 提供生产级 Nginx 配置：

```nginx
upstream ryframe_backend {
    server ryframe:8080;
    keepalive 32;
}

server {
    listen 80;
    server_name localhost;

    # API 反向代理（限流 10r/s）
    location /api/ {
        limit_req zone=api_limit burst=20 nodelay;
        proxy_pass http://ryframe_backend;
        # ...
    }

    # 健康检查端点（不限流）
    location / {
        proxy_pass http://ryframe_backend;
    }
}
```

特性：Gzip 压缩、安全响应头、静态文件缓存、WebSocket 支持。

## 配置说明

### 应用配置 (`config/app.toml`)

```toml
[app]
name = "ryframe"
version = "0.1.0"
host = "0.0.0.0"
port = 8080
```

### 数据库配置

```toml
[database]
sql_log_level = "off"  # off | summary | full

[[database.connections]]
driver = "postgres"      # postgres | mysql | sqlite
host = "localhost"
port = 5432
database = "ryframe"
username = "postgres"
password = ""
max_connections = 10
min_connections = 1
```

### 认证配置

```toml
[auth]
jwt_secret = "change-me-in-production"
access_token_expire = "1h"
refresh_token_expire = "168h"    # 7天
```

### Redis 配置

```toml
[redis]
host = "127.0.0.1"
port = 6379
password = ""
database = 0
max_pool_size = 16
timeout_secs = 3
```

### 限流配置

```toml
[rate_limit]
enabled = true
capacity = 100          # 令牌桶容量
refill_per_sec = 20     # 每秒补充令牌数
window_secs = 0         # 0 = 使用 refill_per_sec 令牌桶模式

# 用户级限流
enable_user_rate_limit = false
user_window_secs = 60
user_capacity = 500

# 接口级敏感端点限流
[rate_limit.api_limits]
"POST /api/v1/auth/login" = 5   # 登录接口 5次/分钟
api_window_secs = 60
```

### 日志配置

```toml
[logger]
level = "info"          # trace | debug | info | warn | error
format = "text"         # json | text
output = "stdout"       # stdout | file
```

### 对象存储配置

```toml
[object_storage]
backend = "s3"          # local | minio | s3
local_base_dir = "uploads"
public_base_url = ""
endpoint = "http://localhost:9000"
access_key = "minioadmin"
secret_key = "minioadmin"
use_ssl = false
region = "us-east-1"
```

## 监控与可观测性

### 健康检查端点

```
GET /             → {"status":"ok"}           # 基础存活检查
GET /api/v1/monitor/health  → 组件状态         # DB/Redis/Disk
GET /api/v1/version         → 服务版本信息      # 版本 + 端点列表
```

### Prometheus Metrics

```
GET /metrics    → Prometheus 格式指标
```

### Grafana 仪表板

`deploy/grafana/dashboards/` 提供预置面板 JSON 模板（Prometheus 数据源）。

### 链路追踪

通过 OpenTelemetry 导出到支持 OTLP 协议的后端（Jaeger / Tempo / Datadog）。

## 测试脚本

### 冒烟测试

```bash
node deploy/tests/smoke-test.js
```

验证所有核心 API 端点正常响应。

### 压力测试

```bash
node deploy/tests/stress-test.js
```

基于 k6 的负载测试，覆盖登录、CRUD、并发场景。

## 备份策略

### 数据库备份

```bash
# MySQL
docker exec ryframe-mysql mysqldump -u root -p ryframe_config > backup.sql

# PostgreSQL
docker exec ryframe-postgres pg_dump -U postgres ryframe > backup.sql
```

### 定时备份

```bash
# crontab: 每天凌晨 3 点备份
0 3 * * * /path/to/deploy/scripts/backup.sh
```

### deploy.sh 自动备份

`deploy.sh` 部署时自动备份配置文件和 `.env` 到 `backups/YYYYMMDD_HHMMSS/` 目录。

## 常用运维命令

```bash
# 查看日志
docker-compose logs -f ryframe

# 重启服务
docker-compose restart ryframe

# 停止全部服务
docker-compose down

# 进入应用容器
docker exec -it ryframe-app bash

# 进入数据库容器
docker exec -it ryframe-mysql mysql -u root -p

# 数据库备份
docker exec ryframe-mysql mysqldump -u root -p ryframe_config > backup.sql

# 查看容器状态
docker-compose ps
```

## 安全建议

1. **HTTPS**: 生产环境必须使用 HTTPS（通过 Nginx 或 Ingress 终结 SSL）
2. **JWT Secret**: 使用 ≥ 64 字符的随机字符串，通过环境变量注入
3. **数据库密码**: 通过 `.env` 文件管理，不提交到 Git
4. **防火墙**: 数据库端口（3306/6379）不对外开放
5. **日志脱敏**: 生产环境 `logger.format = "json"`，敏感字段已自动脱敏
6. **依赖审计**: 定期运行 `cargo audit`（配置见 `.cargo/audit.toml`）
7. **速率限制**: 启用 `rate_limit` 防止暴力破解和 DDoS
8. **安全头**: 应用自动添加 CSP/HSTS/X-Frame-Options 等安全响应头
9. **Docker 非 root**: 容器以非 root 用户 `ryframe` 运行
