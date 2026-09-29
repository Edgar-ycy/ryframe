use std::path::Path;

use serde_json::{Map, Value};

use crate::{
    Result,
    cli::{
        FixtureSuccessorArm, FixtureSuccessorCommand, FixtureSuccessorGeneration,
        FixtureSuccessorRelationship,
    },
    local_test_path::{LocalTestPathKind, validate_absolute_path},
};

use super::{insert_path, output_value, path_text, path_value};

pub(super) fn fields(command: &FixtureSuccessorCommand, root: &Path) -> Result<Map<String, Value>> {
    match command {
        FixtureSuccessorCommand::Help => Err("successor 帮助没有私有请求".into()),
        FixtureSuccessorCommand::Relationship(options) => relationship(options, root),
        FixtureSuccessorCommand::GenerationRequest(options) => generation(options, root),
        FixtureSuccessorCommand::ArmRequest(options) => arm(options, root),
    }
}

fn relationship(options: &FixtureSuccessorRelationship, root: &Path) -> Result<Map<String, Value>> {
    let mut fields = Map::new();
    for (name, value) in [
        ("source_result", options.source_result.as_path()),
        ("predecessor_review", options.predecessor_review.as_path()),
        ("predecessor_request", options.predecessor_request.as_path()),
        ("successor_review", options.successor_review.as_path()),
        ("seed_request", options.seed_request.as_path()),
        ("base_request", options.base_request.as_path()),
        ("candidate_request", options.candidate_request.as_path()),
    ] {
        insert_path(
            &mut fields,
            name,
            value,
            root,
            LocalTestPathKind::ExistingFile,
        )?;
    }
    fields.insert("id".to_owned(), Value::String(options.id.clone()));
    fields.insert(
        "output".to_owned(),
        output_value(&options.output, root, "successor relationship 输出")?,
    );
    Ok(fields)
}

fn generation(options: &FixtureSuccessorGeneration, root: &Path) -> Result<Map<String, Value>> {
    let mut fields = Map::new();
    for (name, value) in [
        ("successor", options.successor.as_path()),
        ("backend_build", options.backend_build.as_path()),
        ("maintenance_build", options.maintenance_build.as_path()),
        ("source_environment", options.source_environment.as_path()),
    ] {
        insert_path(
            &mut fields,
            name,
            value,
            root,
            LocalTestPathKind::ExistingFile,
        )?;
    }
    for (name, value) in [
        ("source_backend", Some(&options.source_backend)),
        ("product_backend", options.product_backend.as_ref()),
    ] {
        if let Some(value) = value {
            validate_absolute_path(value, LocalTestPathKind::ExistingDirectory)?;
            fields.insert(
                name.to_owned(),
                Value::String(path_text(value, name)?.to_owned()),
            );
        }
    }
    fields.insert(
        "expected_head".to_owned(),
        Value::String(options.expected_head.clone()),
    );
    fields.insert("id".to_owned(), Value::String(options.id.clone()));
    if let Some(value) = &options.adapter_contract {
        fields.insert("adapter_contract".to_owned(), Value::String(value.clone()));
    }
    if let Some(output) = &options.output {
        fields.insert(
            "output".to_owned(),
            output_value(output, root, "generation-request 输出")?,
        );
    }
    Ok(fields)
}

fn arm(options: &FixtureSuccessorArm, root: &Path) -> Result<Map<String, Value>> {
    let mut fields = Map::new();
    for (name, value) in [
        ("successor", options.successor.as_path()),
        (
            "source_export_result",
            options.source_export_result.as_path(),
        ),
    ] {
        insert_path(
            &mut fields,
            name,
            value,
            root,
            LocalTestPathKind::ExistingFile,
        )?;
    }
    insert_path(
        &mut fields,
        "workspace",
        &options.workspace,
        root,
        LocalTestPathKind::ExistingDirectory,
    )?;
    fields.insert("id".to_owned(), Value::String(options.id.clone()));
    fields.insert(
        "side".to_owned(),
        Value::String(options.side.as_str().to_owned()),
    );
    fields.insert(
        "copy_directory".to_owned(),
        path_value(
            &options.copy_directory,
            root,
            LocalTestPathKind::NewDirectory,
            "arm 复制目录",
        )?,
    );
    if let Some(output) = &options.output {
        fields.insert(
            "output".to_owned(),
            output_value(output, root, "arm-request 输出")?,
        );
    }
    Ok(fields)
}
