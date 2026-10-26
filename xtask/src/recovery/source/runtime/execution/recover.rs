//! 恢复只消费原生产者和登录审计；已有缓存 attempt 永不再次授权 EXEC。
use super::super::{
    images, inventory, model, ready,
    session::{Session, policy},
};
use crate::{
    Result,
    recovery::clone::native::{export::transport::Transport, json as document, resources::common},
};
use serde_json::{Value, json};

const INTENT: &str = "source-verification-recovery-intent.json";
const RECEIPT: &str = "source-verification-recovery.json";

struct Prefix {
    producer_intent: Value,
    node: Value,
    audit: Value,
    before: Value,
    before_image: Value,
}

struct Progress {
    intent: Value,
    cleanup: Value,
    after: Value,
    after_image: Value,
    effects: Value,
    receipt: Value,
}

pub(super) fn run<T: Transport + Sync>(session: &Session<'_, T>) -> Result<Value> {
    let prefix = prefix(session)?;
    let mut state = Progress {
        intent: Value::Null,
        cleanup: Value::Null,
        after: Value::Null,
        after_image: Value::Null,
        effects: Value::Null,
        receipt: Value::Null,
    };
    let stages = policy("flow", json!({"mode":"recover"}))?;
    for stage in document::array(&stages, "来源恢复正式阶段图")? {
        step(
            session,
            &prefix,
            &mut state,
            document::text(stage, "来源恢复阶段")?,
        )?;
    }
    Ok(json!({"output":session.output,"status":state.receipt["status"],"restore_success":false}))
}

fn prefix<T: Transport>(session: &Session<'_, T>) -> Result<Prefix> {
    policy(
        "recovery-prefix",
        json!({"backend":session.backend,"directory":session.directory}),
    )?;
    let producer = session.producer()?.verify()?;
    let node = model::result(
        &json!({"value":producer["stdout"],"lineage":session.facts["lineage"],
        "start":session.start,"lineage_descriptor":session.facts["receipt"]["dataset_lineage"]}),
    )?;
    let producer_intent = document::read(&session.directory.join("source-producer-intent.json"))?;
    let audit = policy(
        "login-stored",
        json!({"backend":session.backend,"directory":session.directory,
        "descriptor":common::binding(&session.directory.join("audit/login-audit.json"))?,"lineage":session.facts["lineage"],
        "subjects":node["subjects"],"started_at":producer_intent["started_at"]}),
    )?;
    let before = common::binding(&session.directory.join("before/image.json"))?;
    let before_image = images::verify(
        session.backend,
        &before,
        &session.facts["selected"],
        &session.facts["source"]["request"],
        &session.facts["receipt"]["source_registration"],
    )?;
    policy(
        "image-binding",
        json!({"before":before_image["image"],"expected":session.facts["running"]["image"]}),
    )?;
    Ok(Prefix {
        producer_intent,
        node,
        audit,
        before,
        before_image,
    })
}

fn intent<T: Transport>(session: &Session<'_, T>, prefix: &Prefix) -> Result<Value> {
    let arguments = json!({"backend":session.backend,"directory":session.directory,"start":session.start,
        "lineage":session.facts["receipt"]["dataset_lineage"],"before":prefix.before,
        "successor":session.successor(),"subjects":prefix.node["subjects"]});
    let path = session.directory.join(INTENT);
    if path.try_exists()? {
        return policy("recovery-pending", arguments);
    }
    let before = inventory::manifest(&session.directory)?;
    let state = ready::policy::cache_attempt::state(&json!({"backend":session.backend,
        "directory":session.directory,"output":session.directory.join("cache-cleanup.json")}))?;
    let observation = if state["status"] == "not_attempted" {
        let checkpoint = || session.checkpoint();
        let audit = session.directory.join("recovery-audit");
        session
            .resources(&audit, &checkpoint)?
            .cache_observation(&prefix.node["subjects"])?
            .0
    } else {
        state["observation"].clone()
    };
    let mut arguments = arguments;
    let fields = arguments
        .as_object_mut()
        .ok_or("来源恢复内部参数不是对象")?;
    fields.remove("subjects");
    fields.insert("pre_manifest".into(), json!(before));
    fields.insert("authorization_cache".into(), observation);
    fields.insert("started_at".into(), json!(chrono::Utc::now().to_rfc3339()));
    let value = policy("recovery-intent", arguments)?;
    document::write_new(&path, &value)?;
    Ok(value)
}

fn step<T: Transport + Sync>(
    session: &Session<'_, T>,
    prefix: &Prefix,
    s: &mut Progress,
    stage: &str,
) -> Result<()> {
    let checkpoint = || session.checkpoint();
    let audit = session.directory.join("recovery-audit");
    let resources = session.resources(&audit, &checkpoint)?;
    match stage {
        "recovery-intent" => {
            s.intent = intent(session, prefix)?;
        }
        "login-recheck" => {
            if !audit.try_exists()? {
                std::fs::create_dir(&audit)?;
            }
            resources.recheck_login(
                document::text(&prefix.audit["verified"]["upper_id"], "原登录审计上界")?.parse()?,
            )?;
        }
        "cache-resume" => {
            s.cleanup = resources.resume_cleanup(&s.intent)?;
        }
        "capture-after" => {
            (s.after, s.after_image) = session.capture("after")?;
        }
        "effects" => {
            s.effects = model::write_effects(
                &json!({"before":prefix.before_image["image"],"after":s.after_image["image"]}),
            )?;
            policy(
                "cache-effects",
                json!({"cleanup":s.cleanup,"effects":s.effects,"recovered":true}),
            )?;
        }
        "recovery-receipt" => {
            let value = policy(
                "recovery-receipt",
                json!({"backend":session.backend,"directory":session.directory,
                "cleanup":s.cleanup,"after":s.after,"completed_at":chrono::Utc::now().to_rfc3339()}),
            )?;
            document::write_new(&session.directory.join(RECEIPT), &value)?;
        }
        "receipt" => {
            s.receipt = policy(
                "runtime-receipt",
                json!({"backend":session.backend,"directory":session.directory,
                "lineage":session.facts["receipt"]["dataset_lineage"],"start":session.start,
                "started_at":prefix.producer_intent["started_at"],"before":prefix.before,"after":s.after,
                "effects":s.effects,"cleanup":s.cleanup,"recovery":common::binding(&session.directory.join(RECEIPT))?}),
            )?;
        }
        "publish" => {
            document::write_new(&session.output, &s.receipt)?;
        }
        "verify-published" => {
            let actual =
                super::saved::verify(session.backend, &common::binding(&session.output)?, true)?;
            policy(
                "published",
                json!({"expected":s.receipt,"actual":actual["receipt"]}),
            )?;
        }
        "checkpoint" => session.checkpoint()?,
        _ => return Err("来源恢复阶段不属于唯一正式计划".into()),
    }
    Ok(())
}
