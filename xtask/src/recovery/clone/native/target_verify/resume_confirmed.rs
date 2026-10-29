//! 成功确认必须重新绑定完整序列、全部阶段与直接前驱；任何错误均失败关闭。
use super::{
    super::{digest, json as json_io},
    evidence_model::*,
    resume_model as model,
    resume_predecessor::{self, Context},
    resume_runtime,
};
use crate::Result;
use regex::Regex;
use serde_json::{Value, json};
use std::{collections::BTreeSet, path::Path};

const CONFIRMED: &[&str] = &[
    "format_version",
    "kind",
    "attempt",
    "at",
    "intent",
    "started",
    "failure",
    "prepared_files",
    "initialized",
    "original_prefix",
    "resumed_operations",
    "redis_marker_rebind",
    "remote_write_operations",
    "restore_qualified",
];

pub(crate) fn confirmed(backend: &Path, target: &Path, initialized: &Value) -> bool {
    verify(backend, target, initialized).is_ok()
}

fn verify(backend: &Path, target: &Path, initialized: &Value) -> Result<()> {
    let (value, successor) = confirmation(target, backend)?;
    let attempt = json_io::text(&value["attempt"], "续作 attempt")?;
    let intent_path = target.join(format!("resume-initialize-{attempt}.intent.json"));
    let started_path = target.join(format!("resume-initialize-{attempt}.started.json"));
    let intent = read(backend, &intent_path)?;
    let started = read(backend, &started_path)?;
    let prepared = read(backend, &target.join("prepare.json"))?;
    let failure = super::evidence_prepare::failure_path(backend, target, &prepared["request"])?;
    let failed = read(backend, &bound(backend, &failure)?)?;
    json_io::exact(initialized, model::INITIALIZED, "初始化结果")?;
    let mut fields = model::INTENT.to_vec();
    if successor {
        fields.push("predecessor");
    }
    json_io::exact(&intent, &fields, "迁移成功 intent")?;
    json_io::exact(&started, model::STARTED, "迁移成功 started")?;
    json_io::exact(&prepared, model::PREPARE, "迁移 prepare")?;
    let request = read(backend, &bound(backend, &intent["request"])?)?;
    initialized_proof(backend, target, initialized, &prepared, &request)?;
    let creation = super::evidence_creation::creation(backend, target, &request)?;
    let reset =
        super::evidence_reset::history(backend, target, &request, &initialized["generation"])?;
    check(
        initialized["reset"] == reset["completed"],
        "迁移成功 reset 与已确认结果不同",
    )?;
    super::proof::initial_inventory(backend, target, &initialized["inventory"]["receipt"], true)?;
    let (operations, prefix) = completed(backend, target, initialized)?;
    let operations = operations.as_slice();
    let prefix = prefix.as_slice();
    let split = json_io::array(&value["original_prefix"], "原迁移前缀")?.len();
    check(
        split > 0 && split < operations.len() && operations[split]["write"] == false,
        "迁移成功确认没有从首个只读 verify 开始",
    )?;
    if let Some((index, _)) = super::resume_prefix::failed_readonly(backend, target)? {
        check(
            index == split,
            "原始失败只读收据不属于成功确认绑定的原始前缀",
        )?;
    }
    resume_runtime::started(backend, &initialized["generation"], &started)?;
    let initialize_started = binding(backend, &target.join("initialize.started.json"))?;
    let context = Context {
        backend,
        target,
        request: &request,
        descriptor: &prepared["request"],
        failure: &failure,
        prepared_files: &value["prepared_files"],
        initialize_started: &initialize_started,
        creation: &creation,
        reset: &reset,
        operations,
        prefix: &prefix[..split],
    };
    let prior = if successor {
        resume_predecessor::records(&context, Some(&value["predecessor"]))?
    } else {
        None
    };
    let markers = marker_proof(
        &context,
        &value,
        &intent_path,
        &started_path,
        &started,
        prior.as_ref(),
    )?;
    let resumed = resumed_operations(&context, &value, &intent_path, &markers.at, prefix, split)?;
    finish(
        &context,
        &FinalProof {
            value: &value,
            intent: &intent,
            started: &started,
            prepared: &prepared,
            initialized,
            markers: &markers,
            resumed: &resumed,
            prior: prior.as_ref(),
            failed: &failed,
        },
    )?;
    super::resume_baseline::prepared(
        backend,
        target,
        &value["prepared_files"],
        &prepared["request"],
    )?;
    check(model::failure(&failed)?, "迁移原失败不满足只读续作条件")
}

