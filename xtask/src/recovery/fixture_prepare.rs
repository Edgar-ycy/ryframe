use std::path::Path;

use serde_json::json;

use crate::{
    Result,
    cli::{FIXTURE_CONTROL_PROTOCOL_KIND, FixturePrepareCommand, FixturePrepareOptions},
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    process::run_with_env_removed,
};

const SCRIPT: &str = "tools/python/prepare_full_stack_fixture.py";
const PROTOCOL_KEY: &str = "RYFRAME_REFERENCE_FIXTURE_CONTROL_PROTOCOL";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrivateFixturePrepareInvocation {
    pub(crate) script: &'static str,
    pub(crate) protocol: String,
}

pub(super) fn run(command: &FixturePrepareCommand, root: &Path, frontend: &Path) -> Result<()> {
    let FixturePrepareCommand::Run(options) = command else {
        println!(
            "cargo xtask check recovery fixture --output-dir <绝对新目录> \
             [--expected-backend-sha <SHA> --expected-frontend-sha <SHA>] --write"
        );
        return Ok(());
    };
    let invocation = private_invocation_at(options, root, frontend)?;
    run_with_env_removed(
        root,
        "python",
        &["-B", invocation.script],
        &[(PROTOCOL_KEY, invocation.protocol.as_str())],
        &[PROTOCOL_KEY],
    )
}

pub(crate) fn private_invocation_at(
    options: &FixturePrepareOptions,
    root: &Path,
    frontend: &Path,
) -> Result<PrivateFixturePrepareInvocation> {
    validate_local_test_path(&options.output_dir, root, LocalTestPathKind::NewDirectory)
        .map_err(|error| format!("Device 夹具输出目录无效：{error}"))?;
    validate_frontend(frontend)?;
    let mut protocol = json!({
        "backend_dir": path_text(root, "后端目录")?,
        "domain": "prepare",
        "format_version": 1,
        "frontend_dir": path_text(frontend, "前端目录")?,
        "kind": FIXTURE_CONTROL_PROTOCOL_KIND,
        "operation": "prepare",
        "output_dir": path_text(&options.output_dir, "输出目录")?,
        "write": true,
    });
    if let Some(sources) = &options.expected_sources {
        protocol["expected_backend_sha"] = json!(sources.backend_sha);
        protocol["expected_frontend_sha"] = json!(sources.frontend_sha);
    }
    let serialized = serde_json::to_string(&protocol)?;
    if serialized.len() > 32 * 1024 || serialized.contains(['\n', '\r', '\0']) {
        return Err("Device 夹具私有协议过长或包含换行符/NUL".into());
    }
    Ok(PrivateFixturePrepareInvocation {
        script: SCRIPT,
        protocol: serialized,
    })
}

fn validate_frontend(frontend: &Path) -> Result<()> {
    if !frontend.is_absolute() {
        return Err("Device 夹具前端目录必须是绝对路径".into());
    }
    let metadata = std::fs::symlink_metadata(frontend)
        .map_err(|error| format!("无法核验 Device 夹具前端目录：{error}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("Device 夹具前端目录必须是现有真实目录".into());
    }
    if !frontend.join("package.json").is_file() {
        return Err("Device 夹具前端目录缺少 package.json".into());
    }
    Ok(())
}

fn path_text<'a>(value: &'a Path, label: &str) -> Result<&'a str> {
    value
        .to_str()
        .ok_or_else(|| format!("{label}必须能表示为 UTF-8").into())
}
