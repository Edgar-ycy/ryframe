use super::{Command, bootstrap};
use std::{
    error::Error,
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
};

const USAGE: &str = "用法：cargo migrate -- control export-sql --output .local-tests/control-init.sql [--plan|--write]";

pub(super) fn parse(args: &[String]) -> Result<Command, Box<dyn Error>> {
    let mut output = None;
    let mut write = false;
    let mut plan = false;
    let mut values = args.iter();
    while let Some(value) = values.next() {
        match value.as_str() {
            "--output" if output.is_none() => output = values.next().map(PathBuf::from),
            "--write" if !write => write = true,
            "--plan" if !plan => plan = true,
            _ => return Err(format!("未知或重复参数：{value}；{USAGE}").into()),
        }
    }
    if write && plan {
        return Err("--plan 与 --write 不能同时使用".into());
    }
    Ok(Command::ExportSql {
        output: output.ok_or(USAGE)?,
        write,
    })
}

pub(super) fn run(output: &Path, write: bool) -> Result<(), Box<dyn Error>> {
    if !write {
        println!(
            "计划：从控制库初始化定义导出完整 SQL 至 {}；依赖=显式管理员密码；未读取密码、配置或数据库，未写入文件。实际导出需 --write",
            output.display()
        );
        return Ok(());
    }
    let modules = ryframe::business::registered_business_modules()?;
    let migrations = modules
        .iter()
        .flat_map(|module| module.migrations())
        .filter(|migration| migration.scope() == ryframe_sdk::BusinessMigrationScope::Control)
        .map(|migration| migration.as_ref())
        .collect::<Vec<_>>();
    let business = ryframe_sdk::business_initialization_statements(&migrations)?;
    let hashes = bootstrap::load_hashes()?.ok_or(
        "缺少初始化管理员密码；设置 RYFRAME_INIT_ADMIN_PASSWORD 或 RYFRAME_INIT_ADMIN_PASSWORD_FILE 后重新导出，尚未写入文件",
    )?;
    let output = private_output(output)?;
    let sql = ryframe_db::migration::mysql_initialization_sql_with_business(&hashes, &business);
    publish(&output, sql.as_bytes())?;
    println!(
        "完整初始化 SQL 已导出：{}。使用遇错即停模式导入空 MySQL 数据库，再执行 cargo migrate -- control verify；文件含密码哈希，请按凭据保管。",
        output.display()
    );
    Ok(())
}

fn private_output(output: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let current = std::env::current_dir()?;
    let root = current
        .ancestors()
        .find(|path| path.join("Cargo.toml").is_file() && path.join("crates/ryframe").is_dir())
        .ok_or("请在后端 Cargo Workspace 内执行导出")?;
    let private = root.join(".local-tests");
    let output = if output.is_absolute() {
        output.to_path_buf()
    } else {
        root.join(output)
    };
    if output
        .components()
        .any(|part| matches!(part, Component::ParentDir))
        || !output.starts_with(&private)
        || output == private
    {
        return Err(
            "初始化 SQL 含部署凭据，只允许输出到当前后端 .local-tests 下，且路径不能包含 ..".into(),
        );
    }
    for path in output.ancestors().take_while(|path| *path != root) {
        if let Ok(metadata) = fs::symlink_metadata(path) {
            if metadata.file_type().is_symlink() {
                return Err("导出路径不允许符号链接".into());
            }
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err("导出路径不允许重解析点".into());
                }
            }
        }
    }
    if output.exists() {
        return Err(format!(
            "目标文件已存在：{}；请选择新路径，避免覆盖已有初始化凭据",
            output.display()
        )
        .into());
    }
    fs::create_dir_all(output.parent().ok_or("输出文件缺少父目录")?)?;
    Ok(output)
}

fn publish(output: &Path, bytes: &[u8]) -> Result<(), Box<dyn Error>> {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let temporary = output.with_extension(format!("{}-{nonce}.partial", std::process::id()));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    let result = (|| -> Result<(), Box<dyn Error>> {
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::hard_link(&temporary, output)?;
        Ok(())
    })();
    fs::remove_file(&temporary)?;
    result
}
