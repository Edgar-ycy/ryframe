//! 固定 B0 旧构建的独立生产与核验，不进入普通 v3 证明模型。
use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::{
    fail,
    files::{self, Artifacts, StableFile},
    model, source,
};
use crate::{Result, recovery::clone::native::json as strict};

#[path = "b0/inputs.rs"]
mod inputs;
#[path = "b0/toolchain.rs"]
mod toolchain;

pub(crate) const HEAD: &str = "0087ea2ecf62530d042b9e52f5c950fb34c66d78";
pub(crate) const KIND: &str = "restore-legacy-b0-frontend-build";
pub(crate) const CONTRACT: &str = "legacy-stable-readiness-b0-frontend-v1";
const TREE: &str = "52fed4e1321ab4b4c04839a3a165c6bd1645e4b6";
const COMMAND: [&str; 3] = ["corepack", "pnpm", "build"];

pub(crate) struct Proof {
    pub(crate) inventory: Value,
    pub(crate) receipt: Value,
    root: PathBuf,
    registered: StableFile,
    program: StableFile,
    identity: StableFile,
    inputs: inputs::Inputs,
    tools: toolchain::Tools,
    artifacts: Artifacts,
}

impl Proof {
    pub(crate) fn verify(&self) -> Result<()> {
        self.check_held()?;
        if inputs::inventory(&self.root)? != self.inventory
            || files::artifacts(&self.root)?.files != self.artifacts.files
        {
            return Err(fail("B0 源码清单或完整 dist 在核验期间变化"));
        }
        self.check_held()
    }

    fn check_held(&self) -> Result<()> {
        self.registered.verify()?;
        self.program.verify()?;
        self.identity.verify()?;
        self.inputs.verify()?;
        self.tools.verify()?;
        self.artifacts.verify()
    }
}

