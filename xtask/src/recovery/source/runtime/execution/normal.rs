//! 首次验证按唯一阶段图执行；授权只交给已核验 ready 的本次受管生产者。
use super::super::{
    model,
    session::{Session, policy},
};
use crate::utc_timestamp::now_rfc3339;
use crate::{
    Result,
    recovery::clone::native::{export::transport::Transport, json as document},
};
use serde_json::{Value, json};

struct Progress {
    started: String,
    old_rows: Vec<u8>,
    upper_id: u64,
    node: Value,
    before: Value,
    before_image: Value,
    after: Value,
    after_image: Value,
    cleanup: Value,
    effects: Value,
    receipt: Value,
}

pub(super) fn run<T: Transport + Sync>(session: &Session<'_, T>) -> Result<Value> {
    let mut state = Progress {
        started: now_rfc3339(),
        old_rows: Vec::new(),
        upper_id: 0,
        node: Value::Null,
        before: Value::Null,
        before_image: Value::Null,
        after: Value::Null,
        after_image: Value::Null,
        cleanup: Value::Null,
        effects: Value::Null,
        receipt: Value::Null,
    };
    let stages = policy("flow", json!({"mode":"verify"}))?;
    for stage in document::array(&stages, "来源正式阶段图")? {
        step(session, &mut state, document::text(stage, "来源阶段")?)?;
    }
    Ok(json!({"output":session.output,"status":state.receipt["status"],"restore_success":false}))
}

fn step<T: Transport + Sync>(
    session: &Session<'_, T>,
    s: &mut Progress,
    stage: &str,
) -> Result<()> {
    let checkpoint = || session.checkpoint();
    let audit = session.directory.join("audit");
    let resources = session.resources(&audit, &checkpoint)?;
    match stage {
        "prepare" => {
            std::fs::create_dir(&session.directory)?;
            std::fs::create_dir(&audit)?;
        }
        "login-before" => {
            (s.old_rows, s.upper_id) = resources.login_before()?;
        }
        "capture-before" => {
            (s.before, s.before_image) = session.capture("before")?;
            policy(
                "image-binding",
                json!({"before":s.before_image["image"],"expected":session.facts["running"]["image"]}),
            )?;
        }
        "producer" => {
            let result = session.producer()?.run(&mut || session.checkpoint())?;
            s.node = model::result(
                &json!({"value":result["stdout"],"lineage":session.facts["lineage"],
                "start":session.start,"lineage_descriptor":session.facts["receipt"]["dataset_lineage"]}),
            )?;
        }
        "login-after" => {
            resources.login_after(
                &s.old_rows,
                s.upper_id,
                &session.facts["lineage"],
                &s.node["subjects"],
                &s.started,
            )?;
        }
        "cache-cleanup" => {
            let redis = &session.facts["running"]["image"]["storage"]["redis"];
            let (observation, comparisons) =
                resources.cache_observation(redis, &s.node["subjects"])?;
            s.cleanup = resources.cleanup(redis, &observation, &comparisons)?;
        }
        "capture-after" => {
            (s.after, s.after_image) = session.capture("after")?;
        }
        "effects" => {
            s.effects = model::write_effects(
                &json!({"before":s.before_image["image"],"after":s.after_image["image"]}),
            )?;
            policy(
                "cache-effects",
                json!({"cleanup":s.cleanup,"effects":s.effects,"recovered":false}),
            )?;
        }
        "receipt" => {
            s.receipt = policy(
                "runtime-receipt",
                json!({"backend":session.backend,"directory":session.directory,
            "lineage":session.facts["receipt"]["dataset_lineage"],"start":session.start,"started_at":s.started,
            "before":s.before,"after":s.after,"effects":s.effects,"cleanup":s.cleanup,"recovery":null}),
            )?;
        }
        "publish" => {
            document::write_new(&session.output, &s.receipt)?;
        }
        "verify-published" => {
            let descriptor =
                crate::recovery::clone::native::resources::common::binding(&session.output)?;
            let actual = super::saved::verify(session.backend, &descriptor, true)?;
            policy(
                "published",
                json!({"expected":s.receipt,"actual":actual["receipt"]}),
            )?;
        }
        "checkpoint" => session.checkpoint()?,
        _ => return Err("来源执行阶段不属于唯一正式计划".into()),
    }
    Ok(())
}
