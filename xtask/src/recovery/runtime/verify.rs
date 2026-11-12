//! 对登记的单侧运行收据进行只读在线核验。
use std::{
    io::{self, Read},
    path::Path,
};

use serde_json::{Value, json};

use super::{
    build,
    dataset_authority::{
        b0, check, commands,
        cross::{self, RuntimeDocuments},
        environment,
        inputs::Inputs,
        path,
        reads::Window,
        request_shape, schema,
    },
    generation, probes,
};
use crate::{
    Result,
    cli::{RuntimeSources, RuntimeVerifyOptions},
    recovery::{
        clone::native::{json as strict, source},
        fixture_control::services::lifecycle::readonly::process,
        frontend_build::{self, files::StableFile},
        inputs::{comparison, document, domain},
    },
};

#[cfg(test)]
#[path = "../../../tests/unit/recovery/runtime/verify.rs"]
mod tests;

pub(super) fn run(root: &Path, options: &RuntimeVerifyOptions) -> Result<Value> {
    let supplied = read_authority(io::stdin().lock())?;
    let window = Window::direct()?;
    let mut inputs = Inputs::new(root);
    let result = (|| {
        let receipt = inputs.read(&options.receipt)?;
        let observed = online(
            root,
            &options.sources,
            &options.bindings,
            &receipt.value,
            &supplied,
            &mut inputs,
        )?;
        Ok(observed.reply(&receipt))
    })();
    window.finish(inputs.finish(result))
}

fn read_authority(reader: impl Read) -> Result<Value> {
    let mut bytes = Vec::new();
    reader
        .take(strict::MAX_JSON_BYTES + 1)
        .read_to_end(&mut bytes)?;
    check(
        !bytes.is_empty() && bytes.len() as u64 <= strict::MAX_JSON_BYTES,
        "verify 必须从 stdin 接收非空且不超过 16 MiB 的权威恢复上下文",
    )?;
    let value = strict::parse_fragment(std::str::from_utf8(&bytes)?, "stdin 权威恢复上下文")?;
    request_shape::authority(&value)?;
    Ok(value)
}

pub(super) struct Observed {
    documents: RuntimeDocuments,
    processes: Value,
    readiness: Value,
}

impl Observed {
    pub(super) fn reply(self, receipt: &document::Document) -> Value {
        reply(receipt, &self.documents, self.processes, self.readiness)
    }
}

pub(super) fn online(
    root: &Path,
    sources: &RuntimeSources,
    bindings: &Path,
    runtime: &Value,
    supplied: &Value,
    inputs: &mut Inputs,
) -> Result<Observed> {
    build::source::repository(root)?;
    schema::runtime(runtime)?;
    verify_paths(runtime, sources, bindings)?;
    let documents = cross::read_documents(inputs, runtime)?;
    let launch = &documents.launch.value;
    let request = &launch["request"];
    let commands = commands::verify_direct(request, inputs)?;
    let directory = path(&runtime["paths"]["runtime_dir"])?;
    generation::request(&json!({
        "action":"validate-launch", "root":directory, "binding":request["registration"],
        "value":launch, "previous":null, "commands":commands, "number":launch["generation"],
    }))?;
    environment::verify(inputs, request)?;
    let generation_directory = path(&launch["runtime_directory"])?;
    environment::verify_projected(inputs, request, &generation_directory)?;
    let build_environment = registered_build_environment(inputs, request, &documents.backend)?;
    let backend = backend_sources(
        root,
        sources,
        &documents.backend,
        supplied,
        &build_environment,
    )?;
    let frontend = Frontend::capture(&sources.source_frontend, &documents.frontend, supplied)?;
    let product = if backend["product"].is_null() {
        &backend["inventory"]
    } else {
        &backend["product"]["inventory"]
    };
    let source = json!({
        "backend_product_sha":product["source"]["snapshot"]["head"],
        "backend_execution_sha":backend["inventory"]["source"]["snapshot"]["head"],
        "backend_adapter_contract":sources.adapter_contract,
        "frontend_sha":frontend.inventory()["source"]["snapshot"]["head"],
    });
    verify_authority(runtime, &documents, supplied, source)?;
    let files = &documents.frontend.value["files"];
    require_frontend_files(files)?;
    let processes = observe_processes(runtime, &commands, inputs)?;
    let readiness = probe_and_recheck(runtime, files, &processes, || {
        frontend.verify()?;
        check(
            backend_sources(
                root,
                sources,
                &documents.backend,
                supplied,
                &build_environment,
            )? == backend,
            "恢复运行核验期间后端源码发生变化",
        )?;
        environment::verify(inputs, request)?;
        environment::verify_projected(inputs, request, &generation_directory)
    })?;
    Ok(Observed {
        documents,
        processes: processes.value,
        readiness,
    })
}

