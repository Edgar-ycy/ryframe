#[derive(Clone, Copy)]
pub(super) struct EnvOverride {
    pub(super) name: &'static str,
    pub(super) path: &'static [&'static str],
    pub(super) value_type: EnvValueType,
}

impl EnvOverride {
    const fn string(name: &'static str, path: &'static [&'static str]) -> Self {
        Self::new(name, path, EnvValueType::String)
    }

    const fn integer(name: &'static str, path: &'static [&'static str]) -> Self {
        Self::new(name, path, EnvValueType::Integer)
    }

    const fn boolean(name: &'static str, path: &'static [&'static str]) -> Self {
        Self::new(name, path, EnvValueType::Bool)
    }

    const fn string_array(name: &'static str, path: &'static [&'static str]) -> Self {
        Self::new(name, path, EnvValueType::StringArray)
    }

    const fn json(name: &'static str, path: &'static [&'static str]) -> Self {
        Self::new(name, path, EnvValueType::Json)
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
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum EnvValueType {
    String,
    Integer,
    Bool,
    StringArray,
    Json,
}

pub(super) const ENV_OVERRIDES: &[EnvOverride] = &[
    EnvOverride::string("APP_APP_NAME", &["app", "name"]),
    EnvOverride::string("APP_APP_VERSION", &["app", "version"]),
    EnvOverride::string("APP_APP_HOST", &["app", "host"]),
    EnvOverride::integer("APP_APP_PORT", &["app", "port"]),
    EnvOverride::string("APP_DATABASE_SQL_LOG_LEVEL", &["database", "sql_log_level"]),
    EnvOverride::string("APP_DATABASE_HOST", &["database", "primary", "host"]),
    EnvOverride::integer("APP_DATABASE_PORT", &["database", "primary", "port"]),
    EnvOverride::string("APP_DATABASE_NAME", &["database", "primary", "database"]),
    EnvOverride::string(
        "APP_DATABASE_USERNAME",
        &["database", "primary", "username"],
    ),
    EnvOverride::string(
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
    EnvOverride::json("APP_DATABASE_REPLICAS", &["database", "replicas"]),
    EnvOverride::json("APP_DATABASE_SOURCES", &["database", "sources"]),
    EnvOverride::string("APP_GENERATOR_DATA_SOURCE", &["generator", "data_source"]),
    EnvOverride::string("APP_AUTH_JWT_SECRET", &["auth", "jwt_secret"]),
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
    EnvOverride::string("APP_REDIS_PASSWORD", &["redis", "password"]),
    EnvOverride::integer("APP_REDIS_DATABASE", &["redis", "database"]),
    EnvOverride::integer("APP_REDIS_MAX_POOL_SIZE", &["redis", "max_pool_size"]),
    EnvOverride::integer("APP_REDIS_TIMEOUT_SECS", &["redis", "timeout_secs"]),
    EnvOverride::string("APP_LOGGER_LEVEL", &["logger", "level"]),
    EnvOverride::string("APP_LOGGER_FORMAT", &["logger", "format"]),
    EnvOverride::string("APP_LOGGER_OUTPUT", &["logger", "output"]),
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
    EnvOverride::integer(
        "APP_RATE_LIMIT_REFILL_PER_SEC",
        &["rate_limit", "refill_per_sec"],
    ),
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
    EnvOverride::string("APP_OBJECT_STORAGE_BACKEND", &["object_storage", "backend"]),
    EnvOverride::string(
        "APP_OBJECT_STORAGE_LOCAL_BASE_DIR",
        &["object_storage", "local_base_dir"],
    ),
    EnvOverride::string(
        "APP_OBJECT_STORAGE_ENDPOINT",
        &["object_storage", "endpoint"],
    ),
    EnvOverride::string(
        "APP_OBJECT_STORAGE_ACCESS_KEY",
        &["object_storage", "access_key"],
    ),
    EnvOverride::string(
        "APP_OBJECT_STORAGE_SECRET_KEY",
        &["object_storage", "secret_key"],
    ),
    EnvOverride::boolean("APP_OBJECT_STORAGE_USE_SSL", &["object_storage", "use_ssl"]),
    EnvOverride::string("APP_OBJECT_STORAGE_REGION", &["object_storage", "region"]),
];

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::ENV_OVERRIDES;

    #[test]
    fn override_names_and_paths_are_unique_and_non_empty() {
        let mut names = HashSet::new();
        let mut paths = HashSet::new();

        for spec in ENV_OVERRIDES {
            assert!(names.insert(spec.name), "duplicate override: {}", spec.name);
            assert!(!spec.path.is_empty(), "empty path: {}", spec.name);
            assert!(
                spec.path.iter().all(|segment| !segment.is_empty()),
                "empty path segment: {}",
                spec.name
            );
            assert!(
                paths.insert(spec.path),
                "duplicate override path: {}",
                spec.name
            );
        }
    }
}
