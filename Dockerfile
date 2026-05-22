# ============================================================
# RyFrame 多阶段构建 Dockerfile
# ============================================================

# Stage 1: 构建
FROM rust:1.85-slim AS builder

WORKDIR /build

# 安装构建依赖
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# 先复制 Cargo 文件，利用 Docker 缓存
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/

# 预构建依赖（缓存层）
RUN mkdir -p crates/ryframe/src && \
    echo "fn main() {}" > crates/ryframe/src/main.rs && \
    cargo build --release 2>/dev/null || true

# 复制完整源码并构建
COPY . .
RUN touch crates/ryframe/src/main.rs && \
    cargo build --release --bin ryframe

# Stage 2: 运行时
FROM debian:bookworm-slim

WORKDIR /app

# 安装运行时依赖
RUN apt-get update && apt-get install -y \
    ca-certificates \
    tzdata \
    && rm -rf /var/lib/apt/lists/*

# 设置时区
ENV TZ=Asia/Shanghai

# 从构建阶段复制二进制
COPY --from=builder /build/target/release/ryframe /app/ryframe

# 复制配置文件
COPY config/ /app/config/

# 复制 SQL 初始化脚本（供手动执行）
COPY sql/ /app/sql/

# 创建非 root 用户
RUN groupadd -r ryframe && useradd -r -g ryframe -m ryframe
RUN chown -R ryframe:ryframe /app
USER ryframe

# 暴露端口
EXPOSE 8080

# 健康检查
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:8080/ || exit 1

# 启动应用
ENTRYPOINT ["/app/ryframe"]
