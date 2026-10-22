use std::path::{Path, PathBuf};

use files::{Image, diagnostic};
use serde::Deserialize;

#[path = "integration_configuration/capture.rs"]
pub(crate) mod capture;
#[path = "integration_configuration/files.rs"]
pub(crate) mod files;
#[path = "integration_configuration/owned.rs"]
pub(crate) mod owned;

// 私有控制文件，不打印反序列化对象或原始 TOML。
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    format_version: u64,
    run_id: String,
    artifact_root: PathBuf,
    redis_config_dir: PathBuf,
    openssl_program: PathBuf,
    #[serde(default = "default_jobs")]
    test_jobs: usize,
    #[serde(default = "default_timeout")]
    timeout_secs: u64,
}

pub(crate) struct IntegrationConfiguration {
    pub(crate) run_id: String,
    pub(crate) artifact_root: PathBuf,
    pub(crate) redis_config_dir: PathBuf,
    pub(crate) openssl_program: PathBuf,
    pub(crate) test_jobs: usize,
    pub(crate) timeout: std::time::Duration,
    pub(crate) redis: capture::Captured,
    path: PathBuf,
    image: Image,
}

impl IntegrationConfiguration {
    pub(crate) fn load(backend_root: &Path) -> crate::Result<Self> {
        files::require_directory(backend_root)?;
        let path = backend_root.join(".local-tests/integration/ci.toml");
        let image = files::read_required(&path)?;
        let mut settings = parse_settings(&image.bytes, &path)?;
        validate_settings(&settings, &path)?;
        settings.artifact_root = resolve(
            backend_root,
            &settings.artifact_root,
            &path,
            "artifact_root",
        )?;
        settings.redis_config_dir = resolve(
            backend_root,
            &settings.redis_config_dir,
            &path,
            "redis_config_dir",
        )?;
        if settings.openssl_program.as_os_str() != std::ffi::OsStr::new("openssl") {
            settings.openssl_program = resolve(
                backend_root,
                &settings.openssl_program,
                &path,
                "openssl_program",
            )?;
            validate_program(&settings.openssl_program)?;
        }
        // 允许产物目录尚不存在，但存在的路径各层必须是普通目录。
        files::directory_available(&settings.artifact_root)?;
        let redis = capture::Captured::load(&settings.redis_config_dir, false)?;
        validate_redis(&redis)?;
        let input = Self {
            run_id: settings.run_id,
            artifact_root: settings.artifact_root,
            redis_config_dir: settings.redis_config_dir,
            openssl_program: settings.openssl_program,
            test_jobs: settings.test_jobs,
            timeout: std::time::Duration::from_secs(settings.timeout_secs),
            redis,
            path,
            image,
        };
        input.verify()?;
        Ok(input)
    }

    pub(crate) fn verify(&self) -> Result<(), String> {
        if files::read_required(&self.path)? != self.image {
            return Err(diagnostic(
                &self.path,
                "复核CI控制文件",
                "来源漂移，不重新捕获",
            ));
        }
        if self.redis_config_dir != self.redis.directory {
            return Err(diagnostic(
                &self.path,
                "复核redis_config_dir",
                "公开路径与已捕获来源不一致",
            ));
        }
        self.redis.verify()
    }
}

fn parse_settings(bytes: &[u8], path: &Path) -> Result<Settings, String> {
    let table: toml::Table = files::parse(bytes, path)?;
    let required = [
        "format_version",
        "run_id",
        "artifact_root",
        "redis_config_dir",
        "openssl_program",
    ];
    let optional = ["test_jobs", "timeout_secs"];
    for key in required {
        if !table.contains_key(key) {
            return Err(diagnostic(
                path,
                key,
                "缺少必需字段；请补充该字段，不使用环境回退",
            ));
        }
    }
    if table
        .keys()
        .any(|key| !required.contains(&key.as_str()) && !optional.contains(&key.as_str()))
    {
        return Err(diagnostic(
            path,
            "字段集合",
            "存在未允许字段；仅支持format_version/run_id/artifact_root/redis_config_dir/openssl_program/test_jobs/timeout_secs",
        ));
    }
    for key in [
        "run_id",
        "artifact_root",
        "redis_config_dir",
        "openssl_program",
    ] {
        if !table[key].is_str() {
            return Err(diagnostic(path, key, "需要TOML字符串；请修正字段类型"));
        }
    }
    for key in ["format_version", "test_jobs", "timeout_secs"] {
        if let Some(value) = table.get(key)
            && !value.as_integer().is_some_and(|value| value >= 0)
        {
            return Err(diagnostic(path, key, "需要非负TOML整数；请按字段范围修正"));
        }
    }
    table.try_into().map_err(|_| {
        diagnostic(
            path,
            "字段类型",
            "配置不能转换为当前CI类型；请核对整数范围和字段类型",
        )
    })
}

