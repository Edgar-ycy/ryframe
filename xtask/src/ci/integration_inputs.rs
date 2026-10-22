use std::{fs, path::Path};

use ryframe_config::{AppConfig, DbTlsMode, Environment};

use crate::Result;

/// 显式真实协议门禁必须具有完整输入，不能把测试自行跳过当作验收成功。
pub(crate) fn require_service_inputs(root: &Path) -> Result<()> {
    let mysql_directory = root.join(".local-tests/mysql/config");
    let redis_directory = root.join(".local-tests/service-tests/redis/config");
    let mysql = load_service(&mysql_directory)?;
    if mysql.database.primary.tls_mode == DbTlsMode::Disabled {
        return Err(format!(
            "真实协议门禁配置 {} 的 database.primary.tls_mode 禁用了 TLS；请配置 required、verify_ca 或 verify_identity，本次尚未运行服务测试",
            mysql_directory.display()
        )
        .into());
    }
    let redis = load_service(&redis_directory)?;
    if redis.redis.is_none() {
        return Err(format!(
            "真实协议门禁配置 {} 缺少 redis 分组；请填写隔离 Redis 的连接配置，本次尚未运行服务测试",
            redis_directory.display()
        )
        .into());
    }
    Ok(())
}

fn load_service(directory: &Path) -> Result<AppConfig> {
    for path in directory.ancestors().collect::<Vec<_>>().into_iter().rev() {
        check_path(path, true)?;
    }
    for name in ["app.toml", "app.test.toml"] {
        check_path(&directory.join(name), false)?;
    }
    AppConfig::load(directory, Environment::Test).map_err(|error| {
        format!(
            "真实协议门禁输入校验失败：{}；请修正该文件配置，本次尚未运行服务测试",
            error.message()
        )
        .into()
    })
}

fn check_path(path: &Path, directory: bool) -> Result<()> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        format!(
            "无法检查真实协议门禁输入 {}：IOkind={:?}，rawcode={:?}；请创建缺失目录及 app.toml/app.test.toml 或修正权限，本次尚未运行服务测试",
            path.display(),
            error.kind(),
            error.raw_os_error()
        )
    })?;
    let linked = metadata.file_type().is_symlink();
    #[cfg(windows)]
    let linked = {
        use std::os::windows::fs::MetadataExt;
        linked || metadata.file_attributes() & 0x400 != 0
    };
    if linked || (directory && !metadata.is_dir()) || (!directory && !metadata.is_file()) {
        return Err(format!(
            "真实协议门禁输入 {} 必须为普通无链接{}；请保留原对象并显式选择隔离配置，本次尚未运行服务测试",
            path.display(),
            if directory { "目录" } else { "文件" }
        )
        .into());
    }
    Ok(())
}
