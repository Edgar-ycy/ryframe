use std::path::Path;

use serde_json::{Map, Value};

use crate::{
    Result,
    cli::{FixtureEnvironmentCommand, FixtureEnvironmentInputs},
    local_test_path::LocalTestPathKind,
};

use super::{insert_optional_path, insert_path, output_value, path_text, path_value};

#[path = "environment/configuration.rs"]
mod configuration;
#[path = "environment/guards.rs"]
mod guards;
#[path = "environment/output.rs"]
mod output;
#[path = "environment/plan.rs"]
pub(in crate::recovery) mod plan;
#[path = "environment/protocol.rs"]
pub(crate) mod protocol;

#[path = "environment/secrets.rs"]
pub(crate) mod secrets;

pub(super) fn run_plan(root: &Path, inputs: &FixtureEnvironmentInputs) -> Result<Value> {
    let mut fields = Map::new();
    insert_inputs(&mut fields, inputs, root)?;
    fields.insert(
        "backend".to_owned(),
        Value::String(path_text(root, "后端目录")?.to_owned()),
    );
    plan::create(&Value::Object(fields))?;
    Ok(serde_json::json!({
        "status": "planned",
        "services_started": false,
        "remote_writes": 0,
    }))
}

pub(super) fn fields(
    command: &FixtureEnvironmentCommand,
    root: &Path,
) -> Result<Map<String, Value>> {
    let mut fields = Map::new();
    match command {
        FixtureEnvironmentCommand::Help => return Err("environment 帮助没有私有请求".into()),
        FixtureEnvironmentCommand::Plan(_) => {
            return Err("环境计划由 Rust 原生执行，不生成 Python 请求".into());
        }
        FixtureEnvironmentCommand::Prepare {
            inputs,
            output,
            secrets_dir,
        } => {
            insert_inputs(&mut fields, inputs, root)?;
            let publication_path = Path::new(
                fields["execution_source"]
                    .as_str()
                    .ok_or("执行来源路径缺失")?,
            );
            let publication = super::source_pair::execution::verify(
                root,
                &super::file_binding(publication_path, "执行来源")?,
            )?;
            let output = path_value(
                output,
                root,
                LocalTestPathKind::NewDirectory,
                "环境输出目录",
            )?;
            output::validate(
                root,
                &publication,
                Path::new(output.as_str().ok_or("环境输出路径缺失")?),
            )?;
            fields.insert("output".to_owned(), output);
            insert_optional_path(
                &mut fields,
                "secrets_dir",
                secrets_dir.as_ref(),
                root,
                LocalTestPathKind::ExistingDirectory,
            )?;
        }
        FixtureEnvironmentCommand::Review { review, output } => {
            insert_path(
                &mut fields,
                "review",
                review,
                root,
                LocalTestPathKind::ExistingFile,
            )?;
            fields.insert(
                "output".to_owned(),
                output_value(&output.output, root, "审阅输出文件")?,
            );
        }
        FixtureEnvironmentCommand::RotateSecrets { .. }
        | FixtureEnvironmentCommand::BootstrapSecrets { .. } => {
            return Err("秘密文件操作由 Rust 原生执行".into());
        }
    }
    Ok(fields)
}

fn insert_inputs(
    fields: &mut Map<String, Value>,
    inputs: &FixtureEnvironmentInputs,
    root: &Path,
) -> Result<()> {
    for (name, value) in [
        ("review", inputs.review.as_path()),
        ("execution_source", inputs.execution_source.as_path()),
        ("maintenance_build", inputs.maintenance_build.as_path()),
    ] {
        insert_path(fields, name, value, root, LocalTestPathKind::ExistingFile)?;
    }
    fields.insert(
        "side".to_owned(),
        Value::String(inputs.side.as_str().to_owned()),
    );
    Ok(())
}

pub(in crate::recovery) fn validate_password(password: &str) -> Result<()> {
    guards::reset_password(&serde_json::json!({"admin_password": password}))
}
