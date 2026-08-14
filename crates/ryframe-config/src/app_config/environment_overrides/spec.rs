#[derive(Clone, Copy)]
pub(super) struct EnvOverride {
    pub(super) name: &'static str,
    pub(super) path: &'static [&'static str],
    pub(super) value_type: EnvValueType,
    pub(super) allow_file: bool,
}

impl EnvOverride {
    const fn string(name: &'static str, path: &'static [&'static str]) -> Self {
        Self::new(name, path, EnvValueType::String)
    }

    const fn integer(name: &'static str, path: &'static [&'static str]) -> Self {
        Self::new(name, path, EnvValueType::Integer)
    }

    const fn float(name: &'static str, path: &'static [&'static str]) -> Self {
        Self::new(name, path, EnvValueType::Float)
    }

    const fn boolean(name: &'static str, path: &'static [&'static str]) -> Self {
        Self::new(name, path, EnvValueType::Bool)
    }

    const fn string_array(name: &'static str, path: &'static [&'static str]) -> Self {
        Self::new(name, path, EnvValueType::StringArray)
    }

    const fn secret(name: &'static str, path: &'static [&'static str]) -> Self {
        Self::new_with_file(name, path, EnvValueType::String)
    }

    const fn json_file(name: &'static str, path: &'static [&'static str]) -> Self {
        Self::new_with_file(name, path, EnvValueType::Json)
    }

    const fn new(
        name: &'static str,
        path: &'static [&'static str],
        value_type: EnvValueType,
    ) -> Self {
        Self {
            name,
            path,
            value_type,
            allow_file: false,
        }
    }

    const fn new_with_file(
        name: &'static str,
        path: &'static [&'static str],
        value_type: EnvValueType,
    ) -> Self {
        Self {
            name,
            path,
            value_type,
            allow_file: true,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum EnvValueType {
    String,
    Integer,
    Float,
    Bool,
    StringArray,
    Json,
}

pub(super) const ENV_OVERRIDES: &[EnvOverride] = &[
    EnvOverride::string("APP_APP_NAME", &["app", "name"]),
    EnvOverride::string("APP_APP_HOST", &["app", "host"]),
    EnvOverride::integer("APP_APP_PORT", &["app", "port"]),
    EnvOverride::boolean("APP_MULTI_TENANCY_ENABLED", &["multi_tenancy", "enabled"]),
    EnvOverride::boolean("APP_API_DOCS_ENABLED", &["api_docs", "enabled"]),
    EnvOverride::secret(
        "APP_MONITOR_METRICS_BEARER_TOKEN",
        &["monitor", "metrics_bearer_token"],
    ),
    EnvOverride::string("APP_DATABASE_SQL_LOG_LEVEL", &["database", "sql_log_level"]),
    EnvOverride::integer(
        "APP_DATABASE_SQL_SLOW_THRESHOLD_MS",
        &["database", "sql_slow_threshold_ms"],
    ),
    EnvOverride::string(
        "APP_DATABASE_MIGRATION_MODE",
        &["database", "migration_mode"],
    ),
    EnvOverride::string("APP_JOBS_MODE", &["jobs", "mode"]),
    EnvOverride::integer("APP_JOBS_POLL_INTERVAL_MS", &["jobs", "poll_interval_ms"]),
    EnvOverride::integer(
        "APP_JOBS_MAX_IDLE_POLL_INTERVAL_MS",
        &["jobs", "max_idle_poll_interval_ms"],
    ),
    EnvOverride::integer("APP_JOBS_LEASE_SECONDS", &["jobs", "lease_seconds"]),
    EnvOverride::integer("APP_JOBS_HEARTBEAT_SECONDS", &["jobs", "heartbeat_seconds"]),
    EnvOverride::integer(
        "APP_JOBS_DEFAULT_MAX_ATTEMPTS",
        &["jobs", "default_max_attempts"],
    ),
    EnvOverride::integer("APP_JOBS_EXPORT_MAX_ROWS", &["jobs", "export_max_rows"]),
    EnvOverride::integer(
        "APP_JOBS_EXPORT_RETENTION_HOURS",
        &["jobs", "export_retention_hours"],
    ),
    EnvOverride::integer("APP_JOBS_CONCURRENCY", &["jobs", "concurrency"]),
    EnvOverride::string("APP_JOBS_WORKER_ID", &["jobs", "worker_id"]),
    EnvOverride::string("APP_JOBS_HEALTH_HOST", &["jobs", "health_host"]),
    EnvOverride::integer("APP_JOBS_HEALTH_PORT", &["jobs", "health_port"]),
    EnvOverride::integer(
        "APP_JOBS_LEASE_RECOVERY_INTERVAL_SECONDS",
        &["jobs", "lease_recovery_interval_seconds"],
    ),
    EnvOverride::boolean("APP_JOBS_SCHEDULER_ENABLED", &["jobs", "scheduler_enabled"]),
    EnvOverride::integer(
        "APP_JOBS_SCHEDULER_POLL_INTERVAL_MS",
        &["jobs", "scheduler_poll_interval_ms"],
    ),
    EnvOverride::integer(
        "APP_JOBS_SCHEDULER_BATCH_SIZE",
        &["jobs", "scheduler_batch_size"],
    ),
    EnvOverride::integer(
        "APP_JOBS_MAX_ENABLED_SCHEDULES_PER_TENANT",
        &["jobs", "max_enabled_schedules_per_tenant"],
    ),
    EnvOverride::integer(
        "APP_DATA_RETENTION_CLEANUP_BATCH_SIZE",
        &["data_retention", "cleanup_batch_size"],
    ),
    EnvOverride::integer(
        "APP_DATA_RETENTION_MAX_ROWS_PER_RESOURCE_PER_RUN",
        &["data_retention", "max_rows_per_resource_per_run"],
    ),
    EnvOverride::integer(
        "APP_DATA_RETENTION_BACKGROUND_JOB_SUCCEEDED_DAYS",
        &["data_retention", "background_job_succeeded_days"],
    ),
    EnvOverride::integer(
        "APP_DATA_RETENTION_OUTBOX_PUBLISHED_DAYS",
        &["data_retention", "outbox_published_days"],
    ),
    EnvOverride::integer(
        "APP_DATA_RETENTION_SCHEDULE_EXECUTION_DAYS",
        &["data_retention", "schedule_execution_days"],
    ),
    EnvOverride::integer(
        "APP_DATA_RETENTION_EXPORT_JOB_HISTORY_DAYS",
        &["data_retention", "export_job_history_days"],
    ),
    EnvOverride::integer(
        "APP_DATA_RETENTION_OPERATION_LOG_DAYS",
        &["data_retention", "operation_log_days"],
    ),
    EnvOverride::integer(
        "APP_DATA_RETENTION_LOGIN_LOG_DAYS",
        &["data_retention", "login_log_days"],
    ),
    EnvOverride::integer(
        "APP_DATA_RETENTION_USER_IMPORT_HISTORY_DAYS",
        &["data_retention", "user_import_history_days"],
    ),
    EnvOverride::integer(
        "APP_DATA_RETENTION_USER_IMPORT_ARTIFACT_HOURS",
        &["data_retention", "user_import_artifact_hours"],
    ),
    EnvOverride::integer(
        "APP_DATA_RETENTION_RETENTION_RUN_DAYS",
        &["data_retention", "retention_run_days"],
    ),
    EnvOverride::integer(
        "APP_DATA_RETENTION_SERVICE_ACCESS_AUDIT_DAYS",
        &["data_retention", "service_access_audit_days"],
    ),
    EnvOverride::boolean(
        "APP_SERVICE_ACCOUNTS_ENABLED",
        &["service_accounts", "enabled"],
    ),
    EnvOverride::string(
        "APP_SERVICE_ACCOUNTS_PEPPER_KEYRING_FILE",
        &["service_accounts", "pepper_keyring_file"],
    ),
    EnvOverride::integer(
        "APP_SERVICE_ACCOUNTS_ACTIVE_PEPPER_VERSION",
        &["service_accounts", "active_pepper_version"],
    ),
    EnvOverride::integer(
        "APP_SERVICE_ACCOUNTS_MAX_ACTIVE_CREDENTIALS",
        &["service_accounts", "max_active_credentials"],
    ),
    EnvOverride::integer(
        "APP_SERVICE_ACCOUNTS_MAX_CREDENTIAL_DAYS",
        &["service_accounts", "max_credential_days"],
    ),
    EnvOverride::integer(
        "APP_SERVICE_ACCOUNTS_DEFAULT_DELEGATION_HOURS",
        &["service_accounts", "default_delegation_hours"],
    ),
    EnvOverride::integer(
        "APP_SERVICE_ACCOUNTS_MAX_DELEGATION_DAYS",
        &["service_accounts", "max_delegation_days"],
    ),
    EnvOverride::integer(
        "APP_SERVICE_ACCOUNTS_DEFAULT_REQUESTS_PER_MINUTE",
        &["service_accounts", "default_requests_per_minute"],
    ),
    EnvOverride::integer(
        "APP_SERVICE_ACCOUNTS_MAX_CONCURRENT_QUERIES",
        &["service_accounts", "max_concurrent_queries"],
    ),
    EnvOverride::integer(
        "APP_SERVICE_ACCOUNTS_QUERY_TIMEOUT_MS",
        &["service_accounts", "query_timeout_ms"],
    ),
    EnvOverride::integer(
        "APP_SERVICE_ACCOUNTS_MAX_PAGE_SIZE",
        &["service_accounts", "max_page_size"],
    ),
    EnvOverride::integer(
        "APP_SERVICE_ACCOUNTS_MAX_RESPONSE_BYTES",
        &["service_accounts", "max_response_bytes"],
    ),
    EnvOverride::integer(
        "APP_USER_IMPORT_MAX_FILE_BYTES",
        &["user_import", "max_file_bytes"],
    ),
    EnvOverride::integer("APP_USER_IMPORT_MAX_ROWS", &["user_import", "max_rows"]),
    EnvOverride::integer("APP_USER_IMPORT_BATCH_SIZE", &["user_import", "batch_size"]),
    EnvOverride::integer(
        "APP_USER_IMPORT_MAX_ACTIVE_PER_TENANT",
        &["user_import", "max_active_per_tenant"],
    ),
    EnvOverride::integer(
        "APP_USER_IMPORT_HASH_PARALLELISM",
        &["user_import", "hash_parallelism"],
    ),
    EnvOverride::integer(
        "APP_TENANT_CONFIG_TRANSFER_MAX_PACKAGE_BYTES",
        &["tenant_config_transfer", "max_package_bytes"],
    ),
    EnvOverride::integer(
        "APP_TENANT_CONFIG_TRANSFER_MAX_UNCOMPRESSED_BYTES",
        &["tenant_config_transfer", "max_uncompressed_bytes"],
    ),
    EnvOverride::integer(
        "APP_TENANT_CONFIG_TRANSFER_MAX_ITEMS",
        &["tenant_config_transfer", "max_items"],
    ),
    EnvOverride::integer(
        "APP_TENANT_CONFIG_TRANSFER_ARTIFACT_HOURS",
        &["tenant_config_transfer", "artifact_hours"],
    ),
    EnvOverride::integer(
        "APP_TENANT_CONFIG_TRANSFER_ROLLBACK_HOURS",
        &["tenant_config_transfer", "rollback_hours"],
    ),
    EnvOverride::integer(
        "APP_TENANT_CONFIG_TRANSFER_LEASE_SECONDS",
        &["tenant_config_transfer", "lease_seconds"],
    ),
    EnvOverride::integer(
        "APP_TENANT_CONFIG_TRANSFER_MAX_RUNTIME_SECONDS",
        &["tenant_config_transfer", "max_runtime_seconds"],
    ),
    EnvOverride::boolean("APP_TELEMETRY_ENABLED", &["telemetry", "enabled"]),
    EnvOverride::string("APP_TELEMETRY_ENDPOINT", &["telemetry", "endpoint"]),
    EnvOverride::string("APP_TELEMETRY_SERVICE_NAME", &["telemetry", "service_name"]),
    EnvOverride::float("APP_TELEMETRY_SAMPLE_RATIO", &["telemetry", "sample_ratio"]),
    EnvOverride::integer(
        "APP_TELEMETRY_EXPORT_TIMEOUT_SECS",
        &["telemetry", "export_timeout_secs"],
    ),
    EnvOverride::integer(
        "APP_TELEMETRY_MAX_QUEUE_SIZE",
        &["telemetry", "max_queue_size"],
    ),
    EnvOverride::boolean("APP_MESSAGING_ENABLED", &["messaging", "enabled"]),
    EnvOverride::integer(
        "APP_MESSAGING_TICKET_TTL_SECONDS",
        &["messaging", "ticket_ttl_seconds"],
    ),
    EnvOverride::integer(
        "APP_MESSAGING_RETENTION_DAYS",
        &["messaging", "retention_days"],
    ),
    EnvOverride::integer(
        "APP_MESSAGING_MAX_CONNECTIONS_PER_USER",
        &["messaging", "max_connections_per_user"],
    ),
    EnvOverride::integer(
        "APP_MESSAGING_OUTBOUND_BUFFER",
        &["messaging", "outbound_buffer"],
    ),
    EnvOverride::integer(
        "APP_MESSAGING_MAX_RECIPIENTS_PER_MESSAGE",
        &["messaging", "max_recipients_per_message"],
    ),
    EnvOverride::integer(
        "APP_MESSAGING_REPLAY_INTERVAL_SECONDS",
        &["messaging", "replay_interval_seconds"],
    ),
    EnvOverride::integer(
        "APP_MESSAGING_REPLAY_JITTER_SECONDS",
        &["messaging", "replay_jitter_seconds"],
    ),
    EnvOverride::integer(
        "APP_MESSAGING_REPLAY_BATCH_SIZE",
        &["messaging", "replay_batch_size"],
    ),
    EnvOverride::string("APP_DATABASE_HOST", &["database", "primary", "host"]),
    EnvOverride::integer("APP_DATABASE_PORT", &["database", "primary", "port"]),
    EnvOverride::string("APP_DATABASE_NAME", &["database", "primary", "database"]),
    EnvOverride::string(
        "APP_DATABASE_USERNAME",
        &["database", "primary", "username"],
    ),
    EnvOverride::secret(
        "APP_DATABASE_PASSWORD",
        &["database", "primary", "password"],
    ),
    EnvOverride::integer(
        "APP_DATABASE_MAX_CONNECTIONS",
        &["database", "primary", "max_connections"],
    ),
    EnvOverride::integer(
        "APP_DATABASE_MIN_CONNECTIONS",
        &["database", "primary", "min_connections"],
    ),
    EnvOverride::integer(
        "APP_DATABASE_ACQUIRE_TIMEOUT_SECS",
        &["database", "primary", "acquire_timeout_secs"],
    ),
    EnvOverride::integer(
        "APP_DATABASE_IDLE_TIMEOUT_SECS",
        &["database", "primary", "idle_timeout_secs"],
    ),
    EnvOverride::integer(
        "APP_DATABASE_MAX_LIFETIME_SECS",
        &["database", "primary", "max_lifetime_secs"],
    ),
    EnvOverride::integer(
        "APP_DATABASE_CONNECT_TIMEOUT_SECS",
        &["database", "primary", "connect_timeout_secs"],
    ),
    EnvOverride::string(
        "APP_DATABASE_TLS_MODE",
        &["database", "primary", "tls_mode"],
    ),
    EnvOverride::string("APP_DATABASE_TLS_CA", &["database", "primary", "tls_ca"]),
    EnvOverride::string(
        "APP_DATABASE_TLS_CLIENT_CERT",
        &["database", "primary", "tls_client_cert"],
    ),
    EnvOverride::string(
        "APP_DATABASE_TLS_CLIENT_KEY",
        &["database", "primary", "tls_client_key"],
    ),
    EnvOverride::json_file("APP_DATABASE_REPLICAS", &["database", "replicas"]),
    EnvOverride::json_file("APP_DATABASE_SOURCES", &["database", "sources"]),
    EnvOverride::string("APP_GENERATOR_DATA_SOURCE", &["generator", "data_source"]),
    EnvOverride::secret("APP_AUTH_JWT_SECRET", &["auth", "jwt_secret"]),
    EnvOverride::string(
        "APP_AUTH_ACCESS_TOKEN_EXPIRE",
        &["auth", "access_token_expire"],
    ),
    EnvOverride::string(
        "APP_AUTH_REFRESH_TOKEN_EXPIRE",
        &["auth", "refresh_token_expire"],
    ),
    EnvOverride::integer(
        "APP_AUTH_MAX_LOGIN_ATTEMPTS",
        &["auth", "max_login_attempts"],
    ),
    EnvOverride::integer(
        "APP_AUTH_LOCKOUT_DURATION_MINUTES",
        &["auth", "lockout_duration_minutes"],
    ),
    EnvOverride::string("APP_REDIS_MODE", &["redis", "mode"]),
    EnvOverride::string("APP_REDIS_HOST", &["redis", "host"]),
    EnvOverride::integer("APP_REDIS_PORT", &["redis", "port"]),
    EnvOverride::secret("APP_REDIS_PASSWORD", &["redis", "password"]),
    EnvOverride::integer("APP_REDIS_DATABASE", &["redis", "database"]),
    EnvOverride::integer("APP_REDIS_MAX_POOL_SIZE", &["redis", "max_pool_size"]),
    EnvOverride::integer("APP_REDIS_TIMEOUT_SECS", &["redis", "timeout_secs"]),
    EnvOverride::boolean("APP_REDIS_TLS", &["redis", "tls"]),
    EnvOverride::string("APP_REDIS_TLS_CA", &["redis", "tls_ca"]),
    EnvOverride::string("APP_REDIS_TLS_CLIENT_CERT", &["redis", "tls_client_cert"]),
    EnvOverride::string("APP_REDIS_TLS_CLIENT_KEY", &["redis", "tls_client_key"]),
    EnvOverride::string("APP_LOGGER_LEVEL", &["logger", "level"]),
    EnvOverride::string("APP_LOGGER_FORMAT", &["logger", "format"]),
    EnvOverride::string("APP_LOGGER_OUTPUT", &["logger", "output"]),
    EnvOverride::integer("APP_LOGGER_RETENTION_DAYS", &["logger", "retention_days"]),
    EnvOverride::string_array("APP_CORS_ALLOW_ORIGINS", &["cors", "allow_origins"]),
    EnvOverride::string_array("APP_PROXY_TRUSTED_CIDRS", &["proxy", "trusted_cidrs"]),
    EnvOverride::integer("APP_UPLOAD_FILE_MAX_BYTES", &["upload", "file_max_bytes"]),
    EnvOverride::integer(
        "APP_UPLOAD_AVATAR_MAX_BYTES",
        &["upload", "avatar_max_bytes"],
    ),
    EnvOverride::integer(
        "APP_UPLOAD_MULTIPART_ENVELOPE_BYTES",
        &["upload", "multipart_envelope_bytes"],
    ),
    EnvOverride::integer(
        "APP_UPLOAD_TIMEOUT_SECONDS",
        &["upload", "upload_timeout_seconds"],
    ),
    EnvOverride::integer(
        "APP_UPLOAD_API_TIMEOUT_SECONDS",
        &["upload", "api_timeout_seconds"],
    ),
    EnvOverride::boolean("APP_RATE_LIMIT_ENABLED", &["rate_limit", "enabled"]),
    EnvOverride::integer("APP_RATE_LIMIT_CAPACITY", &["rate_limit", "capacity"]),
    EnvOverride::integer("APP_RATE_LIMIT_WINDOW_SECS", &["rate_limit", "window_secs"]),
    EnvOverride::boolean(
        "APP_RATE_LIMIT_ENABLE_USER_RATE_LIMIT",
        &["rate_limit", "enable_user_rate_limit"],
    ),
    EnvOverride::integer(
        "APP_RATE_LIMIT_USER_WINDOW_SECS",
        &["rate_limit", "user_window_secs"],
    ),
    EnvOverride::integer(
        "APP_RATE_LIMIT_USER_CAPACITY",
        &["rate_limit", "user_capacity"],
    ),
    EnvOverride::integer(
        "APP_RATE_LIMIT_API_WINDOW_SECS",
        &["rate_limit", "api_window_secs"],
    ),
    EnvOverride::integer(
        "APP_PAGINATION_DEFAULT_PAGE_SIZE",
        &["pagination", "default_page_size"],
    ),
    EnvOverride::integer(
        "APP_PAGINATION_MAX_PAGE_SIZE",
        &["pagination", "max_page_size"],
    ),
    EnvOverride::string("APP_OBJECT_STORAGE_BACKEND", &["object_storage", "backend"]),
    EnvOverride::string(
        "APP_OBJECT_STORAGE_LOCAL_BASE_DIR",
        &["object_storage", "local_base_dir"],
    ),
    EnvOverride::boolean(
        "APP_OBJECT_STORAGE_ALLOW_LOCAL_IN_PRODUCTION",
        &["object_storage", "allow_local_in_production"],
    ),
    EnvOverride::string(
        "APP_OBJECT_STORAGE_ENDPOINT",
        &["object_storage", "endpoint"],
    ),
    EnvOverride::secret(
        "APP_OBJECT_STORAGE_ACCESS_KEY",
        &["object_storage", "access_key"],
    ),
    EnvOverride::secret(
        "APP_OBJECT_STORAGE_SECRET_KEY",
        &["object_storage", "secret_key"],
    ),
    EnvOverride::boolean("APP_OBJECT_STORAGE_USE_SSL", &["object_storage", "use_ssl"]),
    EnvOverride::string("APP_OBJECT_STORAGE_REGION", &["object_storage", "region"]),
];
