use super::{
    fixture::{MODELS, prepare},
    snapshot,
};
use ryframe_generator::command::{GenerationBuilder, GenerationOperation};
use std::{fs, path::Path};

#[path = "conflicts.rs"]
mod conflicts;

pub(super) fn verify_conflicts(frontend_source: &Path, target_dir: &Path, cases: &[(&str, &str)]) {
    let backend_before =
        snapshot::SourceSnapshot::capture(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
            .unwrap();
    let frontend_before = snapshot::SourceSnapshot::capture(frontend_source).unwrap();
    let (_backend_scope, _frontend_scope, backend, frontend) =
        prepare(&backend_before, &frontend_before, target_dir);
    GenerationBuilder::new(&backend)
        .operation(GenerationOperation::NewBusiness("pipeline-business".into()))
        .write(true)
        .plan()
        .unwrap()
        .execute()
        .unwrap();
    let model = backend.join("crates/pipeline-business/src/resources/mod.rs");
    fs::write(&model, MODELS).unwrap();
    let generate = |sync| {
        GenerationBuilder::new(&backend)
            .operation(GenerationOperation::Resource)
            .package("pipeline-business")
            .sync_frontend(sync)
            .write(true)
            .plan()?
            .execute()
    };
    // 仅构建一次有效后端 baseline；不执行原前端正链、重复生成或 TS 故障链。
    generate(false).unwrap();
    fs::write(
        model,
        MODELS.replace(
            "pub name: String,",
            "pub name: String,\n    pub note: Option<String>,",
        ),
    )
    .unwrap();
    conflicts::verify(
        &backend,
        Some(&frontend),
        "pipeline_business",
        cases,
        |sync| {
            let output = std::process::Command::new(env!("CARGO_BIN_EXE_ryframe-generate"))
                .args([
                    "resource",
                    "--package",
                    "pipeline-business",
                    "--write",
                    if sync {
                        "--sync-frontend"
                    } else {
                        "--no-sync-frontend"
                    },
                ])
                .current_dir(backend.join("crates/pipeline-business/src"))
                .output()
                .map_err(|error| error.to_string())?;
            if output.status.success() {
                Ok(())
            } else {
                Err(format!(
                    "{}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ))
            }
        },
    );
    backend_before.verify().unwrap();
    frontend_before.verify().unwrap();
}
