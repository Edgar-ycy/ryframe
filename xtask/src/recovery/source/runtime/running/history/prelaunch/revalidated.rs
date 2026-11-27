//! 核对第 87 次的原始前像、未消费启动预留与相邻的第 86 次封存。
use super::{RECOVER, START, audited, failed, missing, partial, read_controller};
use crate::{
    Result,
    local_test_path::validate_external_existing_file,
    recovery::{
        clone::native::{digest, json as document, resources::common},
        source::evidence::{
            images, inventory, running::history::lineage::failure::inputs::exact_directory,
        },
    },
};
use serde_json::{Value, json};
use std::{fs, path::Path};

const ATTEMPT: u64 = 87;
const RECOVERY: u64 = 88;
const PREVIOUS: u64 = 86;
const STARTED: &str = "2026-11-27T06:57:03.870248+00:00";
const FINISHED: &str = "2026-11-27T07:46:20.382358+00:00";
const HEAD: &str = "40638110f3fb8c84c25afbb49d5dc3ae3d697c85";
const TREE: &str = "cb39ee7be9500e07728e3e7150bcc3d9943b5462";
const FINGERPRINT: &str = "sha256:d7c6e21ebf3a45c3272427797699fc3e183a24f44dbed6680ee52c99519f30aa";
const PREVIOUS_SHA: &str = "e2e56ae19f9a222a131b361f54f1d98c697e703ba7a78ddf4ccbb94181e9acf0";
const CONTROLLER_SHA: &str = "f000511173f4672ada83ea44c54788717c92ac86852e25d1d1ee7813a39a6906";
const FAILURE_SHA: &str = "8ac193c3d8853758ee4fe86c59a4b3bb71c18a34d39e668c5845b4ca83474575";
const EXECUTABLE_SHA: &str = "e0a0c72c283525a50265978061b205aad60f07ff31cf8986156deac170aa3c03";
const MANIFEST_SHA: &str = "9fce8b75c90f80ca714d136791bac006c10dde84f90500ed75b6f5245003ac1a";
const BEFORE_SHA: &str = "affead7bf7d190161c0256b969cf60234083b906898450a662092353887d7204";
const IMAGE_SHA: &str = "91bded5bf0d30dc8eedf6163f81b511c97456a7e768911a3a3ad69866a92bf54";

pub(in crate::recovery::source::evidence) const STATUS: &str =
    "seed_source_revalidated_prelaunch_closed";

pub(in crate::recovery::source::evidence) struct Original {
    pub start: Value,
    pub proof: Value,
    pub before: Value,
    pub image: Value,
}

pub(in crate::recovery::source::evidence) fn applies(start: &Value) -> bool {
    start["number"] == ATTEMPT
        && start["started_at"] == STARTED
        && start["finished_at"] == FINISHED
        && start["sources"]["snapshot"]["head"] == HEAD
        && start["sources"]["worktree_fingerprint"] == FINGERPRINT
        && failed(start, START, "StageError").is_ok()
}

pub(in crate::recovery::source::evidence) fn load(
    backend: &Path,
    run: &Path,
    prefix: &[Value],
    number: u64,
    request: &Value,
    archive: &Value,
) -> Result<Original> {
    let [.., prior, start] = prefix else {
        return Err("原完整前像复核缺少相邻封存与失败记录".into());
    };
    if number != RECOVERY || !applies(start) || prior["sources"] != start["sources"] {
        return Err("原完整前像复核只能单次接续已审计的第 87 次失败".into());
    }
    verify_prior(backend, run, prior, request, archive)?;
    audited(backend, &start["sources"], TREE)?;
    let (controller, owner) = read_controller(run, start)?;
    if controller["sha256"] != CONTROLLER_SHA {
        return Err("原失败控制器摘要与第 87 次审计不同".into());
    }
    let failure = failure(backend, run, &controller)?;
    let executable = frozen_executable(&owner)?;
    common::require_recorded_stopped(&owner["identity"])?;
    missing_outputs(backend, run)?;

    let output = run.join("g0087");
    let manifest = verified_manifest(backend, &output)?;
    let internal_request = partial::verify_request(backend, run, request, &output)?;
    let intent = common::binding(&output.join("intent.json"))?;
    let runtime_dir = output.join("runtime");
    partial::verify_intent(
        &common::read_bound(backend, &intent, "第 87 次启动意图")?,
        &internal_request,
        &runtime_dir,
    )?;
    let runtime = common::binding(&runtime_dir.join("runtime.json"))?;
    let before = common::binding(&output.join("before/image.json"))?;
    let image = original_image(backend, &runtime_dir, request, &before, archive)?;

    if read_controller(run, start)? != (controller.clone(), owner.clone())
        || verified_manifest(backend, &output)? != manifest
    {
        return Err("原失败控制器或代次目录在完整前像核验期间变化".into());
    }
    common::require_recorded_stopped(&owner["identity"])?;
    common::bound_file(backend, &failure, "第 87 次原失败")?;
    common::bound_file(backend, &prior["result"], "第 86 次原封存")?;
    common::bound_file(backend, request, "原外部生成请求")?;
    missing_outputs(backend, run)?;

    let proof = json!({
        "attempt":digest::plan_hash(start)?,"prior":prior["result"],
        "failure":failure,"controller":controller,"executable":executable,
        "source":start["sources"],"head":HEAD,"tree":TREE,"manifest":manifest,
        "external_request":request,"request":internal_request,"intent":intent,"runtime":runtime,
        "before":before
    });
    Ok(Original {
        start: start.clone(),
        proof,
        before,
        image,
    })
}

