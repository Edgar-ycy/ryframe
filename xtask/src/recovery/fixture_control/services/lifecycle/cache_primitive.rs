//! 保持原服务原语进程边界的原生 cache 专用分派；不管理外层锁或重放。
use super::{
    model::{self, check},
    primitive,
};
use crate::{
    Result,
    recovery::clone::native::{cache, digest, json as doc},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

pub(super) const ENTRY: &str = "__fixture-service-primitive";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    format_version: u8,
    kind: String,
    backend: PathBuf,
    native_tool: Value,
    operation: String,
    arguments: Value,
}

pub(super) fn run_hidden_if_requested() -> Option<i32> {
    let mut args = std::env::args_os().skip(1);
    if args.next()?.as_os_str() != ENTRY {
        return None;
    }
    let mut bytes = Vec::new();
    let read = std::io::stdin()
        .take(super::runtime::LIMIT as u64 + 1)
        .read_to_end(&mut bytes);
    let result = if read.is_err() || args.next().is_some() {
        Err(model::error("原生cache原语输入读取失败或包含命令参数"))
    } else {
        dispatch(&bytes)
    };
    let (code, response) = response(result);
    Some(if println_response(&response).is_ok() {
        code
    } else {
        1
    })
}

fn println_response(value: &Value) -> Result<()> {
    use std::io::Write;
    writeln!(std::io::stdout().lock(), "{value}")?;
    Ok(())
}

fn dispatch(bytes: &[u8]) -> Result<Value> {
    check(
        !bytes.is_empty() && bytes.len() <= super::runtime::LIMIT,
        "原生cache原语输入为空或超限",
    )?;
    let value = doc::parse_fragment(std::str::from_utf8(bytes)?, "原生cache原语")?;
    let request: Request = serde_json::from_value(value)?;
    check(
        request.format_version == 1 && request.kind == "ryframe-fixture-service-primitive",
        "原生cache原语版本或信封无效",
    )?;
    check(
        matches!(request.operation.as_str(), "cache-start" | "cache-stop"),
        "原生cache原语仅接受明确cache启动或停止",
    )?;
    validate_binding(&request.backend, &request.native_tool)?;
    primitive::validate(&request.backend, &request.operation, &request.arguments)?;
    let args = &request.arguments;
    let output = Path::new(model::text(args, "output")?);
    let result = if request.operation == "cache-start" {
        cache::host_start::start(
            &request.backend,
            &args["request"],
            &args["private"],
            output,
            || {
                super::context::guarded(&request.backend, &args["context"], Some(&args["running"]))
                    .map(|_| ())
            },
        )
    } else {
        cache::host_control::stop_guarded(
            &request.backend,
            &args["request"],
            &args["runtime"],
            output,
            || {
                super::context::guarded(&request.backend, &args["context"], Some(&args["running"]))
                    .map(|_| ())
            },
        )
    };
    validate_binding(&request.backend, &request.native_tool)?;
    result
}

fn validate_binding(backend: &Path, tool: &Value) -> Result<()> {
    check(
        !digest::linked_path(backend)?.contains(&true)
            && std::fs::canonicalize(backend)?
                == std::fs::canonicalize(crate::workspace::root_dir())?,
        "原生cache原语backend与当前操作工作区不同",
    )?;
    check(
        *tool == crate::check::python_suite::native_tool_binding()?,
        "原生cache原语native_tool与当前执行文件不同",
    )
}

fn response(result: Result<Value>) -> (i32, Value) {
    match result {
        Ok(value) => {
            let value = json!({"ok":true,"result":value});
            if value.to_string().len() <= super::runtime::LIMIT {
                (0, value)
            } else {
                (
                    1,
                    json!({"ok":false,"error":"PrimitiveUnknown","diagnostic":null,"exit_code":null,"cleanup_diagnostic":null}),
                )
            }
        }
        Err(error) => {
            let (diagnostic, exit_code) = cache::primitive_detail(error.as_ref());
            let category = match error
                .downcast_ref::<std::io::Error>()
                .map(std::io::Error::kind)
            {
                Some(std::io::ErrorKind::PermissionDenied) => "PermissionError",
                Some(std::io::ErrorKind::TimedOut) => "TimeoutError",
                _ if diagnostic["cause"] == "permission_denied" => "PermissionError",
                _ if diagnostic["cause"] == "timed_out" => "TimeoutError",
                _ if matches!(
                    diagnostic["cause"].as_str(),
                    Some(
                        "sources_changed"
                            | "documents_changed"
                            | "state_changed"
                            | "environment_changed"
                            | "foreign_backend"
                    )
                ) =>
                {
                    "ValueError"
                }
                _ if model::category(error.as_ref()) == "LifecycleRejected" => "ValueError",
                _ => "PrimitiveUnknown",
            };
            (
                1,
                json!({"ok":false,"error":category,"diagnostic":diagnostic,"exit_code":exit_code,"cleanup_diagnostic":cache::primitive_cleanup(error.as_ref())}),
            )
        }
    }
}

