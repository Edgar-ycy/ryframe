//! 完整复制后逐件保护已确认的原标记意图，拒绝任意历史回退。
use super::*;

pub(super) fn load(
    receipts: &Receipts,
    expected: &Value,
    verify_prefix: &dyn Fn(&Value) -> Result<()>,
) -> Result<(Prior, Value)> {
    let depth = ancestor_count(&receipts.binding)?;
    let saved = state::load(&receipts.run, true)?;
    let passed: Vec<_> = state::attempts(&saved)?
        .iter()
        .filter(|r| r["stage"] == "target-cache-markers" && r["status"] == "passed")
        .collect();
    if passed.len() != depth
        || passed.windows(2).any(|pair| {
            !matches!(
                (pair[0]["number"].as_u64(), pair[1]["number"].as_u64()),
                (Some(previous), Some(current)) if previous < current
            )
        })
    {
        return Err("完整复制后的 Redis 代次必须按顺序绑定所需全部原标记确认".into());
    }
    load_one(receipts, expected, verify_prefix, depth, &passed)
}

fn ancestor_count(binding: &Value) -> Result<usize> {
    let completed = &binding["completed_copy"];
    let lineage = &binding["fixture"]["lineage"];
    let prior = match (
        completed.get("prepared_before_api"),
        lineage.get("prepared_redis_failure"),
    ) {
        (None, None) => 2,
        (Some(prepared), Some(incident)) if nonempty_object(prepared) && incident.is_object() => 3,
        _ => return Err("第六代原标记确认要求同时绑定 API 准备证明及后继 Redis 事故".into()),
    };
    match (
        completed.get("after_api_incident"),
        lineage.get("runtime_incident_redis_failure"),
    ) {
        (None, None) => Ok(prior),
        (Some(incident), Some(redis))
            if prior == 3 && nonempty_object(incident) && nonempty_object(redis) =>
        {
            Ok(4)
        }
        _ => Err("第七代原标记确认要求同时绑定 API 运行事故及后继 Redis 事故".into()),
    }
}

fn nonempty_object(value: &Value) -> bool {
    value.as_object().is_some_and(|proof| !proof.is_empty())
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
        return Err("标记祖先没有按原 Redis 代次真实顺序登记".into());
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
    if let Some(key) = ancestor_link(&record["binding"], remaining)? {
        let incident = &record["binding"]["fixture"]["lineage"][key];
        let (prior, proof) = load_one(
            receipts,
            &incident["previous_fixture_sha256"],
            verify_prefix,
            remaining - 1,
            passed,
        )?;
        if record["binding"]["previous_confirmed_markers"] != proof {
            return Err("原确认意图未绑定同一直接前驱的已确认标记".into());
        }
        ancestors.push(prior);
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

fn ancestor_link(binding: &Value, remaining: usize) -> Result<Option<&'static str>> {
    if binding["completed_copy"]
        .get("after_api_incident")
        .is_some()
        || binding["fixture"]["lineage"]
            .get("runtime_incident_redis_failure")
            .is_some()
    {
        return Err("API 运行事故后的标记不得作为更早代次的原标记".into());
    }
    match remaining {
        4 if ancestor_count(binding)? == 3 => Ok(Some("prepared_redis_failure")),
        4 => Err("第七代的直接标记前驱必须是原 API 准备窗口的第六代".into()),
        3 if binding["completed_copy"].is_object()
            && binding["completed_copy"]
                .get("prepared_before_api")
                .is_none()
            && binding["fixture"]["lineage"]
                .get("prepared_redis_failure")
                .is_none() =>
        {
            Ok(Some("post_copy_redis_failure"))
        }
        3 => Err("第六代的直接标记前驱必须是原未准备 API 的第五代".into()),
        2 => Ok(Some("accepted_redis_failure")),
        1 if binding.get("previous_confirmed_markers").is_none() => Ok(None),
        _ => Err("第三代原标记意图含未登记的更早写入链".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn third_marker_ancestor_requires_the_prepared_window_and_an_original_fifth_marker() {
        let mut binding = json!({"completed_copy":{"result":{}},"fixture":{"lineage":{}}});
        assert_eq!(ancestor_count(&binding).unwrap(), 2);
        assert_eq!(
            ancestor_link(&binding, 3).unwrap(),
            Some("post_copy_redis_failure")
        );
        assert_eq!(
            ancestor_link(&binding, 2).unwrap(),
            Some("accepted_redis_failure")
        );
        assert_eq!(ancestor_link(&binding, 1).unwrap(), None);

        binding["completed_copy"]["prepared_before_api"] = json!({"request":{}});
        assert!(ancestor_count(&binding).is_err());
        binding["fixture"]["lineage"]["prepared_redis_failure"] =
            json!({"previous_fixture_sha256":"original"});
        assert_eq!(ancestor_count(&binding).unwrap(), 3);
        assert!(ancestor_link(&binding, 3).is_err());
        binding["completed_copy"]["prepared_before_api"] = json!({});
        assert!(ancestor_count(&binding).is_err());
    }

    #[test]
    fn fourth_marker_ancestor_requires_both_incident_bindings_and_an_original_sixth_marker() {
        let mut binding = json!({
            "completed_copy":{"prepared_before_api":{"request":{}}},
            "fixture":{"lineage":{"prepared_redis_failure":{"previous_fixture_sha256":"original"}}}
        });
        assert_eq!(
            ancestor_link(&binding, 4).unwrap(),
            Some("prepared_redis_failure")
        );
        assert!(ancestor_link(&json!({}), 4).is_err());
        binding["completed_copy"]["after_api_incident"] = json!({"api_failure":{}});
        assert!(ancestor_count(&binding).is_err());
        binding["fixture"]["lineage"]["runtime_incident_redis_failure"] = json!({});
        assert!(ancestor_count(&binding).is_err());
        binding["fixture"]["lineage"]["runtime_incident_redis_failure"] =
            json!({"previous_fixture_sha256":"original"});
        assert_eq!(ancestor_count(&binding).unwrap(), 4);
        assert!(ancestor_link(&binding, 4).is_err());
        binding["completed_copy"]["after_api_incident"] = Value::Null;
        assert!(ancestor_count(&binding).is_err());
    }
}