fn confirmation(target: &Path, backend: &Path) -> Result<(Value, bool)> {
    let paths = names(target, r"^resume-initialize-.*\.confirmed\.json$")?;
    let failures = names(target, r"^resume-initialize-.*\.failure\.json$")?;
    check(
        paths.len() == 1 && failures.len() <= 1,
        "迁移成功确认不是唯一结果",
    )?;
    let successor = failures.len() == 1;
    let path = target.join(paths.first().ok_or("迁移成功确认缺失")?);
    let value = read(backend, &path)?;
    let mut fields = CONFIRMED.to_vec();
    if successor {
        fields.push("predecessor");
    }
    json_io::exact(&value, &fields, "迁移成功确认")?;
    let attempt = json_io::text(&value["attempt"], "续作 attempt")?;
    check(
        Regex::new(r"^[a-f0-9]{32}$")?.is_match(attempt)
            && path == target.join(format!("resume-initialize-{attempt}.confirmed.json")),
        "迁移成功确认 attempt 无效",
    )?;
    Ok((value, successor))
}

fn completed(
    backend: &Path,
    target: &Path,
    initialized: &Value,
) -> Result<(Vec<Value>, Vec<Value>)> {
    let operations = super::resume_prefix::operations(
        executable(&initialized["generation"]["maintenance"], "migrate")?,
        &read(backend, &target.join("request.json"))?,
    )?["operations"]
        .clone();
    let prefix =
        super::resume_prefix::prefix(backend, target, &operations, true)?["completed"].clone();
    Ok((
        json_io::array(&operations, "迁移序列")?.to_vec(),
        json_io::array(&prefix, "迁移前缀")?.to_vec(),
    ))
}

fn initialized_proof(
    backend: &Path,
    target: &Path,
    initialized: &Value,
    prepared: &Value,
    request: &Value,
) -> Result<()> {
    check(
        initialized["format_version"] == 1
            && initialized["status"] == "fresh_target_initialized"
            && initialized["id"] == request["id"]
            && initialized["scope_id"] == request["target"]["scope_id"]
            && initialized["generation"] == prepared["generation"]
            && initialized["prepare_sha256"] == digest::file(&target.join("prepare.json"))?.sha256
            && initialized["controlled_generation_never_started"] == true
            && [
                "external_writers_discovered",
                "target_ready",
                "clone_verified",
                "restore_qualified",
            ]
            .iter()
            .all(|key| initialized[*key] == false)
            && read(backend, &target.join("initialized-candidate.json"))? == *initialized
            && read(backend, &target.join("initialized.json"))? == *initialized
            && initialized["history"] == super::proof::history(backend, target)?,
        "迁移成功确认的初始化像无效",
    )
}

fn marker_proof(
    context: &Context<'_>,
    value: &Value,
    intent: &Path,
    started_path: &Path,
    started: &Value,
    prior: Option<&resume_predecessor::Prior>,
) -> Result<resume_runtime::Markers> {
    let markers = if let Some(prior) = prior {
        check(
            prior.failure == value["predecessor"]
                && resume_runtime::fixture(&started["storage_runtime"], &started["cache_runtime"])?
                    == prior.restart
                && started["resources_before"] == prior.resources,
            "迁移后继与首次失败续作代次不同",
        )?;
        model::owned(
            context.backend,
            context.request,
            &started["resources_before"],
            true,
        )?;
        resume_runtime::Markers {
            confirmation: prior.marker.clone(),
            files: prior
                .files
                .iter()
                .filter(|name| name.starts_with("resume-redis-markers-"))
                .cloned()
                .collect(),
            at: started["at"].clone(),
        }
    } else {
        let markers = resume_runtime::markers(
            context.backend,
            context.target,
            context.request,
            json_io::text(&value["attempt"], "attempt")?,
            intent,
            started_path,
            started,
        )?;
        model::owned(
            context.backend,
            context.request,
            &started["resources_before"],
            markers.confirmation.is_null(),
        )?;
        markers
    };
    Ok(markers)
}