fn registered_build_environment(
    inputs: &mut Inputs,
    request: &Value,
    backend: &document::Document,
) -> Result<Value> {
    let target = inputs.bound(&request["registration"]["target_plan"])?;
    let comparison = inputs.bound(&target.value["comparison_sources"])?;
    let side = strict::text(&target.value["target_side"], "恢复目标侧")?;
    let arm = comparison::arm(&comparison.value, side)?;
    check(
        comparison.value["format_version"] == 2
            && comparison.value["kind"] == "restore-comparison-sources"
            && target.value["comparison_arm_sha256"] == document::hash(arm)?
            && arm["roots"]["execution_backend"] == request["roots"]["backend_execution"]
            && arm["execution_sources"]["backend"]["source"]["snapshot"]["head"]
                == request["authority"]["backend_execution_sha"]
            && arm["builds"]["backend"] == document::binding(backend),
        "恢复运行与已登记的单侧后端构建环境不同",
    )?;
    let environment = &arm["backend_environment"];
    let binding = document::binding_shape(environment, "已登记后端构建环境")?;
    inputs.protect(&binding.path)?;
    Ok(environment.clone())
}

fn verify_paths(runtime: &Value, sources: &RuntimeSources, bindings: &Path) -> Result<()> {
    let paths = &runtime["paths"];
    let product = sources
        .product_backend
        .as_deref()
        .unwrap_or(&sources.source_backend);
    for (key, expected) in [
        ("backend_product_root", product),
        ("backend_execution_root", sources.source_backend.as_path()),
        ("frontend_root", sources.source_frontend.as_path()),
        ("bindings", bindings),
    ] {
        check(
            same_path(&path(&paths[key])?, expected),
            "恢复运行收据路径与显式参数不同",
        )?;
    }
    check(
        runtime["source"]["backend_adapter_contract"] == json!(sources.adapter_contract),
        "命令行适配合同与权威恢复上下文不同",
    )
}

pub(super) fn same_path(left: &Path, right: &Path) -> bool {
    if cfg!(windows) {
        left.components()
            .map(|part| part.as_os_str().to_string_lossy().to_lowercase())
            .eq(right
                .components()
                .map(|part| part.as_os_str().to_string_lossy().to_lowercase()))
    } else {
        left.components().eq(right.components())
    }
}

fn verify_authority(
    runtime: &Value,
    documents: &RuntimeDocuments,
    supplied: &Value,
    mut source: Value,
) -> Result<()> {
    let bindings = &documents.bindings.value;
    strict::exact(bindings, &["record", "manifest"], "恢复 bindings")?;
    let record = &bindings["record"];
    let plan = &record["plan"];
    let manifest = &bindings["manifest"];
    strict::object(record, "恢复记录")?;
    strict::object(plan, "恢复产品计划")?;
    strict::object(manifest, "恢复备份清单")?;
    check(
        record["status"] == "data_verified"
            && plan["backup_id"] == manifest["id"]
            && plan["frontend_sha"] == runtime["source"]["frontend_sha"],
        "运行产物必须绑定已完成数据校验的同一演练",
    )?;
    let restore = json!({
        "id":plan["id"], "backup_id":plan["backup_id"], "scope_id":plan["scope_id"],
        "plan_hash":record["plan_hash"], "data_verified_at":record["data_verified_at"],
    });
    let endpoints = json!({
        "api":plan["api_ready_url"], "worker":plan["worker_ready_url"],
        "frontend":supplied["frontend_endpoint"],
    });
    source["backup_source_sha"] = manifest["source_sha"].clone();
    let mut authority = source.clone();
    authority["format_version"] = json!(2);
    authority["kind"] = json!("restore-runtime-authority");
    for (field, input) in [
        ("restore_id", "id"),
        ("backup_id", "backup_id"),
        ("scope_id", "scope_id"),
        ("plan_hash", "plan_hash"),
        ("data_verified_at", "data_verified_at"),
    ] {
        authority[field] = restore[input].clone();
    }
    for role in generation::ROLES {
        authority[format!("{role}_endpoint")] = endpoints[role].clone();
    }
    let paths = &runtime["paths"];
    let request = &documents.launch.value["request"];
    let mut launch_paths = json!({});
    let mut launch_digests = json!({});
    for key in ["bindings", "backend_build", "frontend_build"] {
        launch_paths[key] = paths[key].clone();
        launch_digests[key] = runtime["digests"][key].clone();
    }
    check(
        *supplied == authority
            && runtime["source"] == source
            && runtime["restore"] == restore
            && runtime["endpoints"] == endpoints
            && request["authority"] == authority
            && request["paths"] == launch_paths
            && request["digests"] == launch_digests
            && request["roots"]
                == json!({"backend_product":paths["backend_product_root"],
                "backend_execution":paths["backend_execution_root"], "frontend":paths["frontend_root"]}),
        "恢复运行或启动收据与 stdin 权威恢复上下文不一致",
    )
}

