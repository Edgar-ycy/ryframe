//! 从固定阶段记录派生完整历史集合；正文只在原生登记闭环内流转。
use std::{collections::BTreeSet, path::Path};

use serde_json::{Value, json};

use crate::{
    Result,
    recovery::clone::native::{
        json as documents, lifecycle::state, post_process::command, resources::common,
        seed::history,
    },
};

#[path = "lineage/identities.rs"]
mod identities;
#[path = "lineage/producer.rs"]
mod producer;
#[path = "lineage/runtime.rs"]
mod runtime;
#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/clone/native/seed/source_registration/lineage.rs"]
mod tests;

pub(crate) fn collect(backend: &Path, run: &Path, coordinator_tool: &Value) -> Result<Value> {
    let run = common::local_directory(backend, run, "来源历史运行")?;
    let saved = state::load(&run, true)?;
    let post = history::post(backend, &run, None, None, true)?;
    let paths = runtime_paths(&run, &saved, &post.runtime)?;
    let mut identities = identities::Identities::default();
    let mut seen = BTreeSet::new();
    let mut runtimes = Vec::new();
    for (rank, path) in paths.iter().enumerate() {
        let record = runtime::collect(backend, path)?;
        let recorded = Path::new(documents::text(
            &record["runtime_directory"],
            "历史运行目录",
        )?);
        common::local_directory(backend, recorded, "历史运行目录")?;
        let canonical = recorded.canonicalize()?;
        if canonical != path.canonicalize()? || !seen.insert(canonical) {
            return Err("历史运行目录不属于权威集合或重复登记".into());
        }
        for generation in documents::array(&record["generations"], "完整运行代次")? {
            let sequence = generation["sequence"]
                .as_u64()
                .ok_or("历史运行代次序号无效")?;
            identities.runtime(
                &generation["identity"],
                recorded,
                rank,
                documents::text(&generation["role"], "历史运行角色")?,
                sequence,
            )?;
        }
        runtimes.push(record);
    }
    let producers = collect_producers(backend, &run, &saved, coordinator_tool, &mut identities)?;
    if state::load(&run, true)? != saved || state::manifest_binding(&run)? != saved["manifest"] {
        return Err("收集来源历史期间阶段记录或运行清单变化；保留证据，禁止自动重放".into());
    }
    Ok(json!({
        "format_version":1,"kind":"devex-clone-producer-lineage",
        "run_directory":run,"run_manifest":saved["manifest"],
        "runtimes":runtimes,"node_producers":producers,"identities":identities.into_value()
    }))
}

fn runtime_paths(run: &Path, saved: &Value, active: &Path) -> Result<Vec<std::path::PathBuf>> {
    let attempts = state::attempts(saved)?;
    let amendments = attempts
        .iter()
        .filter(|row| {
            row["stage"] == "post-copy" && row["mode"] == "amend" && row["status"] == "passed"
        })
        .count();
    if amendments > 9999 {
        return Err("post-copy runtime 代次无效".into());
    }
    let base = run.join("post-copy");
    let expected = if amendments == 0 {
        base.join("runtime")
    } else {
        base.join(format!("runtime-{amendments:04}"))
    };
    if active != expected {
        return Err("最新 post-copy 登记没有覆盖全部已发布 runtime 代次".into());
    }
    let prepared = attempts
        .iter()
        .filter(|row| {
            row["stage"] == "seed-runtime" && row["mode"] == "prepare" && row["status"] == "passed"
        })
        .count();
    if prepared > 1 {
        return Err("seed runtime 只能有一个已发布准备代次".into());
    }
    // 所有相对段均来自固定 ASCII 名称；该顺序等同于共同 run 前缀下的 casefold 排序。
    let mut paths = vec![base.join("runtime")];
    paths.extend((1..=amendments).map(|number| base.join(format!("runtime-{number:04}"))));
    if prepared == 1 {
        paths.push(run.join("seed-runtime/runtime"));
    }
    Ok(paths)
}

fn collect_producers(
    backend: &Path,
    run: &Path,
    saved: &Value,
    coordinator_tool: &Value,
    identities: &mut identities::Identities,
) -> Result<Vec<Value>> {
    let mut records = Vec::new();
    for attempt in state::attempts(saved)? {
        let stage = documents::text(&attempt["stage"], "历史阶段")?;
        let mode = documents::text(&attempt["mode"], "历史模式")?;
        if !command::producer_phase(stage, mode) {
            continue;
        }
        let number = attempt["number"].as_u64().ok_or("历史 attempt 编号无效")?;
        let registered = json!({
            "attempt":attempt,"controller":common::binding(&state::controller_path(run,number))?
        });
        if let Some(record) = producer::collect(backend, run, &registered, coordinator_tool)? {
            if record["attempt"] != attempt["number"] {
                return Err("历史生产者结果不属于权威 attempt".into());
            }
            identities.producer(
                &record["identity"],
                number,
                documents::text(&record["producer_kind"], "历史生产者类别")?,
            )?;
            records.push(record);
        }
    }
    Ok(records)
}
