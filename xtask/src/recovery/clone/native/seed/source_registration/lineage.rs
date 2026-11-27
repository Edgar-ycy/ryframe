//! 从固定阶段记录派生完整历史集合；正文只在原生登记闭环内流转。
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use crate::{
    Result,
    recovery::clone::native::{
        cache::handoff::registration::cleanup_runtime,
        json as documents,
        lifecycle::state,
        post_process::command,
        resources::common,
        seed::{history, runtime_model},
    },
};

#[path = "lineage/identities.rs"]
mod identities;
#[path = "lineage/producer.rs"]
mod producer;
#[path = "lineage/runtime.rs"]
pub(crate) mod runtime;
#[cfg(test)]
#[path = "../../../../../../tests/unit/recovery/clone/native/seed/source_registration/lineage.rs"]
mod tests;

pub(super) struct RuntimeInputs<'a> {
    pub(super) product: &'a Path,
    pub(super) post: &'a history::Post,
    pub(super) seed: &'a runtime_model::Inputs,
    pub(super) api_successors: Vec<Value>,
}

pub(super) fn collect(
    backend: &Path,
    run: &Path,
    coordinator_tool: &Value,
    saved: &Value,
    inputs: RuntimeInputs<'_>,
) -> Result<Value> {
    let run = common::local_directory(backend, run, "来源历史运行")?;
    let bindings = runtime_bindings(backend, &run, saved, inputs.post, inputs.seed)?;
    let mut records = inputs.api_successors;
    for (path, binding) in bindings {
        let record = runtime::collect_bound(backend, inputs.product, &path, &binding)?;
        if recorded_runtime(&record)?.canonicalize()? != path.canonicalize()? {
            return Err("历史运行目录不属于权威集合".into());
        }
        records.push(record);
    }
    let (runtimes, mut identities) = merge_runtime_history(backend, &run, records)?;
    let producers = collect_producers(backend, &run, saved, coordinator_tool, &mut identities)?;
    if state::load(&run, true)? != *saved || state::manifest_binding(&run)? != saved["manifest"] {
        return Err("收集来源历史期间阶段记录或运行清单变化；保留证据，禁止自动重放".into());
    }
    Ok(json!({
        "format_version":1,"kind":"devex-clone-producer-lineage",
        "run_directory":run,"run_manifest":saved["manifest"],
        "runtimes":runtimes,"node_producers":producers,"identities":identities.into_value()
    }))
}

fn recorded_runtime(record: &Value) -> Result<&Path> {
    Ok(Path::new(documents::text(
        &record["runtime_directory"],
        "历史运行目录",
    )?))
}

fn merge_runtime_history(
    backend: &Path,
    run: &Path,
    records: Vec<Value>,
) -> Result<(Vec<Value>, identities::Identities)> {
    let canonical_run = run.canonicalize()?;
    let mut ordered = BTreeMap::new();
    for record in records {
        let path = common::local_directory(backend, recorded_runtime(&record)?, "历史运行目录")?;
        let canonical = path.canonicalize()?;
        let relative = canonical.strip_prefix(&canonical_run)?;
        let relative = relative.to_str().ok_or("历史运行相对目录必须为 UTF-8")?;
        // 已核验的运行目录使用固定 ASCII 段；统一分隔符后排序与平台无关。
        if relative.is_empty()
            || ordered
                .insert(relative.replace('\\', "/"), record)
                .is_some()
        {
            return Err("历史运行目录不属于权威集合或重复登记".into());
        }
    }
    let runtimes: Vec<Value> = ordered.into_values().collect();
    let mut identities = identities::Identities::default();
    for (rank, record) in runtimes.iter().enumerate() {
        let recorded = recorded_runtime(record)?;
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
    }
    Ok((runtimes, identities))
}

fn runtime_bindings(
    backend: &Path,
    run: &Path,
    saved: &Value,
    active: &history::Post,
    seed: &runtime_model::Inputs,
) -> Result<Vec<(PathBuf, Value)>> {
    let paths = runtime_paths(run, saved, &active.runtime)?;
    let posts = history::post_generations(backend, run, saved, true)?;
    let latest = posts.last().ok_or("缺少活动 post-copy 登记")?;
    if paths.len() != posts.len() + 1
        || paths.last() != Some(&seed.runtime)
        || latest.descriptor != active.descriptor
        || seed.handoff["backend_build"] != active.request["backend_build"]
    {
        return Err("来源历史不同于已发布 post-copy、seed 和产品构建交接".into());
    }
    let seed_contract =
        common::read_bound(backend, &seed.handoff["runtime"], "已发布 seed 运行合同")?;
    let mut inputs = Vec::with_capacity(paths.len());
    for (path, post) in paths.into_iter().zip(posts.iter()) {
        if path != post.runtime || post.request["backend_build"] != active.request["backend_build"]
        {
            return Err("post-copy 历史路径或产品构建不同于权威登记".into());
        }
        let descriptor = common::binding(&path.join("handoff.json"))?;
        let handoff = common::read_bound(backend, &descriptor, "post-copy 原运行交接")?;
        let contract = cleanup_runtime(backend, run, post)?;
        if handoff["registration"] != post.descriptor
            || common::read_bound(backend, &handoff["runtime"], "post-copy 原运行合同")? != contract
            || post.descriptor == active.descriptor && contract != seed_contract
        {
            return Err("来源历史运行合同不同于已发布 post-copy 或 seed 交接".into());
        }
        common::bound_file(backend, &descriptor, "post-copy 原运行交接后像")?;
        inputs.push((path, handoff["runtime"].clone()));
    }
    inputs.push((seed.runtime.clone(), seed.handoff["runtime"].clone()));
    Ok(inputs)
}

fn runtime_paths(run: &Path, saved: &Value, active: &Path) -> Result<Vec<PathBuf>> {
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
