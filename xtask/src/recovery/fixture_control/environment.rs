use std::path::Path;

use serde_json::{Map, Value};

use crate::{
    Result,
    cli::{FixtureEnvironmentCommand, FixtureEnvironmentInputs},
    local_test_path::LocalTestPathKind,
};

use super::{insert_optional_path, insert_path, output_value, path_value};

pub(super) fn fields(
    command: &FixtureEnvironmentCommand,
    root: &Path,
) -> Result<Map<String, Value>> {
    let mut fields = Map::new();
    match command {
        FixtureEnvironmentCommand::Help => return Err("environment 帮助没有私有请求".into()),
        FixtureEnvironmentCommand::Plan(inputs) => insert_inputs(&mut fields, inputs, root)?,
        FixtureEnvironmentCommand::Prepare {
            inputs,
            output,
            secrets_dir,
        } => {
            insert_inputs(&mut fields, inputs, root)?;
            fields.insert(
                "output".to_owned(),
                path_value(
                    output,
                    root,
                    LocalTestPathKind::NewDirectory,
                    "环境输出目录",
                )?,
            );
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
        FixtureEnvironmentCommand::RotateSecrets { fixture, output } => {
            insert_path(
                &mut fields,
                "fixture",
                fixture,
                root,
                LocalTestPathKind::ExistingFile,
            )?;
            fields.insert(
                "output".to_owned(),
                path_value(
                    output,
                    root,
                    LocalTestPathKind::NewDirectory,
                    "secret set 输出目录",
                )?,
            );
        }
        FixtureEnvironmentCommand::BootstrapSecrets {
            source_fixture,
            source_secrets,
            fixture,
        } => {
            insert_path(
                &mut fields,
                "source_fixture",
                source_fixture,
                root,
                LocalTestPathKind::ExistingFile,
            )?;
            insert_path(
                &mut fields,
                "source_secrets",
                source_secrets,
                root,
                LocalTestPathKind::ExistingDirectory,
            )?;
            insert_path(
                &mut fields,
                "fixture",
                fixture,
                root,
                LocalTestPathKind::ExistingFile,
            )?;
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
        ("fixture", inputs.fixture.as_path()),
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
