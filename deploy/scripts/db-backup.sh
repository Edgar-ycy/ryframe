#!/bin/bash
# RyFrame 数据库备份与恢复脚本
#
# 功能：
#   backup  - 备份 MySQL 数据库（全量导出）
#   restore - 从备份文件恢复数据库
#   rotate  - 清理过期备份文件
#
# 使用：
#   ./deploy/scripts/db-backup.sh backup
#   ./deploy/scripts/db-backup.sh restore backup/db_backup_20260531_120000.sql.gz
#   ./deploy/scripts/db-backup.sh rotate 30
#
# 环境变量（可选，也有默认值）：
#   DB_HOST     - 数据库主机（默认 localhost）
#   DB_PORT     - 数据库端口（默认 3306）
#   DB_USER     - 数据库用户（默认 root）
#   DB_PASSWORD - 数据库密码
#   DB_NAME     - 数据库名（默认 ryframe）
#   BACKUP_DIR  - 备份目录（默认 ./backup）
#   RETENTION_DAYS - 备份保留天数（默认 30）

set -euo pipefail

# ==================== 配置 ====================
DB_HOST="${DB_HOST:-localhost}"
DB_PORT="${DB_PORT:-3306}"
DB_USER="${DB_USER:-root}"
DB_PASSWORD="${DB_PASSWORD:-}"
DB_NAME="${DB_NAME:-ryframe}"
BACKUP_DIR="${BACKUP_DIR:-$(cd "$(dirname "$0")/../../backup" && pwd)}"
RETENTION_DAYS="${RETENTION_DAYS:-30}"
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
BACKUP_FILE="${BACKUP_DIR}/db_backup_${TIMESTAMP}.sql.gz"

# 日志函数
log() { echo "[$(date '+%Y-%m-%d %H:%M:%S')] $*"; }
error() { log "ERROR: $*" >&2; exit 1; }

# ==================== 检查依赖 ====================
check_deps() {
    command -v mysqldump >/dev/null 2>&1 || error "mysqldump 未安装，请安装 mysql-client"
    command -v gzip >/dev/null 2>&1 || error "gzip 未安装"
}

# ==================== 备份 ====================
backup() {
    log "开始备份数据库 ${DB_NAME}..."
    mkdir -p "${BACKUP_DIR}"

    MYSQL_PWD="${DB_PASSWORD}" mysqldump \
        --host="${DB_HOST}" \
        --port="${DB_PORT}" \
        --user="${DB_USER}" \
        --single-transaction \
        --routines \
        --triggers \
        --events \
        --hex-blob \
        --default-character-set=utf8mb4 \
        "${DB_NAME}" | gzip > "${BACKUP_FILE}"

    # 验证备份文件
    if [[ -f "${BACKUP_FILE}" && -s "${BACKUP_FILE}" ]]; then
        local size=$(du -h "${BACKUP_FILE}" | cut -f1)
        log "备份完成: ${BACKUP_FILE} (${size})"
    else
        error "备份文件为空或创建失败: ${BACKUP_FILE}"
    fi

    # 创建软链接指向最新备份
    ln -sf "$(basename "${BACKUP_FILE}")" "${BACKUP_DIR}/latest.sql.gz"
    log "已更新 latest.sql.gz → $(basename "${BACKUP_FILE}")"
}

# ==================== 恢复 ====================
restore() {
    local restore_file="${1:-}"

    if [[ -z "${restore_file}" ]]; then
        # 如果没有指定文件，使用最新的备份
        restore_file="${BACKUP_DIR}/latest.sql.gz"
        log "未指定备份文件，使用最新备份: ${restore_file}"
    fi

    if [[ ! -f "${restore_file}" ]]; then
        error "备份文件不存在: ${restore_file}"
    fi

    log "⚠️  警告：即将覆盖数据库 ${DB_NAME} 的所有数据！"
    log "   目标: ${DB_HOST}:${DB_PORT}/${DB_NAME}"
    log "   备份: ${restore_file}"
    echo -n "确认恢复？输入 'yes' 继续: "
    read -r confirm
    if [[ "${confirm}" != "yes" ]]; then
        log "操作取消"
        exit 0
    fi

    # 解压并导入
    if [[ "${restore_file}" == *.gz ]]; then
        gunzip -c "${restore_file}" | MYSQL_PWD="${DB_PASSWORD}" mysql \
            --host="${DB_HOST}" \
            --port="${DB_PORT}" \
            --user="${DB_USER}" \
            --default-character-set=utf8mb4 \
            "${DB_NAME}"
    else
        MYSQL_PWD="${DB_PASSWORD}" mysql \
            --host="${DB_HOST}" \
            --port="${DB_PORT}" \
            --user="${DB_USER}" \
            --default-character-set=utf8mb4 \
            "${DB_NAME}" < "${restore_file}"
    fi

    log "数据库恢复完成: ${restore_file} → ${DB_NAME}"
}

# ==================== 清理过期备份 ====================
rotate() {
    local days="${1:-${RETENTION_DAYS}}"
    log "清理 ${days} 天前的备份文件..."

    local deleted=0
    # 删除超过指定天数的 .sql.gz 文件（保留 latest 软链接）
    while IFS= read -r -d '' file; do
        log "删除过期备份: $(basename "${file}")"
        rm -f "${file}"
        ((deleted++))
    done < <(find "${BACKUP_DIR}" -name "db_backup_*.sql.gz" -mtime "+${days}" -print0)

    log "清理完成，删除了 ${deleted} 个过期备份"
}

# ==================== 主入口 ====================
check_deps

case "${1:-}" in
    backup)
        backup
        ;;
    restore)
        restore "${2:-}"
        ;;
    rotate)
        rotate "${2:-}"
        ;;
    *)
        echo "用法: $0 {backup|restore [file]|rotate [days]}"
        echo ""
        echo "  backup         全量备份数据库"
        echo "  restore [file] 从备份文件恢复（默认使用 latest.sql.gz）"
        echo "  rotate [days]  清理过期备份（默认 ${RETENTION_DAYS} 天前）"
        echo ""
        echo "环境变量:"
        echo "  DB_HOST        数据库主机 (默认: localhost)"
        echo "  DB_PORT        数据库端口 (默认: 3306)"
        echo "  DB_USER        数据库用户 (默认: root)"
        echo "  DB_PASSWORD    数据库密码"
        echo "  DB_NAME        数据库名 (默认: ryframe)"
        echo "  BACKUP_DIR     备份目录 (默认: ./backup)"
        echo "  RETENTION_DAYS 保留天数 (默认: 30)"
        exit 1
        ;;
esac
