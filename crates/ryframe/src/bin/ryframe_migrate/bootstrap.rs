use ryframe_auth::password;
use ryframe_config::AppConfig;
use ryframe_db::migration::BootstrapHashes;
use std::error::Error;

/// 仅显式初始化使用选定配置内的管理员密码，不回显密码或计算结果。
pub(super) fn load_hashes(config: &AppConfig) -> Result<Option<BootstrapHashes>, Box<dyn Error>> {
    let Some(value) = config
        .bootstrap
        .admin_password
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    password::validate_complexity(value)
        .map_err(|_| "config.bootstrap.admin_password 不满足密码复杂度要求")?;
    let admin = password::hash(value)
        .map_err(|_| "无法计算 config.bootstrap.admin_password 的初始化哈希")?;
    let user =
        password::hash(password::INITIAL_USER_PASSWORD).map_err(|_| "无法计算初始用户密码哈希")?;
    Ok(Some(BootstrapHashes::new(admin, user)?))
}