pub(super) fn backend_sources(
    root: &Path,
    sources: &RuntimeSources,
    receipt: &document::Document,
    authority: &Value,
    environment: &Value,
) -> Result<Value> {
    let execution = &sources.source_backend;
    let expected = &authority["backend_execution_sha"];
    build::verify_registered(
        execution,
        receipt,
        strict::text(expected, "后端执行来源")?,
        environment,
    )?;
    let inventory = build::source::registered(
        root,
        execution,
        strict::text(expected, "后端执行来源")?,
        &authority["backend_adapter_contract"],
        &json!(sources.product_backend),
    )?;
    let product = if inventory["product"].is_null() {
        &inventory["inventory"]
    } else {
        &inventory["product"]["inventory"]
    };
    let product = source::validate_inventory(product)?;
    check(
        receipt.value["sources"]["full"] == inventory["inventory"]
            && receipt.value["sources"]["product"] == source::backend_domains(&product)?["product"]
            && authority["backend_product_sha"] == product.source.snapshot.head,
        "恢复构建未保持登记的后端产品或执行来源",
    )?;
    Ok(inventory)
}

pub(super) enum Frontend {
    B0(Box<frontend_build::b0::Proof>),
    Current(Box<frontend_build::Proof>),
}

impl Frontend {
    pub(super) fn capture(
        frontend: &Path,
        registered: &document::Document,
        authority: &Value,
    ) -> Result<Self> {
        let root = build::source::repository(frontend)?;
        let head = strict::text(&authority["frontend_sha"], "恢复前端提交")?;
        check(
            same_path(
                &registered.path,
                &root.join("dist/.vite/restore-build.json"),
            ),
            "恢复前端构建收据未绑定固定生产文件",
        )?;
        let proof = if b0::matches(authority) {
            Self::B0(Box::new(frontend_build::b0::validate(&root, None, head)?))
        } else {
            Self::Current(Box::new(frontend_build::validate(&root, None, Some(head))?))
        };
        let validated = match &proof {
            Self::B0(proof) => &proof.receipt,
            Self::Current(proof) => &proof.receipt,
        };
        check(
            validated == &registered.value,
            "恢复前端构建收据在来源核验期间发生漂移",
        )?;
        Ok(proof)
    }

    pub(super) fn inventory(&self) -> &Value {
        match self {
            Self::B0(proof) => &proof.inventory,
            Self::Current(proof) => &proof.inventory,
        }
    }

    pub(super) fn verify(&self) -> Result<()> {
        match self {
            Self::B0(proof) => proof.verify(),
            Self::Current(proof) => proof.verify(),
        }
    }
}

struct Processes {
    value: Value,
    executables: Vec<StableFile>,
}

impl Processes {
    fn verify(&self, runtime: &Value) -> Result<()> {
        for (role, executable) in generation::ROLES.into_iter().zip(&self.executables) {
            executable.verify()?;
            observe_listener(runtime, role, &self.value[role]["identity"])?;
        }
        Ok(())
    }
}

fn observe_listener(runtime: &Value, role: &str, identity: &Value) -> Result<()> {
    let endpoint = domain::endpoint(&runtime["endpoints"][role], "恢复角色监听端点", None)?;
    process::listener_at(identity, endpoint.host.parse()?, endpoint.port)
}

