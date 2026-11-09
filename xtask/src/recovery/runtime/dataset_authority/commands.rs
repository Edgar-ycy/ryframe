//! 从当前协调器和受管构建绑定生成独立的恢复命令。
use super::{check, inputs::Inputs, path};
use crate::{
    Result,
    recovery::{
        clone::native::{digest, json as strict, source},
        fixture_control::portable_path,
        frontend_build::b0,
    },
};
use serde_json::{Value, json};
use std::path::Path;

pub(in crate::recovery::runtime) fn current_tools(
    backend: &Path,
    executable: &Path,
    inputs: &mut Inputs,
) -> Result<Value> {
    let program = path(&json!(portable_path(executable)))?;
    inputs.protect(&program)?;
    let binding = crate::check::python_suite::binding(executable)?
        .ok_or("恢复前端服务要求当前本机协调器的无链接原生程序")?;
    let binding = strict::parse_fragment(&binding, "恢复前端服务工具")?;
    let inventory = source::capture(backend)?;
    check(
        inventory.source.snapshot.clean,
        "恢复运行协调工具必须来自干净工作树",
    )?;
    let inventory_sha256 = digest::plan_hash(&source::inventory_value(&inventory))?;
    Ok(json!({
        "coordinator_root": backend,
        "coordinator_head": inventory.source.snapshot.head,
        "inventory_sha256": inventory_sha256,
        "frontend_server": {"path":binding["path"],"bytes":binding["bytes"],"sha256":binding["sha256"]},
    }))
}

fn validate_request(request: &Value, current: &Value) -> Result<()> {
    validate_request_shape(request)?;
    validate_tools(&request["tools"], current)
}

fn validate_request_shape(request: &Value) -> Result<()> {
    super::request_shape::validate(request)?;
    strict::exact(
        request,
        &[
            "format_version",
            "kind",
            "runtime_directory",
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
        request["format_version"] == 2 && request["kind"] == "restore-runtime-launch-request",
        "恢复启动请求版本或类型不同",
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

pub(super) fn validate_tools(tools: &Value, current: &Value) -> Result<()> {
    let server = tool_descriptor(tools)?;
    let coordinator = path(&tools["coordinator_root"])?;
    check(
        same_path(&coordinator, &path(&current["coordinator_root"])?)?
            && tools["coordinator_head"] == current["coordinator_head"]
            && tools["inventory_sha256"] == current["inventory_sha256"],
        "恢复启动工具未绑定当前协调根、提交和完整来源",
    )?;
    check(
        same_path(&server.path, &path(&current["frontend_server"]["path"])?)?
            && tools["frontend_server"]["bytes"] == current["frontend_server"]["bytes"]
            && tools["frontend_server"]["sha256"] == current["frontend_server"]["sha256"],
        "恢复前端服务工具未绑定当前协调器的原生程序及字节",
    )?;
    Ok(())
}

fn tool_descriptor(tools: &Value) -> Result<crate::recovery::inputs::document::Binding> {
    strict::exact(
        tools,
        &[
            "coordinator_root",
            "coordinator_head",
            "inventory_sha256",
            "frontend_server",
        ],
        "恢复工具绑定",
    )?;
    crate::recovery::inputs::domain::hash_40(&tools["coordinator_head"], "恢复协调工具提交")?;
    crate::recovery::inputs::document::digest(&tools["inventory_sha256"], "恢复协调工具来源摘要")?;
    path(&tools["coordinator_root"])?;
    path(&tools["frontend_server"]["path"])?;
    crate::recovery::inputs::document::binding_shape(&tools["frontend_server"], "恢复前端服务工具")
}

pub(in crate::recovery::runtime) fn verify_direct(
    request: &Value,
    inputs: &mut Inputs,
) -> Result<Value> {
    validate_request_shape(request)?;
    let tools = &request["tools"];
    let server = tool_descriptor(tools)?;
    inputs.protect(&server.path)?;
    let current = crate::check::python_suite::binding(&std::env::current_exe()?)?
        .ok_or("恢复前端服务要求当前正式协调器的原生工具绑定")?;
    let current = strict::parse_fragment(&current, "恢复前端服务工具")?;
    check(
        tools["frontend_server"]
            == json!({"path":current["path"],"bytes":current["bytes"],"sha256":current["sha256"]}),
        "恢复前端服务工具与当前正式协调器绑定不同",
    )?;
    request_commands(request, &tools["frontend_server"]["path"])
}

pub(in crate::recovery::runtime) fn expected(request: &Value, current: &Value) -> Result<Value> {
    validate_request(request, current)?;
    request_commands(request, &current["frontend_server"]["path"])
}

pub(in crate::recovery::runtime) fn b0_runtime_source(authority: &Value) -> bool {
    authority["frontend_sha"] == b0::HEAD
        && authority["backend_adapter_contract"] == "legacy-stable-readiness-b0-v1"
        && authority["backend_product_sha"] == "815c5eafb09d4b493319d255fb7ab88ddd02c8b6"
        && authority["backend_execution_sha"] == "c05114bcdf5c369cd74087db6317ce3c8f89bee8"
}

fn request_commands(request: &Value, frontend_program: &Value) -> Result<Value> {
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
    commands["frontend"] = frontend_command(request, frontend_program)?;
    Ok(commands)
}

pub(super) fn frontend_command(request: &Value, executable: &Value) -> Result<Value> {
    let authority = &request["authority"];
    path(&request["runtime_directory"])?;
    let endpoint = crate::recovery::inputs::domain::endpoint(
        &authority["frontend_endpoint"],
        "恢复前端端点",
        Some(""),
    )?;
    check(
        endpoint.scheme == "http" && endpoint.host == "127.0.0.1",
        "恢复前端服务只接受明确的本机 IPv4 HTTP 端口",
    )?;
    let api = crate::recovery::inputs::domain::endpoint(
        &authority["api_endpoint"],
        "恢复 API 端点",
        Some("/readyz"),
    )?;
    check(
        api.scheme == "http" && api.host == "127.0.0.1" && api.port != endpoint.port,
        "恢复前端代理只接受不同端口的已登记 IPv4 回环 API",
    )?;
    let frontend = authority["frontend_sha"] == b0::HEAD;
    let adapter = authority["backend_adapter_contract"] == "legacy-stable-readiness-b0-v1";
    let b0 = b0_runtime_source(authority);
    check(
        b0 || !(frontend || adapter),
        "恢复 B0 前端服务必须绑定精确历史产品、执行树和适配合同",
    )?;
    let mut command = vec![
        executable.clone(),
        json!("__restore-frontend-serve"),
        json!("--frontend"),
        request["roots"]["frontend"].clone(),
        json!("--receipt"),
        request["paths"]["frontend_build"].clone(),
        json!("--receipt-sha256"),
        request["digests"]["frontend_build"].clone(),
        json!("--expected-head"),
        authority["frontend_sha"].clone(),
        json!("--host"),
        json!("127.0.0.1"),
        json!("--port"),
        json!(endpoint.port.to_string()),
        json!("--runtime-dir"),
        request["runtime_directory"].clone(),
        json!("--api-port"),
        json!(api.port.to_string()),
    ];
    if b0 {
        command.push(json!("--b0"));
    }
    Ok(Value::Array(command))
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
                [0, 3, 5, 15].contains(&index)
            } else {
                [0, 2].contains(&index)
            };
            if is_path {
                check(
                    same_path(&path(wanted)?, &path(recorded)?)?,
                    "恢复命令路径与当前独立工具绑定漂移；核对本机协调器及已登记 launch",
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
