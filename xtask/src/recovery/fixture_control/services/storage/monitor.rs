//! monitor 在 Job/session 外持有全部已观察成员，关闭完成前不释放其创建句柄。
use super::*;
use crate::recovery::clone::native::lifecycle::identity;
use std::error::Error;

pub(super) fn run(actor: &actor::Actor, encoded: &str) -> Result<()> {
    let monitor = identity::current()?;
    let command = launch::command(actor, "supervise", encoded)?;
    let mut group = platform::Group::spawn(command, Duration::from_secs(60))?;
    let supervisor = group.identity().clone();
    let result = observe(actor, &monitor, &supervisor, &mut group);
    // 即使 supervisor 自然退出，也先关闭并确认所有成员，再 reap/释放原监督进程。
    let cleanup = group.close();
    let error = cleanup.err();
    let members = group.members();
    let proof = json!({"format_version":1,"operation_id":actor.operation_id,"directory":actor.output,
        "role":"rustfs","scope_id":actor.request["scope_id"],"supervisor":supervisor,"monitor":monitor,
        "status":if error.is_none(){"stopped"}else{"failed"},"members":members,
        "error_type":error.as_ref().map(|_|"MemberCleanupFailed")});
    document::write_new(&actor.member_path("stopped"), &proof)?;
    if result.is_err() || error.is_some() {
        document::write_new(
            &actor.path("actor-failed.json"),
            &json!({"operation_id":actor.operation_id,
            "stage":"supervision","outcome":if error.is_none(){"closed"}else{"unknown"},
            "error_type":"SupervisionFailed"}),
        )?;
        publish_failure_diagnostic(
            &actor.output,
            &actor.operation_id,
            result.as_ref().err().map(Box::as_ref),
            error.as_deref(),
        );
    }
    if error.is_some() {
        return Err("RustFS 完整成员关闭未确认；保留现场".into());
    }
    result
}

pub(super) fn publish_failure_diagnostic(
    output: &Path,
    operation_id: &str,
    observation: Option<&(dyn Error + 'static)>,
    cleanup: Option<&(dyn Error + 'static)>,
) {
    // 旁证写入失败不得覆盖原监督和关闭结果。
    let _ = document::write_new(
        &output.join("actor-failure-diagnostic.json"),
        &json!({
            "format_version": 1,
            "operation_id": operation_id,
            "observation": observation.map(diagnostic),
            "cleanup": cleanup.map(diagnostic),
        }),
    );
}

fn diagnostic(error: &(dyn Error + 'static)) -> Value {
    let mut cause = Some(error);
    while let Some(error) = cause {
        if let Some(reason) = rejection_reason(error) {
            return json!({"category":"rejected","reason":reason});
        }
        if let Some(io) = error.downcast_ref::<std::io::Error>() {
            return json!({"category":"io","os_code":io.raw_os_error()});
        }
        #[cfg(windows)]
        if let Some(code) = error.downcast_ref::<winsafe::co::ERROR>() {
            return json!({"category":"win32","os_code":code.raw()});
        }
        if identity::is_access_denied(error) {
            return json!({"category":"process_access_denied","os_code":5});
        }
        cause = error.source();
    }
    json!({"category":"unknown"})
}

fn observe(
    actor: &actor::Actor,
    monitor: &Value,
    supervisor: &Value,
    group: &mut platform::Group,
) -> Result<()> {
    document::write_new(
        &actor.member_path("ready"),
        &json!({"format_version":1,
        "operation_id":actor.operation_id,"directory":actor.output,"role":"rustfs",
        "scope_id":actor.request["scope_id"],"supervisor":supervisor,"monitor":monitor}),
    )?;
    let mut acknowledged = false;
    loop {
        group.capture()?;
        if actor.path("rustfs.json").try_exists()? && !acknowledged {
            let product = document::read(&actor.path("rustfs.json"))?;
            document::exact(
                &product,
                &["format_version", "role", "scope_id", "identity"],
                "RustFS 产品成员",
            )?;
            check(
                product["format_version"] == 1
                    && product["role"] == "rustfs"
                    && product["scope_id"] == actor.request["scope_id"]
                    && group.members().contains(&product["identity"]),
                "RustFS 产品尚未由 monitor 持有真实创建句柄",
            )?;
            actor::require(&product["identity"])?;
            document::write_new(
                &actor.member_path("product"),
                &json!({"operation_id":actor.operation_id,
                "identity":product["identity"],"monitor":monitor}),
            )?;
            acknowledged = true;
        }
        if group.supervisor_exited()? {
            return Ok(());
        }
        check(!actor.cancelled()?, "本次 RustFS 启动已取消")?;
        std::thread::sleep(Duration::from_millis(20));
    }
}
