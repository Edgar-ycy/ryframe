//! 从固定 prepare 与连续迁移前缀判定唯一可恢复边界。
use super::{
    super::{digest, json as json_io},
    evidence_model::*,
    resume_model as model,
    resume_predecessor::{self, Context},
};
use crate::Result;
use serde_json::{Value, json};
use std::path::Path;

pub(crate) fn state(
    backend: &Path,
    target: &Path,
    descriptor: &Value,
    prepared_files: &Value,
    owned_lock: bool,
) -> Result<Value> {
    let files = super::tree::snapshot(backend, target, owned_lock)?;
    let failure = super::evidence_prepare::failure_path(backend, target, descriptor)?;
    let failure_path = bound(backend, &failure)?;
    let lock = target.join("initialize.lock");
    let lock_invalid = if owned_lock {
        !lock.is_dir() || std::fs::read_dir(&lock)?.next().is_some()
    } else {
        lock.exists()
    };
    if target.join("initialized.json").exists()
        || target.join("initialized-candidate.json").exists()
        || lock_invalid
        || !target.join("initialize.started.json").is_file()
        || !failure_path.is_file()
        || target.join("inventory-initial").exists()
        || !names(target, r"^inventory-resume-.*$")?.is_empty()
    {
        return Ok(json!({"resumable":false,"reason":"缺少唯一的迁移中断现场"}));
    }
    let failed = read(backend, &failure_path)?;
    if !model::failure(&failed)? {
        return Ok(json!({"resumable":false,"reason":"失败不是可证明前缀的迁移文件访问中断"}));
    }
    let prepared = read(backend, &target.join("prepare.json"))?;
    json_io::exact(&prepared, model::PREPARE, "迁移 prepare")?;
    let request = read(backend, &bound(backend, descriptor)?)?;
    let baseline = super::resume_baseline::prepared(backend, target, prepared_files, descriptor)?;
    check(
        prepared["request"] == *descriptor
            && prepared["request_sha256"] == digest::plan_hash(&request)?
            && request == read(backend, &target.join("request.json"))?,
        "迁移续作请求或 prepare 不属于固定 registration",
    )?;
    let started = read(backend, &target.join("initialize.started.json"))?;
    json_io::exact(&started, &["at", "generation_sha256"], "初始化开始")?;
    check(
        ordered(&[&started["at"], &failed["at"]])
            && started["generation_sha256"] == digest::plan_hash(&prepared["generation"])?,
        "迁移续作的初始化前像摘要无效",
    )?;
    let creation = super::evidence_creation::creation(backend, target, &request)?;
    let prepare_names = super::evidence_prepare::stages(backend, target, descriptor)?;
    let reset =
        super::evidence_reset::aggregate(backend, target, &request, &prepared["generation"])?;
    let operations = super::resume_prefix::operations(executable(
        &prepared["generation"]["maintenance"],
        "migrate",
    )?)?["operations"]
        .clone();
    let prefix = super::resume_prefix::prefix(backend, target, &operations, false)?;
    let initialize_started = binding(backend, &target.join("initialize.started.json"))?;
    let operations_array = json_io::array(&operations, "迁移序列")?;
    let completed = json_io::array(&prefix["completed"], "迁移前缀")?;
    let context = Context {
        backend,
        target,
        request: &request,
        descriptor,
        failure: &failure,
        prepared_files,
        initialize_started: &initialize_started,
        creation: &creation,
        reset: &reset,
        operations: operations_array,
        prefix: completed,
    };
    let prior = resume_predecessor::records(&context, None)?;
    verify_tree(&context, &files, &baseline, &prepare_names, prior.as_ref())?;
    check(
        super::tree::snapshot(backend, target, owned_lock)? == files,
        "迁移续作判定期间目标证据树发生变化",
    )?;
    Ok(
        json!({"resumable":true,"reason":null,"mode":if prior.is_some(){"migration-successor"}else{"migration"},
        "prepared":prepared,"request":request,"failure":failure,"initialize_started":initialize_started,"creation":creation,"reset":reset,"operations":operations,
        "completed":prefix["completed"],"next_index":prefix["next_index"],"prepared_files":prepared_files,"baseline_files":baseline,"files":files,
        "predecessor":prior.as_ref().map(|item|&item.failure),"prior_marker_confirmation":prior.as_ref().map(|item|&item.marker),
        "restart_generation":prior.as_ref().map(|item|&item.restart),"protected_binaries":prior.as_ref().map(|item|&item.binaries),"expected_resources":prior.as_ref().map(|item|&item.resources)}),
    )
}

fn verify_tree(
    context: &Context<'_>,
    files: &Value,
    baseline: &Value,
    prepare_names: &std::collections::BTreeSet<String>,
    prior: Option<&resume_predecessor::Prior>,
) -> Result<()> {
    let mut stages = super::evidence_creation::initial_stages(context.request)?;
    stages.extend(prepare_names.iter().cloned());
    let mut required = super::evidence_creation::paths(
        context.target,
        context.creation,
        context.reset,
        &json!(context.prefix),
    )?;
    required.insert(model::descriptor_name(context.failure)?);
    required.extend(prepare_names.iter().cloned());
    let mut current = files.clone();
    if let Some(prior) = prior {
        stages.extend(
            prior
                .files
                .iter()
                .filter(|name| name.ends_with(".intent.json") || name.ends_with(".confirmed.json"))
                .cloned(),
        );
        required.extend(prior.files.iter().cloned());
        let excluded =
            resume_predecessor::observation_names(context, files, baseline, &prior.started)?;
        current.as_array_mut().ok_or("目标树无效")?.retain(|item| {
            item["path"]
                .as_str()
                .is_none_or(|name| !excluded.contains(name))
        });
    }
    super::evidence_creation::stage_names(context.target, &stages)?;
    super::resume_observations::delta(
        context.backend,
        context.target,
        &current,
        baseline,
        context.request,
        super::resume_observations::Delta {
            required: &required,
            rounds: 12 + context.prefix.len(),
            initialized: prior.map(|item| &item.started["resources_before"]["objects"]),
        },
    )
}
