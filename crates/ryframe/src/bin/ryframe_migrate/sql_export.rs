use super::{Command, bootstrap, private_artifact};
use std::{
    error::Error,
    path::{Path, PathBuf},
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
    let output = private_artifact::output(output)?;
    let sql = ryframe_db::migration::mysql_initialization_sql_with_business(&hashes, &business);
    private_artifact::publish(&output, sql.as_bytes())?;
    println!(
        "完整初始化 SQL 已导出：{}。使用遇错即停模式导入空 MySQL 数据库，再执行 cargo migrate -- control verify；文件含密码哈希，请按凭据保管。",
        output.display()
    );
    Ok(())
}
