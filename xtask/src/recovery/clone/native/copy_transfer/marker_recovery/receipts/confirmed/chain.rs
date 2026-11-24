//! 第五代逐件保护第三、第四代的两份原确认意图；不允许任意历史回退。
use super::*;

pub(super) fn load(
    receipts: &Receipts,
    expected: &Value,
    verify_prefix: &dyn Fn(&Value) -> Result<()>,
) -> Result<(Prior, Value)> {
    let saved = state::load(&receipts.run, true)?;
    let passed: Vec<_> = state::attempts(&saved)?
        .iter()
        .filter(|r| r["stage"] == "target-cache-markers" && r["status"] == "passed")
        .collect();
    if passed.len() != 2 {
        return Err("第五代只接受第三及第四代两份正式标记确认".into());
    }
    load_one(receipts, expected, verify_prefix, 2, &passed)
}

fn load_one(
    receipts: &Receipts,
    expected: &Value,
    verify_prefix: &dyn Fn(&Value) -> Result<()>,
    remaining: usize,
    passed: &[&Value],
) -> Result<(Prior, Value)> {
    let name = document::text(expected, "原标记祖先摘要")?;
    if name.len() != 64
        || !name
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("原标记祖先摘要无效".into());
    }
    let root = receipts.directory.parent().ok_or("缺少标记祖先根")?;
    let path = common::local_directory(&receipts.backend, &root.join(name), "精确标记祖先")?;
    if path == receipts.directory {
        return Err("标记接续不得复用当前意图目录".into());
    }
    let directory = Inputs::directory(&path)?;
    let intent = path.join("intent.json");
    let record = document::read(&intent)?;
    let number = record["attempt"]
        .as_u64()
        .filter(|n| *n < receipts.attempt)
        .ok_or("标记祖先阶段无效")?;
    let row = passed[remaining - 1];
    if row["mode"] != "run" || row["number"] != number {
        return Err("标记祖先没有按第三、第四代的真实顺序登记".into());
    }
    let observation_path = path.join(format!("observation-{number:04}.json"));
    let controller_path = state::controller_path(&receipts.run, number);
    let result_path = state::result_path(&receipts.run, number);
    let files = Inputs::open(
        [&intent, &observation_path, &controller_path, &result_path]
            .into_iter()
            .map(|p| protected_file(&receipts.backend, p))
            .collect::<Result<Vec<_>>>()?,
    )?;
    let intent_binding = common::binding(&intent)?;
    if common::read_bound(&receipts.backend, &intent_binding, "原标记祖先意图")? != record {
        return Err("原标记祖先意图在固定期间变化".into());
    }
    let observation_binding = common::binding(&observation_path)?;
    let controller = common::binding(&controller_path)?;
    let result = common::read_bound(&receipts.backend, &row["result"], "原标记祖先正式结果")?;
    let observation = common::read_bound(
        &receipts.backend,
        &observation_binding,
        "原标记祖先完整观察",
    )?;
    Stage::verify_intent_controller(&receipts.backend, &receipts.run, number, &controller)?;
    compare_images(
        receipts,
        expected,
        number,
        &result,
        [&observation_binding, &intent_binding, &controller],
        [&observation, &record],
    )?;
    verify_prefix(&record["copy_prefix"])?;
    let mut ancestors = Vec::new();
    if remaining == 2 {
        let incident = &record["binding"]["fixture"]["lineage"]["accepted_redis_failure"];
        let (prior, proof) = load_one(
            receipts,
            &incident["previous_fixture_sha256"],
            verify_prefix,
            1,
            passed,
        )?;
        if record["binding"]["previous_confirmed_markers"] != proof {
            return Err("第四代原确认意图没有保留同一第三代祖先".into());
        }
        ancestors.push(prior);
    } else if record["binding"]
        .get("previous_confirmed_markers")
        .is_some()
    {
        return Err("第三代原标记意图含未登记的更早写入链".into());
    }
    let prior = Prior {
        intent,
        directory,
        files,
        ancestors,
    };
    prior.check()?;
    Ok((
        prior,
        json!({"result":row["result"],"observation":observation_binding,"intent":intent_binding}),
    ))
}
