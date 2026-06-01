#!/usr/bin/env bash
# ============================================================
# RyFrame 数据库备份脚本
#
# 支持 MySQL 和 PostgreSQL 自动检测与备份。
# 自动压缩、保留最近 N 天的备份、可选上传到 S3。
#
# 使用方式：
#   chmod +x deploy.sh
#   ./deploy.sh backup              # 手动备份
#   ./deploy.sh restore <file>      # 从备份文件恢复
#   ./deploy.sh list                # 列出所有备份
#   ./deploy.sh clean [days]        # 清理旧备份（默认保留 30 天）
#
# 定时任务（crontab）示例：
#   0 2 * * * /path/to/ryframe/deploy.sh backup >> /var/log/ryframe-backup.log 2>&1
#
# 环境变量（可覆盖默认值）：
#   DB_DRIVER          - 数据库类型（mysql | postgres），默认自动检测
#   DB_HOST            - 数据库主机，默认 localhost
#   DB_PORT            - 数据库端口
#   DB_NAME            - 数据库名称，默认 ryframe
#   DB_USER            - 数据库用户，默认 root
#   DB_PASSWORD        - 数据库密码
#   BACKUP_DIR         - 备份目录，默认 ./backup
#   BACKUP_RETENTION   - 备份保留天数，默认 30
#   S3_BACKUP_ENABLED  - 是否上传到 S3（true/false），默认 false
#   S3_ENDPOINT        - S3 端点
#   S3_BUCKET          - S3 Bucket 名称
# ============================================================

set -euo pipefail

# ============================================================
# 配置（环境变量可覆盖）
# ============================================================
DB_DRIVER="${DB_DRIVER:-}"
DB_HOST="${DB_HOST:-localhost}"
DB_PORT="${DB_PORT:-}"
DB_NAME="${DB_NAME:-ryframe}"
DB_USER="${DB_USER:-root}"
DB_PASSWORD="${DB_PASSWORD:-}"
BACKUP_DIR="${BACKUP_DIR:-$(dirname "$0")/backup}"
BACKUP_RETENTION="${BACKUP_RETENTION:-30}"
TIMESTAMP=$(date +"%Y%m%d_%H%M%S")

# S3 备份（可选）
S3_BACKUP_ENABLED="${S3_BACKUP_ENABLED:-false}"
S3_ENDPOINT="${S3_ENDPOINT:-}"
S3_BUCKET="${S3_BUCKET:-ryframe-backups}"

# ============================================================
# 颜色输出
# ============================================================
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

log_info()  { echo -e "${BLUE}[INFO]${NC}  $(date '+%Y-%m-%d %H:%M:%S') $*"; }
log_ok()    { echo -e "${GREEN}[OK]${NC}    $(date '+%Y-%m-%d %H:%M:%S') $*"; }
log_warn()  { echo -e "${YELLOW}[WARN]${NC}  $(date '+%Y-%m-%d %H:%M:%S') $*"; }
log_error() { echo -e "${RED}[ERROR]${NC} $(date '+%Y-%m-%d %H:%M:%S') $*"; }

# ============================================================
# 自动检测数据库类型
# ============================================================
detect_driver() {
    if [ -n "$DB_DRIVER" ]; then
        return
    fi

    if command -v mysql &>/dev/null; then
        DB_DRIVER="mysql"
        DB_PORT="${DB_PORT:-3306}"
    elif command -v psql &>/dev/null; then
        DB_DRIVER="postgres"
        DB_PORT="${DB_PORT:-5432}"
    else
        log_error "无法检测数据库类型。请设置 DB_DRIVER 环境变量（mysql 或 postgres）"
        exit 1
    fi
    log_info "自动检测数据库类型: ${DB_DRIVER}"
}