fn verify_prior(
    backend: &Path,
    run: &Path,
    prior: &Value,
    request: &Value,
    archive: &Value,
) -> Result<()> {
    if prior["number"] != PREVIOUS
        || prior["stage"] != "seed-runtime"
        || prior["mode"] != RECOVER
        || prior["status"] != "passed"
        || !prior["error_type"].is_null()
        || prior["result"]["sha256"] != PREVIOUS_SHA
        || archive["recovery"] != *prior
        || archive["records"].as_array().and_then(|rows| rows.last()) != Some(prior)
        || archive["receipt"]["status"] != "seed_source_partial_prelaunch_closed"
        || archive["receipt"]["request"] != *request
        || common::bound_file(backend, &prior["result"], "第 86 次原封存")?
            != run.join("results/0086.json")
        || common::read_bound(backend, &prior["result"], "第 86 次原封存")? != archive["receipt"]
    {
        return Err("原完整前像未绑定相邻的第 86 次原封存与同一正式请求".into());
    }
    Ok(())
}

fn failure(backend: &Path, run: &Path, controller: &Value) -> Result<Value> {
    let descriptor = common::binding(&run.join("failure-0087.json"))?;
    if descriptor["sha256"] != FAILURE_SHA
        || common::read_bound(backend, &descriptor, "第 87 次原失败")?
            != json!({"format_version":2,"kind":"devex-stage-failure","attempt":ATTEMPT,
                "stage":"seed-runtime","mode":START,"operation_id":"clone.seed-runtime.source-generation-start",
                "error_type":"StageError","controller":controller})
    {
        return Err("第 87 次原生 StageError 收据不同".into());
    }
    Ok(descriptor)
}

fn frozen_executable(owner: &Value) -> Result<Value> {
    let path = Path::new(document::text(
        &owner["identity"]["executable"],
        "第 87 次冻结控制器",
    )?);
    validate_external_existing_file(path)?;
    let binding = common::binding(path)?;
    if binding["sha256"] != EXECUTABLE_SHA || binding["bytes"] != 75_530_240 {
        return Err("第 87 次原冻结控制器大小或摘要不同".into());
    }
    Ok(binding)
}

fn missing_outputs(backend: &Path, run: &Path) -> Result<()> {
    missing(backend, &run.join("results/0087.json"))?;
    missing(backend, &run.join("seed-runtime/attempt-0087"))
}

fn verified_manifest(backend: &Path, output: &Path) -> Result<Value> {
    exact_directory(
        backend,
        output,
        &["intent.json", "request.json"],
        &["before", "runtime", "runtime-configuration"],
    )?;
    exact_directory(
        backend,
        &output.join("runtime"),
        &["binaries.json", "runtime.json"],
        &[],
    )?;
    let configuration = output.join("runtime-configuration");
    exact_directory(backend, &configuration, &[], &["config", "worker-config"])?;
    for role in ["config", "worker-config"] {
        exact_directory(
            backend,
            &configuration.join(role),
            &["app.toml", "app.test.toml"],
            &[],
        )?;
    }
    let files = inventory::manifest(output)?;
    let bytes = files.iter().try_fold(0_u64, |sum, entry| {
        sum.checked_add(entry["bytes"].as_u64().ok_or("原证据文件字节数无效")?)
            .ok_or("原证据文件字节数溢出")
    })?;
    let summary =
        json!({"files":files.len(),"bytes":bytes,"sha256":digest::plan_hash(&json!(files))?});
    if summary != json!({"files":10216,"bytes":1_077_388_636_u64,"sha256":MANIFEST_SHA}) {
        return Err("第 87 次完整代次目录与已审计的原文件集合不同".into());
    }
    for entry in files {
        let name = document::text(&entry["path"], "原证据文件路径")?;
        if !name.starts_with("before/") {
            let path = output.join(name);
            let metadata = fs::symlink_metadata(&path)?;
            if !metadata.is_file()
                || digest::linked(&metadata)
                || common::link_count_for(&path, &metadata)? != 1
            {
                return Err("原启动意图和运行配置必须为无链接普通文件".into());
            }
        }
    }
    Ok(summary)
}

fn original_image(
    backend: &Path,
    runtime: &Path,
    request: &Value,
    before: &Value,
    archive: &Value,
) -> Result<Value> {
    if before["sha256"] != BEFORE_SHA || before["bytes"] != 692_642 {
        return Err("第 87 次完整前像原件摘要或大小不同".into());
    }
    let generation = common::read_bound(backend, request, "原正式生成请求")?;
    let registration = &generation["source_registration"];
    let outer = common::read_bound(backend, registration, "来源登记结果")?;
    if outer["status"] != "seed_source_registered"
        || registration != &archive["receipt"]["source_registration"]
        || generation["format_version"] != 2
    {
        return Err("第 87 次原正式请求与第 86 次封存的登记范围不同".into());
    }
    let predecessor = common::read_bound(backend, &outer["source_request"], "已登记的来源请求")?;
    let mut selected = predecessor["source"].clone();
    document::object(&selected, "第 87 次原物理来源")?;
    selected["runtime_dir"] = json!(runtime);
    selected["s3"]["configuration"] = generation["configuration"].clone();
    let value = images::verify(backend, before, &selected, &predecessor, registration)?;
    if value["image_sha256"] != IMAGE_SHA || value["image"] != archive["image"] {
        return Err("第 87 次原完整前像与第 86 次封存逻辑像不同".into());
    }
    Ok(value["image"].clone())
}