struct Resumed {
    values: Vec<Value>,
    files: BTreeSet<String>,
    at: Value,
}

fn resumed_operations(
    context: &Context<'_>,
    value: &Value,
    resume: &Path,
    at: &Value,
    prefix: &[Value],
    split: usize,
) -> Result<Resumed> {
    let attempt = json_io::text(&value["attempt"], "attempt")?;
    let mut result = Resumed {
        values: Vec::new(),
        files: BTreeSet::new(),
        at: at.clone(),
    };
    for (index, operation) in context.operations.iter().enumerate().skip(split) {
        let intent_name = format!("resume-migrate-{attempt}-{index:02}.intent.json");
        let confirmed_name = format!("resume-migrate-{attempt}-{index:02}.confirmed.json");
        let intent = read(context.backend, &context.target.join(&intent_name))?;
        let confirmed = read(context.backend, &context.target.join(&confirmed_name))?;
        json_io::exact(&intent, model::OP_INTENT, "续作操作 intent")?;
        json_io::exact(&confirmed, model::OP_CONFIRMED, "续作操作 confirmed")?;
        let receipt = &prefix[index]["receipt"];
        check(
            intent["format_version"] == 1
                && intent["kind"] == "devex-clone-migration-operation-intent"
                && intent["attempt"] == attempt
                && intent["index"].as_u64() == Some(index as u64)
                && intent["resume"] == binding(context.backend, resume)?
                && intent["failure"] == value["failure"]
                && intent["operation"] == model::operation(operation)
                && confirmed["format_version"] == 1
                && confirmed["kind"] == "devex-clone-migration-operation-confirmed"
                && confirmed["attempt"] == attempt
                && confirmed["index"].as_u64() == Some(index as u64)
                && confirmed["intent"]
                    == binding(context.backend, &context.target.join(&intent_name))?
                && confirmed["receipt"] == *receipt
                && confirmed["write"] == operation["write"]
                && ordered(&[&result.at, &intent["at"], &confirmed["at"]]),
            "迁移续作操作收据、确认或时间链无效",
        )?;
        result.at = confirmed["at"].clone();
        result.values.push(json!({"id":operation["id"],"receipt":receipt,"confirmation":binding(context.backend,&context.target.join(&confirmed_name))?}));
        result.files.extend([intent_name, confirmed_name]);
    }
    Ok(result)
}

struct FinalProof<'a> {
    value: &'a Value,
    intent: &'a Value,
    started: &'a Value,
    prepared: &'a Value,
    initialized: &'a Value,
    markers: &'a resume_runtime::Markers,
    resumed: &'a Resumed,
    prior: Option<&'a resume_predecessor::Prior>,
    failed: &'a Value,
}

fn finish(context: &Context<'_>, proof: &FinalProof<'_>) -> Result<()> {
    let FinalProof {
        value,
        started,
        prepared,
        initialized,
        markers,
        resumed,
        prior,
        ..
    } = proof;
    model::owned(
        context.backend,
        context.request,
        &json!({"databases":started["resources_before"]["databases"],"objects":initialized["objects"],"redis":initialized["redis"]}),
        true,
    )?;
    let mut files = resumed.files.clone();
    for suffix in ["intent", "started", "confirmed"] {
        files.insert(format!(
            "resume-initialize-{}.{}.json",
            json_io::text(&value["attempt"], "attempt")?,
            suffix
        ));
    }
    files.extend(markers.files.iter().cloned());
    if let Some(prior) = prior {
        files.extend(prior.files.iter().cloned());
    }
    check(
        files == model::resume_names(context.target)?,
        "迁移成功确认包含缺失或未知续作文件",
    )?;
    let mut stages = super::evidence_creation::initial_stages(context.request)?;
    stages.extend(super::evidence_prepare::stages(
        context.backend,
        context.target,
        &prepared["request"],
    )?);
    stages.extend(
        files
            .iter()
            .filter(|name| name.ends_with(".intent.json") || name.ends_with(".confirmed.json"))
            .cloned(),
    );
    super::evidence_creation::stage_names(context.target, &stages)?;
    confirmation_fields(context, proof)?;
    intent_fields(context, proof)
}

