#!/usr/bin/env bash
# ============================================================
# RyFrame 一键部署脚本
# ============================================================
# 用法:
#   chmod +x deploy.sh
#   ./deploy.sh [dev|test|prod]    # 默认 prod
#
# 前置条件:
#   - Linux / macOS
#   - Docker ≥ 20.10 + Docker Compose ≥ 2.0
#   - Git（用于拉取代码）
# ============================================================

set -euo pipefail

# ---------- 颜色输出 ----------
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

log_info()  { echo -e "${GREEN}[INFO]${NC}  $*"; }
log_warn()  { echo -e "${YELLOW}[WARN]${NC}  $*"; }
log_error() { echo -e "${RED}[ERROR]${NC} $*"; }
log_step()  { echo -e "\n${CYAN}==== $* ====${NC}"; }

# ---------- 参数解析 ----------
ENV="${1:-prod}"
APP_NAME="ryframe"
PROJECT_DIR="$(cd "$(dirname "$0")" && pwd)"
LOG_FILE="${PROJECT_DIR}/deploy.log"
BACKUP_DIR="${PROJECT_DIR}/backups/$(date +%Y%m%d_%H%M%S)"

log_info "部署环境: ${ENV}"
log_info "项目目录: ${PROJECT_DIR}"

# ---------- 前置检查 ----------
log_step "1/7 前置检查"

check_command() {
    if ! command -v "$1" &>/dev/null; then
        log_error "$1 未安装，请先安装 $1"
        exit 1
    fi
}

check_command docker
check_command git

# 检查 Docker Compose（v2 插件 或独立版）
if docker compose version &>/dev/null; then
    COMPOSE_CMD="docker compose"
elif command -v docker-compose &>/dev/null; then
    COMPOSE_CMD="docker-compose"
else
    log_error "Docker Compose 未安装"
    exit 1
fi
log_info "Docker Compose: ${COMPOSE_CMD}"

# ---------- 加载环境变量 ----------
log_step "2/7 加载环境变量"

ENV_FILE="${PROJECT_DIR}/.env"
if [[ ! -f "${ENV_FILE}" ]]; then
    log_warn ".env 文件不存在，创建默认配置..."
    cat > "${ENV_FILE}" <<'EOF'
# RyFrame 部署环境变量
# 请务必修改以下密钥！

# MySQL root 密码
MYSQL_ROOT_PASSWORD=ryframe2024

# JWT 密钥（至少 32 位随机字符串）
JWT_SECRET=please-change-this-secret-key-in-production

# Redis 密码（留空则不设密码）
REDIS_PASSWORD=
EOF
    log_warn "已创建 .env 文件，请修改其中的密钥后重新运行部署脚本"
    exit 0
fi
log_info ".env 文件已加载"

# ---------- 拉取最新代码（可选） ----------
log_step "3/7 检查代码更新"

if [[ -d "${PROJECT_DIR}/.git" ]]; then
    read -rp "是否拉取最新代码？[y/N]: " PULL_CODE
    if [[ "${PULL_CODE}" =~ ^[Yy] ]]; then
        log_info "拉取最新代码..."
        git -C "${PROJECT_DIR}" pull --ff-only || {
            log_error "git pull 失败，请手动解决冲突"
            exit 1
        }
    fi
else
    log_warn "非 Git 仓库，跳过代码更新"
fi

# ---------- 备份现有数据 ----------
log_step "4/7 备份数据"

mkdir -p "${BACKUP_DIR}"

# 备份配置文件
if [[ -d "${PROJECT_DIR}/config" ]]; then
    cp -r "${PROJECT_DIR}/config" "${BACKUP_DIR}/config"
    log_info "配置已备份到 ${BACKUP_DIR}/config"
fi

# 备份 .env
if [[ -f "${ENV_FILE}" ]]; then
    cp "${ENV_FILE}" "${BACKUP_DIR}/.env"
    log_info ".env 已备份到 ${BACKUP_DIR}/.env"
fi

# ---------- 构建并启动 ----------
log_step "5/7 构建 Docker 镜像"

cd "${PROJECT_DIR}"

case "${ENV}" in
    dev)
        export APP_ENV=dev
        ;;
    test)
        export APP_ENV=test
        ;;
    prod|*)
        export APP_ENV=prod
        ;;
esac

# 停止旧容器
log_info "停止旧容器..."
${COMPOSE_CMD} down --remove-orphans 2>/dev/null || true

# 构建新镜像
log_info "构建镜像（首次构建约需 5-15 分钟）..."
${COMPOSE_CMD} build --no-cache 2>&1 | tee -a "${LOG_FILE}"

# ---------- 启动服务 ----------
log_step "6/7 启动服务"

${COMPOSE_CMD} up -d 2>&1 | tee -a "${LOG_FILE}"

# 等待服务启动
log_info "等待服务启动..."
MAX_WAIT=60
WAIT_COUNT=0
while [[ ${WAIT_COUNT} -lt ${MAX_WAIT} ]]; do
    if curl -sf http://localhost:8080/api/v1/monitor/health &>/dev/null; then
        log_info "服务已就绪！"
        break
    fi
    sleep 3
    WAIT_COUNT=$((WAIT_COUNT + 3))
    echo -ne "\r  已等待 ${WAIT_COUNT}s / ${MAX_WAIT}s..."
done
echo ""

if [[ ${WAIT_COUNT} -ge ${MAX_WAIT} ]]; then
    log_warn "服务启动超时（${MAX_WAIT}s），请手动检查容器状态"
    log_warn "查看日志: ${COMPOSE_CMD} logs ryframe"
fi

# ---------- 状态检查 ----------
log_step "7/7 状态检查"

echo ""
echo "============================================================"
echo "  容器状态"
echo "============================================================"
${COMPOSE_CMD} ps
echo ""

echo "============================================================"
echo "  访问地址"
echo "============================================================"
echo "  API:        http://<服务器IP>:8080"
echo "  Nginx:      http://<服务器IP>:80"
echo "  OpenAPI:    http://<服务器IP>:8080/api/v1/openapi.json"
echo "  MySQL:      <服务器IP>:3306"
echo "  Redis:      <服务器IP>:6379"
echo "============================================================"
echo ""

echo "============================================================"
echo "  常用运维命令"
echo "============================================================"
echo "  查看日志:      ${COMPOSE_CMD} logs -f ryframe"
echo "  重启服务:      ${COMPOSE_CMD} restart ryframe"
echo "  停止全部:      ${COMPOSE_CMD} down"
echo "  查看状态:      ${COMPOSE_CMD} ps"
echo "  进入容器:      docker exec -it ryframe-app bash"
echo "  数据库备份:    docker exec ryframe-mysql mysqldump -u root -p ryframe_config > backup.sql"
echo "============================================================"
echo ""

log_info "部署完成！环境: ${ENV}"
log_info "部署日志: ${LOG_FILE}"
log_info "数据备份: ${BACKUP_DIR}"
