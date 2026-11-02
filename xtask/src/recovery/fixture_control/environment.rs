use std::path::Path;

use serde_json::{Map, Value};

use crate::{Result, cli::FixtureEnvironmentInputs, local_test_path::LocalTestPathKind};

use super::{insert_path, path_text};

#[path = "environment/configuration.rs"]
mod configuration;
#[path = "environment/guards.rs"]
mod guards;
#[path = "environment/output.rs"]
mod output;
#[path = "environment/plan.rs"]
pub(in crate::recovery) mod plan;
#[path = "environment/prepare.rs"]
pub(super) mod prepare;
#[path = "environment/protocol.rs"]
pub(crate) mod protocol;
#[path = "environment/review.rs"]
pub(super) mod review;

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
