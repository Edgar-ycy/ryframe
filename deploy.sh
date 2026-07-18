#!/usr/bin/env bash
# RyFrame MySQL backup and recovery utility.

set -Eeuo pipefail
umask 077

DB_HOST="${DB_HOST:-127.0.0.1}"
DB_PORT="${DB_PORT:-3306}"
DB_NAME="${DB_NAME:-ryframe_config}"
DB_USER="${DB_USER:-root}"
DB_PASSWORD="${DB_PASSWORD:-}"
BACKUP_DIR="${BACKUP_DIR:-$(cd "$(dirname "$0")" && pwd)/backup}"
BACKUP_RETENTION="${BACKUP_RETENTION:-30}"
S3_BACKUP_ENABLED="${S3_BACKUP_ENABLED:-false}"
S3_ENDPOINT="${S3_ENDPOINT:-}"
S3_BUCKET="${S3_BUCKET:-ryframe-backups}"
TIMESTAMP="$(date -u +%Y%m%dT%H%M%SZ)"

MYSQL_DEFAULTS_FILE=""
BACKUP_RESULT=""

log() {
    printf '[%s] %s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "$*"
}

fail() {
    log "ERROR: $*" >&2
    exit 1
}

cleanup() {
    if [[ -n "$MYSQL_DEFAULTS_FILE" && -f "$MYSQL_DEFAULTS_FILE" ]]; then
        rm -f -- "$MYSQL_DEFAULTS_FILE"
    fi
}
trap cleanup EXIT INT TERM

validate_database_name() {
    local name="$1"
    [[ "$name" =~ ^[A-Za-z0-9_]+$ ]] || fail "invalid database name"
}

validate_positive_integer() {
    local value="$1"
    local label="$2"
    [[ "$value" =~ ^[0-9]+$ ]] || fail "$label must be a non-negative integer"
}

require_command() {
    command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"
}

prepare_mysql_credentials() {
    [[ -n "$MYSQL_DEFAULTS_FILE" ]] && return
    MYSQL_DEFAULTS_FILE="$(mktemp "${TMPDIR:-/tmp}/ryframe-mysql.XXXXXX.cnf")"
    chmod 600 "$MYSQL_DEFAULTS_FILE"
    {
        printf '[client]\n'
        printf 'host=%s\n' "$DB_HOST"
        printf 'port=%s\n' "$DB_PORT"
        printf 'user=%s\n' "$DB_USER"
        printf 'password=%s\n' "$DB_PASSWORD"
        printf 'default-character-set=utf8mb4\n'
    } >"$MYSQL_DEFAULTS_FILE"
}

mysql_client() {
    mysql --defaults-extra-file="$MYSQL_DEFAULTS_FILE" --protocol=TCP "$@"
}

check_dependencies() {
    require_command mysql
    require_command mysqldump
    require_command gzip
    require_command gunzip
    require_command awk
    require_command find
    if [[ "$S3_BACKUP_ENABLED" == "true" ]]; then
        require_command aws
    elif [[ "$S3_BACKUP_ENABLED" != "false" ]]; then
        fail "S3_BACKUP_ENABLED must be true or false"
    fi
}

ensure_backup_directory() {
    mkdir -p -- "$BACKUP_DIR"
    chmod 700 "$BACKUP_DIR"
}

validate_backup() {
    local file="$1"
    [[ -f "$file" ]] || fail "backup does not exist: $file"
    [[ "$file" == *.sql.gz ]] || fail "only .sql.gz backups are accepted"
    gzip -t -- "$file" || fail "gzip integrity check failed: $file"
    # Read the complete gzip stream. Exiting a grep pipeline on the first match
    # can SIGPIPE gunzip under `set -o pipefail` and reject a healthy backup.
    gunzip -c -- "$file" | awk '
        /^(-- MySQL dump|CREATE TABLE|INSERT INTO|DROP TABLE)/ { found = 1 }
        END { exit(found ? 0 : 1) }
    ' >/dev/null || fail "backup does not contain recognizable MySQL SQL"
}

backup_mysql() {
    ensure_backup_directory
    prepare_mysql_credentials
    local final_file="${BACKUP_DIR}/${DB_NAME}_${TIMESTAMP}.sql.gz"
    local partial_file="${final_file}.partial"
    local error_file
    error_file="$(mktemp "${TMPDIR:-/tmp}/ryframe-mysqldump.XXXXXX.log")"

    log "backing up MySQL database ${DB_NAME} from ${DB_HOST}:${DB_PORT}"
    if ! mysqldump \
        --defaults-extra-file="$MYSQL_DEFAULTS_FILE" \
        --protocol=TCP \
        --single-transaction \
        --quick \
        --routines \
        --triggers \
        --events \
        --hex-blob \
        --set-gtid-purged=OFF \
        --default-character-set=utf8mb4 \
        "$DB_NAME" 2>"$error_file" | gzip -9 >"$partial_file"; then
        log "mysqldump failed; diagnostic follows" >&2
        sed -E 's/(password=)[^[:space:]]+/\1[REDACTED]/Ig' "$error_file" >&2
        rm -f -- "$partial_file" "$error_file"
        return 1
    fi
    rm -f -- "$error_file"
    chmod 600 "$partial_file"
    mv -- "$partial_file" "$final_file"
    validate_backup "$final_file"
    ln -sfn -- "$(basename "$final_file")" "${BACKUP_DIR}/${DB_NAME}_latest.sql.gz"
    BACKUP_RESULT="$final_file"
    log "backup complete: ${final_file}"
}