pub(super) fn decode(success: bool, value: &Value) -> Result<Value> {
    if success {
        doc::exact(value, &["ok", "result"], "原生cache原语成功响应")?;
        check(value["ok"] == true, "原生cache原语退出状态与响应不同")?;
        return Ok(value["result"].clone());
    }
    doc::exact(
        value,
        &[
            "ok",
            "error",
            "diagnostic",
            "exit_code",
            "cleanup_diagnostic",
        ],
        "原生cache原语失败响应",
    )?;
    check(
        value["ok"] == false
            && matches!(
                value["error"].as_str(),
                Some("ValueError" | "PermissionError" | "TimeoutError" | "PrimitiveUnknown")
            ),
        "原生cache原语失败类别无效",
    )?;
    if !value["diagnostic"].is_null() || !value["exit_code"].is_null() {
        let error = cache::primitive_error(&value["diagnostic"], &value["exit_code"])?;
        return Err(cache::primitive_cleanup_error(
            error,
            &value["cleanup_diagnostic"],
        )?);
    }
    match super::runtime::decode(false, &json!({"ok":false,"error":value["error"]})) {
        Err(error) => Err(cache::primitive_cleanup_error(
            error,
            &value["cleanup_diagnostic"],
        )?),
        Ok(_) => Err("原语失败响应不得解释为成功".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_primitive_rejects_other_operations_foreign_backend_and_changed_self_binding() {
        let backend = crate::workspace::root_dir();
        let tool = crate::check::python_suite::native_tool_binding().unwrap();
        let envelope = json!({"format_version":1,"kind":"ryframe-fixture-service-primitive",
            "backend":backend,"native_tool":tool,"operation":"cache-start","arguments":{}});
        let mut wrong = envelope.clone();
        wrong["operation"] = json!("tree-stop");
        assert_eq!(
            dispatch(&serde_json::to_vec(&wrong).unwrap())
                .unwrap_err()
                .to_string(),
            "原生cache原语仅接受明确cache启动或停止"
        );
        wrong = envelope.clone();
        wrong["native_tool"]["sha256"] = json!("0".repeat(64));
        assert_eq!(
            dispatch(&serde_json::to_vec(&wrong).unwrap())
                .unwrap_err()
                .to_string(),
            "原生cache原语native_tool与当前执行文件不同"
        );
        std::fs::create_dir_all(backend.join(".local-tests")).unwrap();
        let directory = tempfile::tempdir_in(backend.join(".local-tests")).unwrap();
        wrong = envelope;
        wrong["backend"] = json!(directory.path());
        assert_eq!(
            dispatch(&serde_json::to_vec(&wrong).unwrap())
                .unwrap_err()
                .to_string(),
            "原生cache原语backend与当前操作工作区不同"
        );
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        assert!(dispatch(&vec![b'x'; super::super::runtime::LIMIT + 1]).is_err());
    }

    #[test]
    fn private_native_response_preserves_exit_status_and_rejects_untrusted_diagnostics() {
        let original = cache::primitive_error(&Value::Null, &json!(17)).unwrap();
        let cleanup = json!({"format_version":1,"kind":"cache-cleanup-diagnostic","operation":"stop",
            "cause":"permission_denied","os_code":5,"diagnostic":null,"exit_code":null});
        let original = cache::primitive_cleanup_error(original, &cleanup).unwrap();
        let (code, value) = response(Err(original));
        assert_eq!(code, 1);
        assert_eq!(value["exit_code"], 17);
        let error = decode(false, &value).unwrap_err();
        assert_eq!(cache::primitive_cleanup(error.as_ref()), cleanup);
        assert!(
            cache::primitive_message(error.as_ref())
                .unwrap()
                .contains("17")
        );
        for (success, value) in [
            (
                true,
                json!({"ok":false,"error":"PrimitiveUnknown","diagnostic":null,"exit_code":null,"cleanup_diagnostic":null}),
            ),
            (
                false,
                json!({"ok":false,"error":"PrimitiveUnknown","diagnostic":{"advice":"private-marker"},"exit_code":17,"cleanup_diagnostic":null}),
            ),
            (
                false,
                json!({"ok":false,"error":"private-marker","diagnostic":null,"exit_code":null,"cleanup_diagnostic":null}),
            ),
            (
                false,
                json!({"ok":false,"error":"PrimitiveUnknown","diagnostic":null,"exit_code":"private-marker","cleanup_diagnostic":null}),
            ),
            (
                false,
                json!({"ok":false,"error":"PrimitiveUnknown","diagnostic":{"category":"prelaunch_guard_rejected","cause":"private-marker","cause_sha256":"0".repeat(64)},"exit_code":null,"cleanup_diagnostic":null}),
            ),
        ] {
            assert!(
                !decode(success, &value)
                    .unwrap_err()
                    .to_string()
                    .contains("private-marker")
            );
        }
        assert_eq!(
            decode(true, &json!({"ok":true,"result":{"running":true}})).unwrap(),
            json!({"running":true})
        );
    }
}
