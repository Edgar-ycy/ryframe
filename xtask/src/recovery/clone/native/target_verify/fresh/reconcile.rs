//! 只收尾唯一 reset 预检失败：先发布完整空前像，再精确清理，不重放初始化。
mod evidence;

use super::super::super::{
    bridge::scope::Scope,
    export::transport::{SystemTransport, Transport},
    json as document,
    resources::common,
};
use super::super::{
    guard::Generation,
    storage::{StorageInputs, SystemObserver},
};
use super::{
    initialize::{inputs, operations::database, window::Window},
    receipts,
    registration::{Registration, Workspace, combine},
    resources::{Resources, database_absence},
};
use crate::Result;
use evidence::Evidence;
use serde_json::{Value, json};
use std::path::Path;

pub(crate) fn execute(backend: &Path, workspace: &Path) -> Result<Value> {
    let result = (|| {
        let scope = Scope::load(backend, &[], &[])?;
        let control = Workspace::claim(backend, workspace)?;
        let result = (|| {
            let registration = Registration::load(backend, workspace)?;
            registration.execute(|active| run(&registration, &scope, active, &SystemTransport))
        })();
        combine(control.finish(result), scope.assert_unchanged())
    })();
    // 与原入口一致，公开输出不透传底层私有错误；原失败文件保持不变。
    result.map_err(|cause| {
        Box::new(super::failure::StageFailure {
            category: "PreflightReconciliationError",
            cause: Some(cause),
        }) as Box<dyn std::error::Error>
    })
}

fn run<T: Transport>(
    registration: &Registration,
    scope: &Scope,
    active: Option<&Path>,
    transport: &T,
) -> Result<Value> {
    let request = common::read_bound(
        &registration.backend,
        &registration.value["request"],
        "预检收尾固定请求",
    )?;
    let environment = super::environment::registered(registration, &request)?;
    let protected = inputs::protect(registration, &request, &environment)?;
    let owner = Generation::acquire(&registration.backend, &registration.target)?;
    let inputs = StorageInputs {
        backend: &registration.backend,
        request: &request,
        environment: &environment,
        observation_dir: &registration.target,
    };
    let resources = Resources {
        inputs: StorageInputs {
            backend: inputs.backend,
            request: inputs.request,
            environment: inputs.environment,
            observation_dir: inputs.observation_dir,
        },
        transport,
        scope,
        run: active,
    };
    let observe_generation = |publish| resources.generation_owned(Some(&owner), publish);
    let observer = SystemObserver::new(transport);
    let result = cleanup(&Window {
        registration,
        inputs,
        transport,
        owner: &owner,
        observe_generation: &observe_generation,
        observer: &observer,
        protected: &protected,
    });
    let result = combine(owner.finish(result), protected.check())?;
    Ok(
        json!({"status":result["status"],"registration":registration.binding,
        "reconciliation":common::binding(&registration.target.join("reconciliation-completed.json"))?,
        "restore_qualified":false}),
    )
}

pub(crate) fn cleanup<T: Transport>(w: &Window<'_, T>) -> Result<Value> {
    w.checkpoint()?;
    let evidence = Evidence::open(w.inputs.observation_dir, &w.registration.value["request"])?;
    let original = &evidence.prepared["generation"];
    check(w, &evidence, original)?;
    // 保留原先双轮完整前像；任何一轮出现资源写入都不能发布清理计划。
    observe(w, original, true)?;
    let before = observe(w, original, true)?;
    check(w, &evidence, original)?;
    receipts::write(
        &w.inputs.observation_dir.join("reconciliation-plan.json"),
        &json!({"status":"preflight_failure_cleanup_planned", "before":before,
            "failure":evidence.failure_digest(),"reset_report":evidence.report_digest()}),
    )?;
    check(w, &evidence, original)?;
    w.redis_state(original, false, true)?;
    let deleted = w.redis(
        original,
        &[
            "DEL",
            document::text(&w.inputs.request["reset"]["sentinel_key"], "sentinel key")?,
        ],
    )?;
    if deleted.as_str() != Some("1") {
        return Err(super::failure::DEL_UNCONFIRMED.into());
    }
    w.redis_state(original, false, false)?;
    let mut databases = document::array(&w.inputs.request["target"]["databases"], "四目标")?
        .iter()
        .collect::<Vec<_>>();
    databases.sort_by(|a, b| b["key"].as_str().cmp(&a["key"].as_str()));
    for db in databases {
        check(w, &evidence, original)?;
        database(w, db, true)?;
        w.mysql(
            db,
            &format!(
                "DROP DATABASE `{}`;",
                document::text(&db["database"], "数据库名")?
            ),
        )?;
        absent(w, db)?;
    }
    let after = observe(w, original, false)?;
    check(w, &evidence, original)?;
    let result = json!({"status":"preflight_failure_reconciled","before":before,"after":after,
        "failure":evidence.failure_digest(),"reset_report":evidence.report_digest(),
        "automatic_retry":false,"restore_qualified":false});
    receipts::write(
        &w.inputs
            .observation_dir
            .join("reconciliation-completed.json"),
        &result,
    )?;
    super::super::binding::reconciliation(w.inputs.backend, w.inputs.observation_dir)?;
    combine(Ok(result), check(w, &evidence, original))
}

fn check<T: Transport>(w: &Window<'_, T>, evidence: &Evidence, original: &Value) -> Result<()> {
    w.owner.check()?;
    w.protected.check()?;
    w.registration.check(true)?;
    evidence.check()?;
    w.unchanged(original)
}

fn observe<T: Transport>(w: &Window<'_, T>, original: &Value, before: bool) -> Result<Value> {
    let databases = document::array(&w.inputs.request["target"]["databases"], "四目标")?
        .iter()
        .map(|db| {
            if before {
                database(w, db, true)
            } else {
                absent(w, db)
            }
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({"databases":databases,"objects":w.objects(false)?,
        "redis":w.redis_state(original,false,before)?}))
}

fn absent<T: Transport>(w: &Window<'_, T>, db: &Value) -> Result<Value> {
    w.checkpoint()?;
    let result = database_absence(&w.inputs, w.transport, db);
    combine(result, w.checkpoint())
}
