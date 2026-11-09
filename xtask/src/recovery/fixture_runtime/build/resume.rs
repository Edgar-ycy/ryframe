//! 只续接已完成四角色构建及配置快照的暂停前缀，不执行构建或资源操作。
use super::{Inputs, backend_build, configuration, context, registration_guard};
use crate::{
    Result,
    recovery::{
        clone::native::{bridge, json::exact, source},
        fixture_control,
        fixture_runtime::{model::*, runtime},
        full_stack,
    },
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

const DOCUMENTS: [&str; 4] = [
    "source-pair.json",
    "binaries.json",
    "build-evidence.json",
    "backend-build.json",
];
const ROLES: [&str; 4] = [
    "ryframe",
    "ryframe-worker",
    "ryframe-reset",
    "ryframe-migrate",
];

#[derive(PartialEq, Eq)]
struct Prefix {
    documents: Vec<Value>,
    guards: Vec<Value>,
    inventories: Vec<(PathBuf, source::Inventory)>,
    configurations: (PathBuf, PathBuf),
    directories: Vec<(PathBuf, (u64, u64))>,
}

pub(super) fn run(
    inputs: &Inputs,
    output: &Path,
    command: &mut impl FnMut(&str, &[String]) -> Result<String>,
) -> Result<Value> {
    let process = registration_guard(inputs, output)?;
    let prefix = capture(inputs, output, command, false)?;
    let protected = inputs.protect(prefix.paths()?)?;
    check(
        capture(inputs, output, command, false)? == prefix,
        "保护暂停前缀期间输入变化",
    )?;
    protected.check()?;
    process.check()?;
    let (api, worker) = &prefix.configurations;
    let receipt = fixture_control::artifact::runtime::register(&inputs.root, output, api, worker)?;
    protected.check()?;
    process.check()?;
    check(
        capture(inputs, output, command, true)? == prefix,
        "续接运行登记期间输入变化",
    )?;
    runtime::Runtime::load(&inputs.coordinator, &inputs.bootstrap, output)?.verify()?;
    protected.check()?;
    inputs.verify()?;
    process.check()?;
    Ok(
        json!({"status":"reference_fixture_runtime_built","resumed":true,
        "source_pair":bound(&output.join("source-pair.json"))?,
        "backend_build":bound(&output.join("backend-build.json"))?,
        "runtime":bound(&output.join("runtime.json"))?,
        "scope_id":receipt["scope_id"],"remote_writes":0}),
    )
}

impl Prefix {
    fn paths(&self) -> Result<Vec<PathBuf>> {
        let mut paths = self
            .guards
            .iter()
            .map(|guard| path(&guard["descriptor"]["path"]))
            .collect::<Result<Vec<_>>>()?;
        for (root, inventory) in &self.inventories {
            paths.extend(inventory.files.iter().map(|file| root.join(&file.path)));
        }
        Ok(paths)
    }
}

fn capture(
    inputs: &Inputs,
    output: &Path,
    command: &mut impl FnMut(&str, &[String]) -> Result<String>,
    registered: bool,
) -> Result<Prefix> {
    inputs.verify()?;
    let mut names = vec!["artifacts"];
    names.extend(DOCUMENTS);
    if registered {
        names.extend(["runtime.json", "runtime-evidence.json"]);
    }
    exact_directory(output, &names)?;
    let configurations = configuration::verify(&inputs.root, output, &inputs.configuration)?;
    let sidecar = configurations.0.parent().ok_or("旁置配置缺少父目录")?;
    exact_directory(sidecar, &["config", "worker-config"])?;
    for configuration in [&configurations.0, &configurations.1] {
        exact_directory(configuration, &["app.toml", "app.test.toml"])?;
    }
    let captured = DOCUMENTS
        .into_iter()
        .map(|name| document(&output.join(name)))
        .collect::<Result<Vec<_>>>()?;
    let documents = captured
        .iter()
        .map(|value| value.0.clone())
        .collect::<Vec<_>>();
    let mut guards = captured
        .into_iter()
        .map(|value| value.1)
        .collect::<Vec<_>>();
    let provenance = full_stack::provenance::current(&inputs.root, output)?;
    check(
        documents[0]
            == fixture_control::source_pair::execution::pair(
                &inputs.coordinator,
                &inputs.root,
                &inputs.execution_source,
            )?,
        "暂停前缀的执行来源组合变化",
    )?;
    verify_artifacts(inputs, output, &documents[1], &documents[2], &provenance)?;
    let backend = source::capture(&inputs.root)?;
    bridge::verify_build(
        &inputs.root,
        &output.join("backend-build.json"),
        &documents[3],
    )?;
    check(
        documents[3] == backend_build(&documents[1], &backend, &context(inputs, command)?)?,
        "暂停构建的完整来源、工具链、并发或构建环境变化；保留原前缀，不能补签",
    )?;
    let frontend = path(&provenance["roots"]["frontend"])?;
    let frontend_inventory = source::capture(&frontend)?;
    let directories = [
        output.to_path_buf(),
        output.join("artifacts"),
        sidecar.to_path_buf(),
        configurations.0.clone(),
        configurations.1.clone(),
    ]
    .into_iter()
    .map(|path| runtime::configuration_identity(&path).map(|identity| (path, identity)))
    .collect::<Result<Vec<_>>>()?;
    for configuration in [&configurations.0, &configurations.1] {
        for name in ["app.toml", "app.test.toml"] {
            guards.push(snapshot(&configuration.join(name), MAX_BYTES)?);
        }
    }
    for role in ROLES {
        guards.push(snapshot(&path(&documents[1][role])?, MAX_BYTES)?);
    }
    let tools = inputs.root.join(".local-tests/tools.toml");
    if tools.try_exists()? {
        guards.push(snapshot(&tools, MAX_BYTES)?);
    }
    inputs.verify()?;
    Ok(Prefix {
        documents,
        guards,
        inventories: vec![
            (inputs.root.clone(), backend),
            (frontend, frontend_inventory),
        ],
        configurations,
        directories,
    })
}

fn verify_artifacts(
    inputs: &Inputs,
    output: &Path,
    binaries: &Value,
    build: &Value,
    provenance: &Value,
) -> Result<()> {
    exact(binaries, &ROLES, "暂停二进制清单")?;
    exact(
        build,
        &[
            "format_version",
            "kind",
            "backend_root",
            "source",
            "artifacts",
            "build_jobs",
        ],
        "暂停全栈构建证据",
    )?;
    check(
        build["format_version"] == 1
            && build["kind"] == "full-stack-build"
            && build["backend_root"] == inputs.root.to_string_lossy().as_ref()
            && build["source"] == *provenance
            && build["build_jobs"].as_u64() == Some(u64::from(bridge::build_jobs(&inputs.root)?)),
        "暂停全栈构建的来源或并发与当前输入不匹配",
    )?;
    exact(&build["artifacts"], &ROLES, "暂停全栈产物")?;
    let artifacts = output.join("artifacts");
    let mut names = Vec::new();
    for role in ROLES {
        let executable = path(&binaries[role])?;
        let name = format!("{role}{}", std::env::consts::EXE_SUFFIX);
        check(
            executable.parent() == Some(artifacts.as_path())
                && executable.file_name() == Some(std::ffi::OsStr::new(&name))
                && build["artifacts"][role] == bound(&executable)?,
            "暂停产物路径、字节或摘要与原构建证据不匹配",
        )?;
        names.push(name);
    }
    exact_directory(
        &artifacts,
        &names.iter().map(String::as_str).collect::<Vec<_>>(),
    )
}

fn exact_directory(path: &Path, names: &[&str]) -> Result<()> {
    directory(path)?;
    let observed = fs::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<std::io::Result<BTreeSet<_>>>()?;
    let expected = names
        .iter()
        .map(|name| OsString::from(*name))
        .collect::<BTreeSet<_>>();
    check(
        observed == expected,
        "暂停构建前缀不完整或存在未知文件；保留现场，不能自动重放",
    )
}
