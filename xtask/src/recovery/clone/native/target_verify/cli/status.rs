//! I/O 适配层采集事实，状态投影不能授权初始化或未知写入重试。
use serde_json::{Value, json};

use super::{DIAGNOSTICS, check, json_io};
use crate::Result;

fn report(
    status: &str,
    last: Option<&str>,
    pending: &str,
    action: Option<&str>,
    valid: bool,
    reason: Option<&str>,
) -> Value {
    json!({"status":status,"last_successful_stage":last,"pending_stage":pending,"next_action":action,
        "evidence_valid":valid,"blocking_reason":reason,"restore_qualified":false})
}

fn blocked(last: Option<&str>, reason: &str) -> Value {
    report(
        "fresh_target_needs_reconciliation",
        last,
        "reconciliation",
        None,
        false,
        Some(reason),
    )
}

pub(super) fn project(args: &Value) -> Result<Value> {
    json_io::exact(
        args,
        &["phase", "details", "registration", "fixture", "controller"],
        "目标状态观察",
    )?;
    let phase = json_io::text(&args["phase"], "目标状态阶段")?;
    let detail = &args["details"];
    let mut result = base(phase, detail)?;
    if !args["registration"].is_null() {
        descriptor(&args["registration"])?;
        result["registration"] = args["registration"].clone();
    }
    let fixture = &args["fixture"];
    if result["next_action"] == "resume-initialize" && !fixture.is_null() {
        let available = fixture["available"].as_bool().ok_or(DIAGNOSTICS[9])?;
        if !available {
            let reason = fixture["reason"].as_str().ok_or(DIAGNOSTICS[9])?;
            check(
                [
                    "夹具服务已正常关闭；须先执行 fixture services restart 再续作 fresh target",
                    "夹具服务外部终止已核对但尚未重启",
                ]
                .contains(&reason),
                DIAGNOSTICS[9],
            )?;
            block(&mut result, false, "service_restart", None, reason);
        }
    }
    controller(&mut result, &args["controller"])?;
    Ok(result)
}

fn base(phase: &str, detail: &Value) -> Result<Value> {
    let registered = Some("registered");
    Ok(match phase {
        "unregistered" => report(
            "fresh_target_unregistered",
            None,
            "registration",
            Some("prepare"),
            true,
            None,
        ),
        "incomplete" => blocked(None, "workspace 或 registration 不完整"),
        "registration-invalid" => blocked(None, "registration、私有环境或受控存储绑定无效"),
        "request-invalid" => blocked(registered, "固定请求未达到可执行条件"),
        "fixture-invalid" => blocked(registered, "夹具服务账本或当前代次无效"),
        "tree-invalid" => blocked(registered, "fresh 目标目录结构无效"),
        "local-invalid" => blocked(registered, "fresh 目标本地阶段证据无效"),
        "initialized-failure" => blocked(Some("prepared"), "初始化存在未解决失败"),
        "preflight-reconciled" => {
            descriptor(detail)?;
            let mut value = report(
                "fresh_target_preflight_reconciled",
                Some("preflight_reconciled"),
                "new_registration",
                Some("prepare-new-workspace"),
                true,
                Some("原 workspace 已记录收尾，必须使用新的私有环境和工作区重新登记"),
            );
            value["reconciliation"] = detail.clone();
            value
        }
        "initialized" => report(
            "fresh_target_initialized",
            Some("initialized"),
            "verification",
            Some("verify"),
            true,
            None,
        ),
        "initialize-resume" => resume(detail)?,
        "initialize-blocked" => blocked(
            if detail.as_bool().ok_or(DIAGNOSTICS[9])? {
                Some("prepared")
            } else {
                registered
            },
            "初始化已有 intent、锁或未解决失败",
        ),
        "prepared" => report(
            "fresh_creation_prepared",
            Some("prepared"),
            "initialization",
            Some("initialize"),
            true,
            None,
        ),
        "prepare-publication" => report(
            "fresh_target_prepare_publication_pending",
            Some("prepared"),
            "prepared_snapshot",
            Some("resume-prepare"),
            true,
            None,
        ),
        "prepare-interrupted" => report(
            "fresh_target_prepare_interrupted",
            registered,
            "prepare",
            Some("resume-prepare"),
            true,
            None,
        ),
        "prepare-blocked" => {
            let reason = detail.as_str().ok_or(DIAGNOSTICS[9])?;
            check(
                ["已存在资源写入 intent", "包含非只读 prepare 证据"].contains(&reason),
                DIAGNOSTICS[9],
            )?;
            blocked(registered, reason)
        }
        _ => return Err(DIAGNOSTICS[9].into()),
    })
}

