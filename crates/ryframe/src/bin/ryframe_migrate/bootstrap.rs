use ryframe_auth::password;
use ryframe_db::migration::BootstrapHashes;
use std::{env, error::Error, fs};

/// 初始管理员密码只在显式初始化时读取，不写入配置或日志。
pub(super) fn load_hashes() -> Result<Option<BootstrapHashes>, Box<dyn Error>> {
    let direct = env::var("RYFRAME_INIT_ADMIN_PASSWORD").ok();
    let file = env::var_os("RYFRAME_INIT_ADMIN_PASSWORD_FILE");
    let value = match (direct, file) {
        (Some(_), Some(_)) => {
            return Err("RYFRAME_INIT_ADMIN_PASSWORD 与 _FILE 只能配置一个".into());
        }
        (Some(value), None) => Some(value),
        (None, Some(path)) => Some(
            fs::read_to_string(path)
                .map_err(
                    |_| "无法读取 RYFRAME_INIT_ADMIN_PASSWORD_FILE；检查路径、UTF-8 编码与读取权限",
                )?
                .trim_end_matches(['\r', '\n'])
                .to_owned(),
        ),
        (None, None) => None,
    };
    value
        .map(|value| {
            password::validate_complexity(&value)?;
            Ok(BootstrapHashes::new(
                password::hash(&value)?,
                password::hash(password::INITIAL_USER_PASSWORD)?,
            )?)
        })
        .transpose()
}