fn validate_settings(settings: &Settings, path: &Path) -> Result<(), String> {
    if settings.format_version != 1 {
        return Err(diagnostic(
            path,
            "format_version",
            "只接受版本1；请使用当前CI文件格式",
        ));
    }
    if settings.run_id.is_empty()
        || settings.run_id.len() > 64
        || !settings
            .run_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(diagnostic(
            path,
            "run_id",
            "需要1..64位ASCII字母、数字或短横线",
        ));
    }
    if !(1..=64).contains(&settings.test_jobs) || (cfg!(windows) && settings.test_jobs != 4) {
        return Err(diagnostic(path, "test_jobs", "需要1..64；Windows必须为4"));
    }
    if !(1..=3600).contains(&settings.timeout_secs) {
        return Err(diagnostic(path, "timeout_secs", "需要1..3600秒的整数"));
    }
    for (name, value) in [
        ("artifact_root", &settings.artifact_root),
        ("redis_config_dir", &settings.redis_config_dir),
        ("openssl_program", &settings.openssl_program),
    ] {
        if value.as_os_str().is_empty() || value.to_string_lossy().chars().any(char::is_control) {
            return Err(diagnostic(
                path,
                name,
                "不得为空或包含控制字符；请填写明确路径或程序名openssl",
            ));
        }
    }
    Ok(())
}

fn resolve(root: &Path, value: &Path, context: &Path, field: &str) -> Result<PathBuf, String> {
    if value
        .components()
        .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(diagnostic(
            context,
            field,
            "路径不得包含父目录跳转；相对路径以后端根目录解析",
        ));
    }
    Ok(if value.is_absolute() {
        value.to_owned()
    } else {
        root.join(value)
    })
}

fn validate_program(program: &Path) -> Result<(), String> {
    files::require_directory(program.parent().expect("已解析绝对程序路径"))?;
    let metadata = std::fs::symlink_metadata(program)
        .map_err(|error| files::io_error(program, "检查openssl_program", error))?;
    if !files::ordinary(&metadata) || !metadata.is_file() {
        return Err(diagnostic(
            program,
            "检查openssl_program",
            "必须是普通无链接程序文件；仅openssl可显式使用工具PATH",
        ));
    }
    Ok(())
}

fn validate_redis(redis: &capture::Captured) -> Result<(), String> {
    let path = &redis.directory;
    if !redis.redis_endpoint_explicit {
        return Err(diagnostic(
            path,
            "redis.host/redis.port",
            "源文件必须显式给出host和port；不采用默认6379连接",
        ));
    }
    let configured = redis
        .app
        .redis
        .as_ref()
        .ok_or_else(|| diagnostic(path, "redis", "缺少必需的redis配置"))?;
    if configured.mode != ryframe_config::RedisMode::Required {
        return Err(diagnostic(path, "redis.mode", "TLS门禁上游要求required"));
    }
    if configured.tls {
        return Err(diagnostic(
            path,
            "redis.tls",
            "fixture上游只支持普通TCP，不支持TLS上游；不会降级",
        ));
    }
    if !["127.0.0.1", "::1", "localhost"].contains(&configured.host.trim()) {
        return Err(diagnostic(
            path,
            "redis.host",
            "TLS门禁只允许回环Redis；请使用隔离本机端点",
        ));
    }
    Ok(())
}

const fn default_jobs() -> usize {
    4
}
const fn default_timeout() -> u64 {
    900
}

#[cfg(test)]
#[path = "integration_configuration/test_fixture.rs"]
pub(crate) mod test_fixture;

#[cfg(test)]
#[path = "integration_configuration/testing.rs"]
mod testing;