pub(crate) fn build(root: &Path) -> Result<Proof> {
    let root = files::directory(root, root)?;
    let dist = root.join("dist");
    match fs::symlink_metadata(&dist) {
        Ok(_) => return validate(&root, None, HEAD),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let inventory = inputs::inventory(&root)?;
    let source_files = inputs::protect_source(&root, &inventory)?;
    let inputs = inputs::Inputs::observe(&root, &inventory)?;
    let tools = toolchain::Tools::discover(&root)?;
    let program = inputs::stable(&std::env::current_exe()?)?;
    tools.observe_versions(&root)?;
    check_build_inputs(&root, &inventory, &inputs, &tools, &program)?;
    // 原先不存在时原子占用输出；失败留下本次目录，后续只允许完整收据复用。
    fs::create_dir(&dist)?;
    let built = tools.build(&root);
    let checked = check_build_inputs(&root, &inventory, &inputs, &tools, &program);
    match (built, checked) {
        (Ok(()), Ok(())) => {}
        (Err(error), Ok(())) | (Ok(()), Err(error)) => return Err(error),
        (Err(error), Err(check)) => {
            return Err(format!("{error}；B0 输入复核同时失败：{check}").into());
        }
    }
    let identity = emitted_identity(&root)?;
    let artifacts = files::artifacts(&root)?;
    let value = json!({
        "format_version":1,"kind":KIND,
        "adapter":{"contract":CONTRACT,"sources":adapter_sources(),"program":inputs::binding(&program)},
        "sources":domains(&inventory)?,
        "build":{"command":COMMAND,"mode":"production","toolchain":tools.value,
            "configuration":inputs.value,"identity":{"frontend_commit":HEAD,"source_file":null}},
        "files":artifacts.files,
    });
    receipt(&value)?;
    identity.verify()?;
    artifacts.verify()?;
    check_build_inputs(&root, &inventory, &inputs, &tools, &program)?;
    strict::write_new(&dist.join(files::RECEIPT), &value)?;
    let proof = validate(&root, Some(&inventory), HEAD)?;
    if proof.receipt != value {
        return Err(fail("B0 适配收据发布后与本次构建不同"));
    }
    drop(source_files);
    Ok(proof)
}

fn check_build_inputs(
    root: &Path,
    inventory: &Value,
    inputs: &inputs::Inputs,
    tools: &toolchain::Tools,
    program: &StableFile,
) -> Result<()> {
    inputs.verify()?;
    tools.verify()?;
    program.verify()?;
    if inputs::inventory(root)? != *inventory {
        return Err(fail("B0 构建前后完整干净源码、树或索引变化"));
    }
    Ok(())
}

pub(crate) fn validate(root: &Path, inventory: Option<&Value>, expected: &str) -> Result<Proof> {
    if expected != HEAD {
        return Err(fail("B0 适配收据仅用于固定前端 HEAD"));
    }
    let root = files::directory(root, root)?;
    let observed = inputs::inventory(&root)?;
    if inventory.is_some_and(|value| *value != observed) {
        return Err(fail("B0 适配收据不属于调用方给定的完整源码清单"));
    }
    let (registered, value) = files::document(&root.join("dist").join(files::RECEIPT))?;
    receipt(&value)?;
    let program = inputs::bound(&value["adapter"]["program"])?;
    let inputs = inputs::Inputs::observe(&root, &observed)?;
    let tools = toolchain::Tools::registered(&root, &value["build"]["toolchain"])?;
    let identity = emitted_identity(&root)?;
    let artifacts = files::artifacts(&root)?;
    if value["sources"] != domains(&observed)?
        || value["build"]["configuration"] != inputs.value
        || model::normalized_files(&value["files"])? != artifacts.files
    {
        return Err(fail("B0 收据不同于实际源码、依赖、配置或完整产物"));
    }
    let proof = Proof {
        inventory: observed,
        receipt: value,
        root,
        registered,
        program,
        identity,
        inputs,
        tools,
        artifacts,
    };
    proof.verify()?;
    Ok(proof)
}

pub(crate) fn receipt(value: &Value) -> Result<()> {
    model::exact(
        value,
        &[
            "format_version",
            "kind",
            "adapter",
            "sources",
            "build",
            "files",
        ],
    )?;
    let adapter = &value["adapter"];
    let build = &value["build"];
    model::exact(adapter, &["contract", "sources", "program"])?;
    model::exact(
        build,
        &["command", "mode", "toolchain", "configuration", "identity"],
    )?;
    if value["format_version"].as_u64() != Some(1)
        || value["kind"] != KIND
        || adapter["contract"] != CONTRACT
        || adapter["sources"] != adapter_sources()
        || build["command"] != json!(COMMAND)
        || build["mode"] != "production"
        || build["identity"] != json!({"frontend_commit":HEAD,"source_file":null})
    {
        return Err(fail("必须使用精确 B0 前端适配收据"));
    }
    inputs::descriptor(&adapter["program"])?;
    inputs::shape(&build["configuration"])?;
    toolchain::shape(&build["toolchain"])?;
    source::protocol::request(
        &json!({"action":"validate-domains","repository":"frontend","value":value["sources"]}),
    )?;
    let inventory = source::validate_inventory(&value["sources"]["full"])?;
    inputs::require_head(&inventory)?;
    model::artifact_records(&value["files"])?;
    if !["index.html", ".vite/manifest.json", "build-identity.json"]
        .iter()
        .all(|name| {
            value["files"]
                .as_array()
                .is_some_and(|rows| rows.iter().any(|row| row["path"] == *name))
        })
    {
        return Err(fail("B0 前端产物清单缺少必需的索引、manifest 或身份"));
    }
    Ok(())
}

fn adapter_sources() -> Value {
    let sources: [(&str, &[u8]); 3] = [
        (
            "xtask/src/recovery/frontend_build/b0.rs",
            include_bytes!("b0.rs"),
        ),
        (
            "xtask/src/recovery/frontend_build/b0/inputs.rs",
            include_bytes!("b0/inputs.rs"),
        ),
        (
            "xtask/src/recovery/frontend_build/b0/toolchain.rs",
            include_bytes!("b0/toolchain.rs"),
        ),
    ];
    json!(
        sources
            .map(|(path, bytes)| json!({"path":path,"sha256":hex::encode(Sha256::digest(bytes))}))
    )
}

fn domains(inventory: &Value) -> Result<Value> {
    source::protocol::request(
        &json!({"action":"domains","repository":"frontend","inventory":inventory}),
    )
}

fn emitted_identity(root: &Path) -> Result<StableFile> {
    let (file, identity) = files::document(&root.join("dist/build-identity.json"))?;
    if identity != json!({"frontend_commit":HEAD}) {
        return Err(fail("B0 Vite 实际身份输出未绑定固定前端 HEAD"));
    }
    Ok(file)
}