fn confirmation_fields(context: &Context<'_>, proof: &FinalProof<'_>) -> Result<()> {
    let FinalProof {
        value,
        intent,
        started,
        initialized,
        markers,
        resumed,
        prior,
        failed,
        ..
    } = proof;
    let attempt = json_io::text(&value["attempt"], "attempt")?;
    let writes = context
        .operations
        .iter()
        .skip(context.prefix.len())
        .filter(|item| item["write"] == true)
        .count()
        + usize::from(!markers.confirmation.is_null());
    check(
        value["format_version"] == 1
            && value["kind"] == "devex-clone-migration-resume-confirmed"
            && value["intent"]
                == binding(
                    context.backend,
                    &context
                        .target
                        .join(format!("resume-initialize-{attempt}.intent.json")),
                )?
            && value["started"]
                == binding(
                    context.backend,
                    &context
                        .target
                        .join(format!("resume-initialize-{attempt}.started.json")),
                )?
            && value["failure"] == *context.failure
            && value["initialized"]
                == binding(context.backend, &context.target.join("initialized.json"))?
            && value["prepared_files"] == intent["prepared_files"]
            && value["original_prefix"] == json!(context.prefix)
            && value["resumed_operations"] == json!(resumed.values)
            && value["redis_marker_rebind"] == markers.confirmation
            && value["remote_write_operations"].as_u64() == Some(writes as u64)
            && value["restore_qualified"] == false
            && ordered(&[
                prior.map(|item| &item.at).unwrap_or(&failed["at"]),
                &intent["at"],
                &started["at"],
            ])
            && ordered(&[&resumed.at, &initialized["completed_at"], &value["at"]]),
        "迁移成功确认内容或因果顺序无效",
    )
}

fn intent_fields(context: &Context<'_>, proof: &FinalProof<'_>) -> Result<()> {
    let FinalProof {
        value,
        intent,
        started,
        prepared,
        prior,
        ..
    } = proof;
    let attempt = json_io::text(&value["attempt"], "attempt")?;
    super::resume_prefix::prior_failed_stage(
        context.backend,
        context.target,
        &intent["files_before"],
        json_io::text(
            &context.operations[context.prefix.len()]["stage"],
            "原始缺失阶段",
        )?,
    )?;

    check(
        intent["format_version"] == 1
            && intent["kind"] == "devex-clone-migration-resume-intent"
            && intent["attempt"] == attempt
            && intent["request"] == prepared["request"]
            && intent["prepared_files"] == value["prepared_files"]
            && *context.request == read(context.backend, &context.target.join("request.json"))?
            && intent["failure"] == *context.failure
            && intent["prepare"] == binding(context.backend, &context.target.join("prepare.json"))?
            && intent["initialize_started"] == *context.initialize_started
            && intent["reset"] == *context.reset
            && intent["creation"] == *context.creation
            && intent["protected_binaries"] == model::binaries(context.backend, context.request)?
            && intent["migration_prefix"] == value["original_prefix"]
            && intent["next_operation"] == context.operations[context.prefix.len()]["id"]
            && intent["remote_write_operations_before_resume"].as_u64()
                == Some(u64::from(prior.is_some()))
            && prior.as_ref().is_none_or(|item| {
                intent["predecessor"] == value["predecessor"]
                    && value["predecessor"] == item.failure
            })
            && started["format_version"] == 1
            && started["kind"] == "devex-clone-migration-resume-started"
            && started["attempt"] == attempt
            && started["intent"]
                == binding(
                    context.backend,
                    &context
                        .target
                        .join(format!("resume-initialize-{attempt}.intent.json")),
                )?,
        "迁移成功确认的 intent 或 started 无效",
    )
}