# ============================================================
# 检查依赖
# ============================================================
check_deps() {
    local missing=()

    if [ "$DB_DRIVER" = "mysql" ] && ! command -v mysqldump &>/dev/null; then
        missing+=("mysqldump (mysql-client)")
    fi
    if [ "$DB_DRIVER" = "postgres" ] && ! command -v pg_dump &>/dev/null; then
        missing+=("pg_dump (postgresql-client)")
    fi
    if ! command -v gzip &>/dev/null; then
        missing+=("gzip")
    fi
    if [ "$S3_BACKUP_ENABLED" = "true" ] && ! command -v aws &>/dev/null; then
        missing+=("aws-cli (s3 备份需要)")
    fi

    if [ ${#missing[@]} -gt 0 ]; then
        log_error "缺少依赖: ${missing[*]}"
        exit 1
    fi
}

# ============================================================
# 创建备份目录
# ============================================================
ensure_backup_dir() {
    if [ ! -d "$BACKUP_DIR" ]; then
        mkdir -p "$BACKUP_DIR"
        log_info "创建备份目录: $BACKUP_DIR"
    fi
}

# ============================================================
# MySQL 备份
# ============================================================
backup_mysql() {
    local dump_file="${BACKUP_DIR}/${DB_NAME}_${TIMESTAMP}.sql"
    local compressed_file="${dump_file}.gz"

    log_info "开始 MySQL 备份: ${DB_NAME} → ${compressed_file}"

    local auth_args=()
    if [ -n "$DB_PASSWORD" ]; then
        auth_args+=("-p${DB_PASSWORD}")
    fi

    if mysqldump \
        -h "$DB_HOST" \
        -P "$DB_PORT" \
        -u "$DB_USER" \
        "${auth_args[@]}" \
        --single-transaction \
        --routines \
        --triggers \
        --events \
        --hex-blob \
        --default-character-set=utf8mb4 \
        "$DB_NAME" 2>/tmp/ryframe_backup_err.log | gzip > "$compressed_file"; then
        local size=$(du -h "$compressed_file" | cut -f1)
        log_ok "MySQL 备份完成: ${compressed_file} (${size})"
    else
        log_error "MySQL 备份失败:"
        cat /tmp/ryframe_backup_err.log
        rm -f "$compressed_file"
        return 1
    fi

    # 创建软链接指向最新备份
    ln -sf "$(basename "$compressed_file")" "${BACKUP_DIR}/${DB_NAME}_latest.sql.gz"

    echo "$compressed_file"
}

# ============================================================
# PostgreSQL 备份
# ============================================================
backup_postgres() {
    local dump_file="${BACKUP_DIR}/${DB_NAME}_${TIMESTAMP}.dump"
    local compressed_file="${dump_file}.gz"

    log_info "开始 PostgreSQL 备份: ${DB_NAME} → ${compressed_file}"

    export PGPASSWORD="$DB_PASSWORD"

    if pg_dump \
        -h "$DB_HOST" \
        -p "$DB_PORT" \
        -U "$DB_USER" \
        -F c \
        -b \
        -v \
        -f "$dump_file" \
        "$DB_NAME" 2>/tmp/ryframe_backup_err.log; then
        gzip -f "$dump_file"
        local size=$(du -h "$compressed_file" | cut -f1)
        log_ok "PostgreSQL 备份完成: ${compressed_file} (${size})"
    else
        log_error "PostgreSQL 备份失败:"
        cat /tmp/ryframe_backup_err.log
        rm -f "$dump_file"
        return 1
    fi

    unset PGPASSWORD

    # 创建软链接指向最新备份
    ln -sf "$(basename "$compressed_file")" "${BACKUP_DIR}/${DB_NAME}_latest.dump.gz"

    echo "$compressed_file"
}

# ============================================================
# 上传到 S3
# ============================================================
upload_to_s3() {
    local file="$1"

    if [ "$S3_BACKUP_ENABLED" != "true" ]; then
        return 0
    fi

    log_info "上传备份到 S3: s3://${S3_BUCKET}/$(basename "$file")"

    local endpoint_args=()
    if [ -n "$S3_ENDPOINT" ]; then
        endpoint_args+=(--endpoint-url "$S3_ENDPOINT")
    fi

    if aws s3 cp "$file" "s3://${S3_BUCKET}/$(basename "$file")" "${endpoint_args[@]}" --no-progress; then
        log_ok "S3 上传完成"
    else
        log_warn "S3 上传失败，本地备份已保留"
    fi
}

# ============================================================
# 清理旧备份
# ============================================================
clean_old_backups() {
    local retention="${1:-$BACKUP_RETENTION}"

    log_info "清理 ${retention} 天前的备份..."

    local count=0
    while IFS= read -r old_file; do
        if [ -n "$old_file" ]; then
            rm -f "$old_file"
            count=$((count + 1))
            log_info "  已删除: $(basename "$old_file")"
        fi
    done < <(find "$BACKUP_DIR" -type f \( -name "*.sql.gz" -o -name "*.dump.gz" \) -mtime "+${retention}" 2>/dev/null)

    # 清理临时错误日志
    rm -f /tmp/ryframe_backup_err.log

    log_ok "清理完成，删除了 ${count} 个旧备份"
}

# ============================================================
# 列出备份
# ============================================================
list_backups() {
    ensure_backup_dir

    echo ""
    echo "============================================================"
    echo "  RyFrame 备份列表"
    echo "  目录: $BACKUP_DIR"
    echo "============================================================"

    local count=0
    for f in "$BACKUP_DIR"/*.{sql.gz,dump.gz} 2>/dev/null; do
        if [ -f "$f" ]; then
            local size=$(du -h "$f" | cut -f1)
            local date=$(stat -c "%y" "$f" 2>/dev/null || stat -f "%Sm" "$f" 2>/dev/null)
            echo "  $(basename "$f")  (${size})  ${date}"
            count=$((count + 1))
        fi
    done

    if [ $count -eq 0 ]; then
        echo "  (无备份文件)"
    fi
    echo "------------------------------------------------------------"
    echo "  共 ${count} 个备份"
    echo "============================================================"
    echo ""
}

# ============================================================
# 恢复备份
# ============================================================
restore_backup() {
    local file="$1"

    if [ ! -f "$file" ]; then
        log_error "备份文件不存在: $file"
        exit 1
    fi

    detect_driver

    echo ""
    echo "============================================================"
    echo "  ⚠️  警告: 即将恢复数据库 ${DB_NAME}，所有现有数据将被覆盖！"
    echo "  备份文件: $file"
    echo "============================================================"
    echo ""
    read -r -p "  确认恢复? (输入 yes 继续): " confirm

    if [ "$confirm" != "yes" ]; then
        log_info "已取消恢复操作"
        exit 0
    fi

    if [ "$DB_DRIVER" = "mysql" ]; then
        log_info "开始 MySQL 恢复: ${DB_NAME} ← ${file}"
        local auth_args=()
        if [ -n "$DB_PASSWORD" ]; then
            auth_args+=("-p${DB_PASSWORD}")
        fi

        if gunzip -c "$file" | mysql \
            -h "$DB_HOST" \
            -P "$DB_PORT" \
            -u "$DB_USER" \
            "${auth_args[@]}" \
            "$DB_NAME"; then
            log_ok "MySQL 恢复完成"
        else
            log_error "MySQL 恢复失败"
            exit 1
        fi
    elif [ "$DB_DRIVER" = "postgres" ]; then
        log_info "开始 PostgreSQL 恢复: ${DB_NAME} ← ${file}"
        export PGPASSWORD="$DB_PASSWORD"

        if gunzip -c "$file" | pg_restore \
            -h "$DB_HOST" \
            -p "$DB_PORT" \
            -U "$DB_USER" \
            -d "$DB_NAME" \
            -v \
            --clean \
            --if-exists; then
            log_ok "PostgreSQL 恢复完成"
        else
            log_error "PostgreSQL 恢复失败"
            exit 1
        fi

        unset PGPASSWORD
    fi
}

# ============================================================
# 备份主流程
# ============================================================
do_backup() {
    detect_driver
    check_deps
    ensure_backup_dir

    local backup_file=""

    if [ "$DB_DRIVER" = "mysql" ]; then
        backup_file=$(backup_mysql)
    elif [ "$DB_DRIVER" = "postgres" ]; then
        backup_file=$(backup_postgres)
    else
        log_error "不支持的数据库类型: ${DB_DRIVER}"
        exit 1
    fi

    # 上传到 S3
    if [ -n "$backup_file" ]; then
        upload_to_s3 "$backup_file"
    fi

    # 自动清理旧备份
    clean_old_backups "$BACKUP_RETENTION"

    log_ok "备份流程完成"
}

# ============================================================
# 帮助信息
# ============================================================
show_help() {
    echo "RyFrame 数据库备份工具"
    echo ""
    echo "用法: $0 <命令> [参数]"
    echo ""
    echo "命令:"
    echo "  backup              执行数据库备份"
    echo "  restore <file>      从指定备份文件恢复数据库"
    echo "  list                列出所有备份文件"
    echo "  clean [days]        清理旧备份（默认保留 ${BACKUP_RETENTION} 天）"
    echo "  help                显示此帮助"
    echo ""
    echo "环境变量:"
    echo "  DB_DRIVER           数据库类型（mysql | postgres）"
    echo "  DB_HOST             数据库主机（默认 localhost）"
    echo "  DB_PORT             数据库端口"
    echo "  DB_NAME             数据库名称（默认 ryframe）"
    echo "  DB_USER             数据库用户（默认 root）"
    echo "  DB_PASSWORD         数据库密码"
    echo "  BACKUP_DIR          备份目录（默认 ./backup）"
    echo "  BACKUP_RETENTION    备份保留天数（默认 30）"
    echo "  S3_BACKUP_ENABLED   是否上传到 S3（true/false）"
    echo "  S3_ENDPOINT         S3 端点 URL"
    echo "  S3_BUCKET           S3 Bucket 名称（默认 ryframe-backups）"
    echo ""
    echo "示例:"
    echo "  $0 backup"
    echo "  $0 backup                            # 使用默认配置备份"
    echo "  DB_NAME=ryframe_config $0 backup     # 指定数据库名称"
    echo "  $0 restore ./backup/ryframe_latest.sql.gz"
    echo "  $0 list"
    echo "  $0 clean 60                          # 保留 60 天"
}

# ============================================================
# 入口
# ============================================================
main() {
    local cmd="${1:-help}"

    case "$cmd" in
        backup)
            do_backup
            ;;
        restore)
            if [ -z "${2:-}" ]; then
                log_error "restore 命令需要指定备份文件路径"
                echo "用法: $0 restore <file>"
                exit 1
            fi
            restore_backup "$2"
            ;;
        list)
            list_backups
            ;;
        clean)
            local days="${2:-$BACKUP_RETENTION}"
            ensure_backup_dir
            clean_old_backups "$days"
            ;;
        help|--help|-h)
            show_help
            ;;
        *)
            log_error "未知命令: $cmd"
            show_help
            exit 1
            ;;
    esac
}

main "$@"

