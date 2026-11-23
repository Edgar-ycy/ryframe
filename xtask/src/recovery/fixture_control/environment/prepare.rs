//! 显式准备隔离环境；只写受控本地文件，成功前不发布 bootstrap。
use super::{configuration, output, plan, protocol::check};
use crate::{
    Result,
    cli::FixtureEnvironmentCommand,
    local_test_path::same_local_test_file,
    recovery::clone::native::{
        digest, json as document, maintenance, target_verify::input::request_review::Kind,
    },
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};
#[path = "prepare/files.rs"]
mod files;
#[path = "prepare/render.rs"]
mod render;

pub(in crate::recovery::fixture_control) fn run(
    root: &Path,
    command: &FixtureEnvironmentCommand,
) -> Result<Value> {
    run_with(root, command, |_| Ok(()))
}

fn run_with(
    root: &Path,
    command: &FixtureEnvironmentCommand,
    mut after_render: impl FnMut(&Path) -> Result<()>,
) -> Result<Value> {
    let FixtureEnvironmentCommand::Prepare {
        inputs,
        output,
        secrets_dir,
    } = command
    else {
        return Err("不是环境准备操作".into());
    };
    let output = output_path(root, output)?;
    let mut fields = serde_json::Map::new();
    super::insert_inputs(&mut fields, inputs, root)?;
    fields.insert("backend".into(), json!(root));
    let planned = plan::create(&Value::Object(fields))?;
    let captured = plan::inputs(&json!({"backend":root,"plan":planned}))?;
    if Kind::read(&captured["inputs"][1])? == Kind::B1RuntimeMeasurement {
        check(secrets_dir.is_some(), "B1 测量必须显式指定 --secrets-dir")?;
    }
    output::request(&json!({"backend":root,"plan":planned,"output":output}))?;
    create_output(&output)?;
    let result = files::private_permissions(&output, true).and_then(|()| {
        prepare(
            root,
            &planned,
            &captured,
            &output,
            secrets_dir.as_deref(),
            &mut after_render,
        )
    });
    finish(&output, result)?;
    Ok(json!({"status":"prepared","services_started":false,"remote_writes":0}))
}

fn output_path(root: &Path, output: &Path) -> Result<PathBuf> {
    let absolute = if output.is_absolute() {
        output.to_owned()
    } else {
        root.join(output)
    };
    let value = super::super::path_value(
        &absolute,
        root,
        crate::local_test_path::LocalTestPathKind::NewDirectory,
        "环境输出目录",
    )?;
    Ok(PathBuf::from(value.as_str().ok_or("环境输出路径缺失")?))
}

fn create_output(output: &Path) -> Result<()> {
    fs::create_dir(output).map_err(|error| {
        format!("环境准备阶段“独占创建输出”失败：{}；未取得目录所有权，不写失败标记；保留现场，禁止覆盖或自动重放", safe_reason(&error)).into()
    })
}

// 仅在 create_output 成功取得本次目录后调用，禁止给他人的目录写失败标记。
fn finish(output: &Path, result: Result<Value>) -> Result<Value> {
    let error = match result {
        Ok(value) => return Ok(value),
        Err(error) => error,
    };
    let marker = if output.join("bootstrap.json").exists() {
        "已存在 bootstrap，保留并核查".to_owned()
    } else {
        match document::write_new(
            &output.join("failed.json"),
            &json!({"status":"failed","services_started":false,"remote_writes":0}),
        ) {
            Ok(()) => "已同步".to_owned(),
            Err(error) => format!("未能落盘（{}）", safe_reason(error.as_ref())),
        }
    };
    Err(format!("环境准备阶段“私有配置与输入后验发布”失败：{}；本地文件保留，失败标记{marker}；无远端写入或服务启动，禁止覆盖或自动重放", safe_reason(error.as_ref())).into())
}

