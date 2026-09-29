use std::path::Path;

use serde_json::{Map, Value};

use crate::{
    Result,
    cli::{FixtureDatasetCommand, FixtureDatasetInputs, FixtureSide},
    local_test_path::LocalTestPathKind,
};

use super::{insert_path, output_value, path_value};

pub(super) fn fields(command: &FixtureDatasetCommand, root: &Path) -> Result<Map<String, Value>> {
    let mut fields = Map::new();
    match command {
        FixtureDatasetCommand::Help => return Err("dataset 帮助没有私有请求".into()),
        FixtureDatasetCommand::Plan {
            inputs,
            work_dir,
            output,
        } => {
            insert_inputs(&mut fields, inputs, root)?;
            require_runtime_child(&inputs.runtime, work_dir, "参考数据工作目录")?;
            require_runtime_child(&inputs.runtime, output, "参考数据计划输出")?;
            if work_dir == output {
                return Err("参考数据工作目录与计划输出必须不同".into());
            }
            fields.insert(
                "work_dir".to_owned(),
                path_value(
                    work_dir,
                    root,
                    LocalTestPathKind::NewDirectory,
                    "参考数据工作目录",
                )?,
            );
            fields.insert(
                "output".to_owned(),
                output_value(output, root, "参考数据计划文件")?,
            );
        }
        FixtureDatasetCommand::Prepare { inputs, plan } => {
            insert_inputs(&mut fields, inputs, root)?;
            require_runtime_child(&inputs.runtime, plan, "参考数据计划")?;
            insert_path(
                &mut fields,
                "plan",
                plan,
                root,
                LocalTestPathKind::ExistingFile,
            )?;
        }
    }
    Ok(fields)
}

fn insert_inputs(
    fields: &mut Map<String, Value>,
    inputs: &FixtureDatasetInputs,
    root: &Path,
) -> Result<()> {
    if inputs.side == FixtureSide::Seed {
        return Err("参考数据目标侧只允许 base 或 candidate".into());
    }
    insert_path(
        fields,
        "environment",
        &inputs.environment,
        root,
        LocalTestPathKind::ExistingFile,
    )?;
    insert_path(
        fields,
        "runtime",
        &inputs.runtime,
        root,
        LocalTestPathKind::ExistingDirectory,
    )?;
    fields.insert(
        "side".to_owned(),
        Value::String(inputs.side.as_str().to_owned()),
    );
    Ok(())
}

fn require_runtime_child(runtime: &Path, value: &Path, label: &str) -> Result<()> {
    if value.parent() == Some(runtime) {
        Ok(())
    } else {
        Err(format!("{label}必须是源运行时目录的直接子项").into())
    }
}
