use std::path::Path;

use serde_json::{Value, json};

use crate::{Result, recovery::clone::native::lifecycle::stage::StageRequest};

fn request(root: &Path) -> StageRequest<'_> {
    StageRequest {
        backend: root,
        run_dir: root,
        stage: "target-verify",
        mode: "run",
        attempt: None,
    }
}

#[test]
fn writing_stage_protocol_requires_a_positive_attempt() {
    let root = crate::workspace::root_dir();
    assert!(protocol(&request(&root), "target-authority").is_err());
    assert!(protocol(&request(&root), "unknown").is_err());
    assert!(protocol(&request(&root), "run").is_err());
    assert!(
        protocol(
            &StageRequest {
                attempt: Some(0),
                ..request(&root)
            },
            "run"
        )
        .is_err()
    );
    let input = StageRequest {
        attempt: Some(1),
        ..request(&root)
    };
    let value: Value = serde_json::from_str(&protocol(&input, "run").unwrap()).unwrap();
    assert_eq!(value["request"]["attempt"], 1);
    assert_eq!(value["request"]["write"], true);
    assert!(protocol(&input, "preflight").is_err());
}

pub(super) fn protocol(request: &StageRequest<'_>, operation: &str) -> Result<String> {
    let attempt = request.attempt;
    if !matches!(operation, "preflight" | "run")
        || (operation == "run") != attempt.is_some()
        || attempt == Some(0)
    {
        return Err("阶段资源编排 operation 与 attempt 不匹配".into());
    }
    let value = json!({
        "format_version": 1,
        "kind": "ryframe-xtask-recovery-clone-stage",
        "native_tool": crate::rate_limit::current_tool_binding()?,
        "request": {
            "backend_dir": path_text(request.backend, "后端目录")?,
            "run_dir": path_text(request.run_dir, "运行目录")?,
            "stage": request.stage,
            "mode": request.mode,
            "operation": operation,
            "attempt": attempt,
            "write": operation == "run",
        },
    });
    let payload = serde_json::to_string(&value)?;
    if payload.len() > 32 * 1024 || payload.contains(['\r', '\n', '\0']) {
        return Err("阶段资源编排私有协议过长或包含换行/NUL".into());
    }
    Ok(payload)
}

fn path_text<'a>(path: &'a Path, label: &str) -> Result<&'a str> {
    path.to_str()
        .ok_or_else(|| format!("{label}必须为 UTF-8").into())
}
