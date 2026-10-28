//! 从协调器文件配置生成预期命令，不执行解释器或采用收据自报的程序路径。
use super::{check, inputs::Inputs, path};
use crate::{
    Result,
    recovery::{
        clone::native::{digest, json as strict},
        fixture_control::portable_path,
    },
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub(super) fn interpreter(backend: &Path, inputs: &mut Inputs) -> Result<PathBuf> {
    for relative in ["config/tools.toml", ".local-tests/tools.toml"] {
        let file = backend.join(relative);
        super::reads::optional(&file)?;
        if file.try_exists()? {
            inputs.protect(&file)?;
        }
    }
    let settings = ryframe_config::ToolSettings::load(backend)?;
    let program = settings.verification.python_program.ok_or(
        "恢复静态权威要求在 tools.toml 设置 verification.python_program 为实际 Python 程序的绝对路径"
    )?;
    check(
        program.is_absolute(),
        "verification.python_program 必须为实际 Python 程序绝对路径",
    )?;
    path(&json!(program))?;
    check(
        !digest::linked_path(&program)?.contains(&true)
            && std::fs::symlink_metadata(&program)?.is_file(),
        "verification.python_program 必须存在且为无链接的普通实际程序文件",
    )?;
    inputs.protect(&program)?;
    Ok(portable_path(&program.canonicalize()?))
}

fn validate_request(request: &Value) -> Result<()> {
    super::request_shape::validate(request)?;
    strict::exact(
        request,
        &[
            "format_version",
            "kind",
            "authority",
            "registration",
            "roots",
            "paths",
            "digests",
            "artifacts",
            "tools",
            "environment",
        ],
        "恢复启动请求",
    )?;
    check(
        request["format_version"] == 1 && request["kind"] == "restore-runtime-launch-request",
        "恢复启动请求版本或类型不同",
    )?;
    strict::exact(
        &request["tools"],
        &[
            "coordinator_root",
            "coordinator_head",
            "inventory_sha256",
            "frontend_server",
        ],
        "恢复工具绑定",
    )?;
    crate::recovery::inputs::domain::hash_40(
        &request["tools"]["coordinator_head"],
        "恢复协调工具提交",
    )?;
    crate::recovery::inputs::document::digest(
        &request["tools"]["inventory_sha256"],
        "恢复协调工具来源摘要",
    )?;
    let coordinator = path(&request["tools"]["coordinator_root"])?;
    let server = crate::recovery::inputs::document::binding_shape(
        &request["tools"]["frontend_server"],
        "恢复前端服务工具",
    )?;
    check(
        server
            .path
            .file_name()
            .is_some_and(|name| name == "restore_frontend_server.py"),
        "恢复前端服务工具名称无效",
    )?;
    path(&request["tools"]["frontend_server"]["path"])?;
    check(
        same_path(
            &server.path,
            &coordinator.join("tools/python/restore_frontend_server.py"),
        )?,
        "恢复前端服务工具未绑定协调后端",
    )?;
    let actual = digest::file(&server.path)?;
    check(
        actual.bytes == server.bytes && actual.sha256 == server.sha256,
        "恢复前端服务工具字节与启动绑定漂移",
    )?;
    strict::exact(
        &request["environment"],
        &["document", "configurations"],
        "恢复环境绑定",
    )?;
    strict::exact(
        &request["environment"]["configurations"],
        &["api", "worker"],
        "恢复角色配置",
    )?;
    Ok(())
}

pub(super) fn expected(request: &Value, python: &Path) -> Result<Value> {
    validate_request(request)?;
    let endpoint = crate::recovery::inputs::domain::endpoint(
        &request["authority"]["frontend_endpoint"],
        "恢复前端端点",
        Some(""),
    )?;
    let host = endpoint.host;
    let port = endpoint.port.to_string();
    let mut commands = json!({});
    for role in ["api", "worker"] {
        path(&request["artifacts"][role]["executable"])?;
        path(&request["environment"]["configurations"][role]["directory"])?;
        commands[role] = json!([
            request["artifacts"][role]["executable"],
            "--config-dir",
            request["environment"]["configurations"][role]["directory"]
        ]);
    }
    commands["frontend"] = json!([
        python,
        "-X",
        "utf8",
        "-E",
        "-s",
        "-B",
        request["tools"]["frontend_server"]["path"],
        "__serve",
        "--frontend",
        request["roots"]["frontend"],
        "--receipt",
        request["paths"]["frontend_build"],
        "--host",
        host,
        "--port",
        port
    ]);
    Ok(commands)
}

pub(super) fn recorded_spelling(mut expected: Value, actual: &Value) -> Result<Value> {
    strict::exact(actual, &["api", "worker", "frontend"], "恢复命令角色")?;
    for role in ["api", "worker", "frontend"] {
        let wanted = strict::array(&expected[role], "权威预期命令")?;
        let recorded = strict::array(&actual[role], "恢复记录命令")?;
        check(
            wanted.len() == recorded.len(),
            "恢复命令参数数量与独立权威不同",
        )?;
        for (index, (wanted, recorded)) in wanted.iter().zip(recorded).enumerate() {
            let is_path = if role == "frontend" {
                [0, 6, 9, 11].contains(&index)
            } else {
                [0, 2].contains(&index)
            };
            if is_path {
                check(
                    same_path(&path(wanted)?, &path(recorded)?)?,
                    "恢复命令路径与当前独立工具绑定漂移；核对 verification.python_program 和历史 launch",
                )?;
            } else {
                check(wanted == recorded, "恢复命令参数与独立权威不同")?;
            }
        }
        expected[role] = actual[role].clone();
    }
    Ok(expected)
}

pub(crate) fn same_path(left: &Path, right: &Path) -> Result<bool> {
    if left == right {
        return Ok(true);
    }
    let left = portable_path(&left.canonicalize()?);
    let right = portable_path(&right.canonicalize()?);
    Ok(if cfg!(windows) {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    } else {
        left == right
    })
}