fn observe_processes(runtime: &Value, commands: &Value, inputs: &mut Inputs) -> Result<Processes> {
    let directory = path(&runtime["paths"]["runtime_dir"])?;
    let mut processes = Processes {
        value: json!({}),
        executables: Vec::with_capacity(generation::ROLES.len()),
    };
    for role in generation::ROLES {
        let registered = &runtime["processes"][role];
        let receipt = path(&registered["receipt_path"])?;
        check(
            same_path(&receipt, &directory.join(format!("{role}.json"))),
            "恢复进程收据不属于本代运行目录",
        )?;
        let document = inputs.read(&receipt)?;
        let (identity, observed) =
            generation::receipts::process(&directory, role, &runtime["restore"]["scope_id"])?;
        check(
            observed["sha256"] == document.sha256()
                && registered["receipt_sha256"] == observed["sha256"]
                && registered["identity"] == identity,
            "恢复进程收据摘要或创建身份不同",
        )?;
        let artifact = &runtime["backend"]["artifacts"][role];
        let expected = if role == "frontend" {
            &commands[role][0]
        } else {
            &artifact["executable"]
        };
        let executable = path(expected)?;
        check(
            same_path(&path(&identity["executable"])?, &executable),
            "恢复进程未运行登记的可执行文件",
        )?;
        let file = StableFile::open(&executable)?;
        let facts = file.facts()?;
        check(
            role == "frontend"
                || (artifact["bytes"] == facts["bytes"] && artifact["sha256"] == facts["sha256"]),
            "恢复进程未运行登记的构建产物",
        )?;
        observe_listener(runtime, role, &identity)?;
        processes.value[role] = json!({"identity":identity,
            "process_receipt":{"path":document.path, "sha256":observed["sha256"]},
            "executable":{"path":executable, "bytes":facts["bytes"], "sha256":facts["sha256"]}});
        processes.executables.push(file);
    }
    Ok(processes)
}

fn probe(runtime: &Value, files: &Value) -> Result<Value> {
    let endpoints = &runtime["endpoints"];
    let readiness = probes::request(&json!({"role":"api","url":endpoints["api"]}))?;
    probes::request(&json!({"role":"worker","url":endpoints["worker"]}))?;
    probes::request(&json!({"role":"frontend","url":endpoints["frontend"],"files":files}))?;
    Ok(readiness)
}

fn probe_and_recheck(
    runtime: &Value,
    files: &Value,
    processes: &Processes,
    verify_sources: impl FnOnce() -> Result<()>,
) -> Result<Value> {
    let observation = probe(runtime, files);
    let after = processes.verify(runtime).and_then(|()| verify_sources());
    finish(observation, after)
}

fn require_frontend_files(files: &Value) -> Result<()> {
    let files = strict::array(files, "恢复前端产物")?;
    check(
        ["index.html", ".vite/manifest.json"]
            .iter()
            .all(|required| {
                files
                    .iter()
                    .any(|file| file["path"].as_str() == Some(*required))
            }),
        "恢复前端缺少生产首页或 Vite manifest",
    )
}

fn finish<T>(observed: Result<T>, after: Result<()>) -> Result<T> {
    match (observed, after) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(after)) => Err(format!("{error}；恢复探针后验同时失败：{after}").into()),
    }
}

fn reply(
    runtime: &document::Document,
    documents: &RuntimeDocuments,
    processes: Value,
    readiness: Value,
) -> Value {
    let value = &runtime.value;
    let descriptor =
        |input: &document::Document| json!({"path":input.path,"sha256":input.sha256()});
    json!({
        "format_version":1, "kind":"restore-runtime-verification", "status":"verified",
        "runtime_receipt_sha256":runtime.sha256(), "bindings":descriptor(&documents.bindings),
        "build_receipts":{"backend":descriptor(&documents.backend),"frontend":descriptor(&documents.frontend),
            "launch":descriptor(&documents.launch)},
        "restore":value["restore"], "source":value["source"], "endpoints":value["endpoints"],
        "processes":processes, "api_readiness":readiness,
        "frontend":{"build_receipt_sha256":documents.frontend.sha256(),"files":documents.frontend.value["files"]},
    })
}