fn safe_reason(error: &(dyn std::error::Error + 'static)) -> String {
    let mut current = Some(error);
    for _ in 0..16 {
        let Some(error) = current else {
            break;
        };
        if let Some(error) = error.downcast_ref::<super::protocol::GuardError>() {
            return error.0.to_owned();
        }
        if let Some(error) = error.downcast_ref::<std::io::Error>() {
            return format!(
                "I/O 操作失败（kind={:?}，raw_os_error={:?}）",
                error.kind(),
                error.raw_os_error()
            );
        }
        current = error.source();
    }
    "上游核验未完成，细因待检查".to_owned()
}
fn prepare(
    root: &Path,
    planned: &Value,
    captured: &Value,
    output: &Path,
    selected_secrets: Option<&Path>,
    after_render: &mut impl FnMut(&Path) -> Result<()>,
) -> Result<Value> {
    let values = &captured["inputs"];
    let review = &values[1];
    let publication = &values[3];
    let execution = Path::new(
        publication["published"]["backend"]["path"]
            .as_str()
            .ok_or("执行树缺失")?,
    );
    let original = Path::new(
        publication["original"]["paths"]["backend"]
            .as_str()
            .ok_or("原始来源缺失")?,
    );
    let kind = Kind::read(review)?;
    let (source, secrets) = open_secrets(root, original, execution, kind, selected_secrets)?;
    let side = planned["side"].as_str().ok_or("夹具侧缺失")?;
    let selected = &review["scopes"][side];
    let mysql = mysql_path(selected)?;
    if let Some(source) = &source {
        files::copy_mysql(root, &source.directory.join("mysql-client.cnf"), &mysql)?;
    } else {
        check(
            same_local_test_file(root, &mysql, &secrets.directory.join("mysql-client.cnf"))?,
            "B1 四库必须直接绑定本次私有秘密集中的 MySQL 客户端配置",
        )?;
    }
    let mysql_binding = files::binding(&mysql)?;
    let database = selected["databases"]
        .as_array()
        .ok_or("数据库目标缺失")?
        .iter()
        .find(|item| item["key"] == "shared-control")
        .ok_or("共享控制库缺失")?;
    let credentials = json!({"backend":root,"database":{"defaults_file":mysql,
        "defaults_sha256":mysql_binding["sha256"],"database":database["database"]}});
    let client = crate::recovery::source::evidence::binding::summary(&credentials)?;
    if kind == Kind::B1RuntimeMeasurement {
        check(
            client["host"] == database["host"] && client["port"] == database["port"],
            "B1 MySQL 客户端必须连接审阅登记的独立实例",
        )?;
    }
    let settings = configuration::settings(&json!({"selected":selected,"client":client,
        "values":secrets.values()?,"side":side}))?;
    render::configuration(execution, output, &settings, &credentials)?;
    let administrator = configuration::administrator(
        &json!({"admin_password":secrets.values()?["admin_password"]}),
    )?;
    files::write_private(
        &output.join("administrator.toml"),
        administrator["text"]
            .as_str()
            .ok_or("管理员配置缺失")?
            .as_bytes(),
    )?;
    after_render(output)?;
    publish(
        root,
        planned,
        captured,
        output,
        execution,
        (source.as_ref(), &secrets),
        (&mysql, mysql_binding),
    )
}

fn open_secrets(
    root: &Path,
    original: &Path,
    execution: &Path,
    kind: Kind,
    selected: Option<&Path>,
) -> Result<(Option<files::Secrets>, files::Secrets)> {
    match kind {
        Kind::Comparison => Ok((
            Some(files::Secrets::open(root, original, None)?),
            files::Secrets::open(root, original, selected)?,
        )),
        Kind::B1RuntimeMeasurement => {
            let selected = selected.ok_or("B1 测量必须显式指定 --secrets-dir")?;
            let selected = if selected.is_absolute() {
                selected.to_owned()
            } else {
                root.join(selected)
            };
            let default = execution.join(".local-tests/reference-fixture/secrets");
            check(
                !default.try_exists()?
                    || fs::canonicalize(&default)? != fs::canonicalize(&selected)?,
                "B1 测量须使用独立秘密集，不能选择默认 MySQL 凭据",
            )?;
            Ok((
                None,
                files::Secrets::open(root, execution, Some(&selected))?,
            ))
        }
    }
}

fn publish(
    root: &Path,
    planned: &Value,
    captured: &Value,
    output: &Path,
    execution: &Path,
    secret_sets: (Option<&files::Secrets>, &files::Secrets),
    mysql: (&Path, Value),
) -> Result<Value> {
    let (source, secrets) = secret_sets;
    let (mysql, mysql_binding) = mysql;
    let values = &captured["inputs"];
    fs::create_dir(output.join("tmp"))?;
    let environment = json!({"APP_ENV":"test","TEMP":output.join("tmp"),"TMP":output.join("tmp")});
    let configuration = render::digest(execution, "test", &output.join("config"))?;
    let maintenance_file = Path::new(values[4].as_str().ok_or("维护收据缺失")?);
    let verified = maintenance::verify(execution, maintenance_file)?;
    check(
        plan::inputs(&json!({"backend":root,"plan":planned}))? == *captured,
        "夹具计划输入在准备期间变化",
    )?;
    if let Some(source) = source {
        source.check()?;
    }
    secrets.check()?;
    check(
        files::binding(mysql)? == mysql_binding,
        "MySQL 凭据在准备期间变化",
    )?;
    let mut secret_files = secrets.bindings.clone();
    secret_files["mysql-client.cnf"] = mysql_binding;
    document::write_new(
        &output.join("environment.json"),
        &json!({"environment":environment}),
    )?;
    let receipt = json!({"format_version":1,"kind":"reference-fixture-environment","status":"prepared",
        "plan":planned,"execution_backend":execution,"configuration_sha256":configuration,"config_dir":output.join("config"),
        "administrator_credentials":files::binding(&output.join("administrator.toml"))?,
        "maintenance":files::binding(maintenance_file)?,"maintenance_source":verified["source"],
        "secret_files":secret_files,"environment_sha256":digest::plan_hash(&environment)?,
        "services_started":false,"remote_writes":0,"historical_data_used":false});
    document::write_new(&output.join("bootstrap.json"), &receipt)?;
    Ok(receipt)
}

fn mysql_path(selected: &Value) -> Result<PathBuf> {
    let databases = selected["databases"].as_array().ok_or("数据库目标缺失")?;
    let entry = databases
        .iter()
        .find(|item| item["key"] == "shared-control")
        .ok_or("共享控制库缺失")?;
    let path = entry["connection_file"]
        .as_str()
        .ok_or("MySQL 凭据路径缺失")?;
    check(
        databases.iter().all(|item| item["connection_file"] == path),
        "同侧四库必须使用同一冻结 MySQL 凭据",
    )?;
    Ok(PathBuf::from(path))
}

#[cfg(test)]
#[path = "../../../../tests/unit/recovery/fixture_control/environment/prepare.rs"]
mod tests;