upload_backup() {
    local file="$1"
    [[ "$S3_BACKUP_ENABLED" == "true" ]] || return 0
    local endpoint_args=()
    if [[ -n "$S3_ENDPOINT" ]]; then
        endpoint_args=(--endpoint-url "$S3_ENDPOINT")
    fi
    aws "${endpoint_args[@]}" s3 cp --no-progress --only-show-errors \
        "$file" "s3://${S3_BUCKET}/$(basename "$file")"
    log "uploaded backup to s3://${S3_BUCKET}/$(basename "$file")"
}

clean_old_backups() {
    local retention="${1:-$BACKUP_RETENTION}"
    validate_positive_integer "$retention" "retention"
    ensure_backup_directory
    find "$BACKUP_DIR" -maxdepth 1 -type f -name '*.sql.gz' -mtime "+${retention}" \
        -print -delete
}

list_backups() {
    ensure_backup_directory
    find "$BACKUP_DIR" -maxdepth 1 -type f -name '*.sql.gz' \
        -printf '%TY-%Tm-%TdT%TH:%TM:%TSZ %10s %f\n' | sort -r
}

restore_into() {
    local file="$1"
    local target_database="$2"
    validate_database_name "$target_database"
    validate_backup "$file"
    prepare_mysql_credentials
    gunzip -c -- "$file" | mysql_client "$target_database"
}

restore_backup() {
    local file="${1:-}"
    local confirm_flag="${2:-}"
    local expected_name="${3:-}"
    [[ -n "$file" ]] || fail "restore requires a backup file"
    [[ "$confirm_flag" == "--confirm" ]] \
        || fail "restore requires: --confirm <expected-database-name>"
    [[ "$expected_name" == "$DB_NAME" ]] \
        || fail "confirmation database name does not match DB_NAME"
    restore_into "$file" "$DB_NAME"
    log "restore complete for database ${DB_NAME}"
}

rehearse_restore() {
    local file="${1:-}"
    [[ -n "$file" ]] || fail "rehearse requires a backup file"
    prepare_mysql_credentials
    local rehearsal_database="ryframe_restore_${TIMESTAMP//[^A-Za-z0-9]/_}_$$"
    validate_database_name "$rehearsal_database"

    mysql_client -e "CREATE DATABASE \`${rehearsal_database}\` CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci"
    local rehearsal_created=true
    drop_rehearsal_database() {
        if [[ "$rehearsal_created" == "true" ]]; then
            mysql_client -e "DROP DATABASE IF EXISTS \`${rehearsal_database}\`" || true
        fi
    }
    trap 'drop_rehearsal_database; cleanup' EXIT INT TERM

    restore_into "$file" "$rehearsal_database"
    local table_count
    table_count="$(mysql_client --batch --skip-column-names -e \
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema='${rehearsal_database}'")"
    [[ "$table_count" =~ ^[0-9]+$ && "$table_count" -gt 0 ]] \
        || fail "restore rehearsal produced no tables"
    drop_rehearsal_database
    rehearsal_created=false
    trap cleanup EXIT INT TERM
    log "restore rehearsal passed (${table_count} tables)"
}

show_help() {
    cat <<'HELP'
Usage:
  ./deploy.sh backup
  ./deploy.sh validate FILE.sql.gz
  ./deploy.sh rehearse FILE.sql.gz
  ./deploy.sh restore FILE.sql.gz --confirm DATABASE_NAME
  ./deploy.sh list
  ./deploy.sh clean [DAYS]

MySQL connection: DB_HOST, DB_PORT, DB_NAME, DB_USER, DB_PASSWORD.
Backups are permission-restricted .sql.gz files; credentials are never passed
on the process command line.
HELP
}

main() {
    validate_database_name "$DB_NAME"
    validate_positive_integer "$DB_PORT" "DB_PORT"
    validate_positive_integer "$BACKUP_RETENTION" "BACKUP_RETENTION"

    case "${1:-help}" in
        help|-h|--help)
            show_help
            return
            ;;
    esac

    check_dependencies

    case "${1:-help}" in
        backup)
            backup_mysql
            upload_backup "$BACKUP_RESULT"
            clean_old_backups "$BACKUP_RETENTION"
            ;;
        validate)
            validate_backup "${2:-}"
            log "backup validation passed"
            ;;
        rehearse)
            rehearse_restore "${2:-}"
            ;;
        restore)
            restore_backup "${2:-}" "${3:-}" "${4:-}"
            ;;
        list)
            list_backups
            ;;
        clean)
            clean_old_backups "${2:-$BACKUP_RETENTION}"
            ;;
        *)
            show_help >&2
            fail "unknown command: $1"
            ;;
    esac
}

main "$@"
