use super::diagnostic::{Context, Writes};
use super::{Command, bootstrap, private_artifact};
use ryframe_config::{AppConfig, Environment};
use std::{
    error::Error,
    path::{Path, PathBuf},
};

const USAGE: &str = "用法：cargo migrate -- control export-sql --output <源码检出之外的新私有绝对路径> [--plan|--write]";

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

pub(super) fn run(directory: &Path, output: &Path, write: bool) -> Result<(), Box<dyn Error>> {
    if !write {
        println!(
            "计划：从控制库初始化定义导出完整 SQL 至 {}；依赖=显式管理员密码；未读取密码、配置或数据库，未写入文件。实际导出需 --write",
            output.display()
        );
        return Ok(());
    }
    let initial = Context {
        directory,
        environment: None,
        config: None,
        command: "control export-sql",
        target: None,
    };
    let environment = Environment::from_env().map_err(|_| {
        initial.error(
            "APP_ENV 选择",
            Writes::None,
            &std::io::Error::other("APP_ENV 必须为 dev、test 或 prod"),
        )
    })?;
    let initial = Context {
        environment: Some(environment),
        ..initial
    };
    let config = AppConfig::load(directory, environment)
        .map_err(|error| initial.error("配置加载", Writes::None, &error))?;
    let context = Context {
        config: Some(&config),
        ..initial
    };
    let modules = ryframe::business::registered_business_modules()
        .map_err(|error| context.error("业务模块登记", Writes::None, &error))?;
    let migrations = modules
        .iter()
        .flat_map(|module| module.migrations())
        .map(|migration| migration.as_ref())
        .collect::<Vec<_>>();
    let mut phase = "初始化密码哈希";
    let sql = (|| {
        let hashes = bootstrap::load_hashes(&config)?.ok_or(
            "缺少 config.bootstrap.admin_password；在选定配置文件中填写后重新导出，尚未写入文件",
        )?;
        phase = "初始化 SQL 离线生成";
        Ok::<_, Box<dyn Error>>(ryframe::business::shared_control_initialization_sql(
            config.scope_id.as_str(),
            &hashes,
            &migrations,
        )?)
    })()
    .map_err(|error| context.error(phase, Writes::None, error.as_ref()))?;
    let output = private_artifact::output(output).map_err(|error| {
        context.error(
            &format!("导出路径核验（{}）", output.display()),
            Writes::File,
            error.as_ref(),
        )
    })?;
    private_artifact::publish(&output, sql.as_bytes()).map_err(|error| {
        context.error(
            &format!("导出文件发布（{}）", output.display()),
            Writes::File,
            error.as_ref(),
        )
    })?;
    println!(
        "完整初始化 SQL 已导出：{}。使用遇错即停模式导入空 MySQL 数据库，再执行 control verify 与 tenant-data verify --target shared-control，无需补 up；独立 MySQL 目标另行迁移；文件含密码哈希，请按凭据保管。",
        output.display()
    );
    Ok(())
}