fn resume(state: &Value) -> Result<Value> {
    check(state["resumable"] == true, DIAGNOSTICS[9])?;
    let mode = state["mode"].as_str().ok_or(DIAGNOSTICS[9])?;
    if !["migration", "migration-successor"].contains(&mode) {
        check(mode == "inventory", DIAGNOSTICS[9])?;
        return Ok(report(
            "fresh_target_inventory_resume_pending",
            Some("reset"),
            "inventory_resume",
            Some("resume-initialize"),
            true,
            None,
        ));
    }
    let completed = json_io::array(&state["completed"], "迁移已完成序列")?;
    let ids = completed
        .iter()
        .map(|item| json_io::text(&item["id"], "迁移操作"))
        .collect::<Result<Vec<_>>>()?;
    let last = ids.last().ok_or(DIAGNOSTICS[9])?;
    let operations = json_io::array(&state["operations"], "迁移操作序列")?;
    let index = state["next_index"]
        .as_u64()
        .and_then(|index| usize::try_from(index).ok())
        .ok_or(DIAGNOSTICS[9])?;
    let operation = operations.get(index).ok_or(DIAGNOSTICS[9])?;
    let next = json_io::text(&operation["id"], "迁移下一操作")?;
    let write = operation["write"].as_bool().ok_or(DIAGNOSTICS[9])?;
    let mut value = report(
        "fresh_target_migration_resume_pending",
        Some(&format!("migration:{last}")),
        "migration_resume",
        Some("resume-initialize"),
        true,
        None,
    );
    value["resume"] = json!({"mode":mode,"completed_operations":ids,"next_operation":next,"next_operation_is_read_only":!write});
    Ok(value)
}

fn block(result: &mut Value, valid: bool, pending: &str, action: Option<&str>, reason: &str) {
    result["evidence_valid"] = json!(valid);
    result["pending_stage"] = json!(pending);
    result["next_action"] = json!(action);
    result["blocking_reason"] = json!(reason);
}

fn controller(result: &mut Value, controller: &Value) -> Result<()> {
    if controller.is_null() {
        return Ok(());
    }
    if controller == "invalid" {
        block(
            result,
            false,
            "reconciliation",
            None,
            "控制器收据或进程身份无法核实",
        );
        return Ok(());
    }
    let mut fields = vec![
        "run_directory",
        "owner",
        "process_matches",
        "process_missing",
    ];
    if controller.get("phase").is_some() {
        fields.push("phase");
        check(controller["phase"] == "publication_pending", DIAGNOSTICS[9])?;
    }
    json_io::exact(controller, &fields, "控制器只读观察")?;
    check(
        std::path::Path::new(json_io::text(&controller["run_directory"], "控制器目录")?)
            .is_absolute(),
        DIAGNOSTICS[9],
    )?;
    descriptor(&controller["owner"])?;
    let missing = controller["process_missing"]
        .as_bool()
        .ok_or(DIAGNOSTICS[9])?;
    let matches = controller["process_matches"]
        .as_bool()
        .ok_or(DIAGNOSTICS[9])?;
    check(!(missing && matches), DIAGNOSTICS[9])?;
    result["controller"] = controller.clone();
    if result["evidence_valid"] == true {
        let reason = if missing {
            "原控制器已退出，须通过原 recover 入口核对并回收锁"
        } else if matches {
            "原控制器仍在运行"
        } else {
            "控制器 PID 已复用或进程身份不同，不能回收锁"
        };
        block(
            result,
            true,
            "controller",
            if missing { Some("recover") } else { None },
            reason,
        );
    }
    Ok(())
}

fn descriptor(value: &Value) -> Result<()> {
    json_io::exact(value, &["path", "bytes", "sha256"], "目标状态文件绑定")?;
    check(
        value["bytes"].as_u64().is_some()
            && value["path"].as_str().is_some_and(|path| {
                std::path::Path::new(path).is_absolute() && !path.chars().any(char::is_control)
            })
            && value["sha256"].as_str().is_some_and(|sha| {
                sha.len() == 64
                    && sha
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            }),
        DIAGNOSTICS[9],
    )
}
