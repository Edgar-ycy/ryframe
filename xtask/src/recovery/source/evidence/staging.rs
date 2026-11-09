//! 从不可变Git对象构造唯一的原生验收契约快照；中断后保留部分文件供人工核对。
use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::documents;
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path},
    recovery::clone::native::{digest, source},
};

const CONTRACT: &str = "openapi/openapi.json";
const MANIFEST: &str = "manifest.json";
const ENTRY: &str = "__recovery-existing-source";

struct Inputs {
    coordinator: PathBuf,
    execution: PathBuf,
    root: PathBuf,
    source: Value,
    execution_sha: String,
    tool: Value,
    blob: Vec<u8>,
}

fn path(value: &Value) -> Result<PathBuf> {
    let value = value
        .as_str()
        .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
        .ok_or("来源staging缺少明确路径")?;
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err("来源staging必须使用绝对路径".into());
    }
    Ok(path)
}

fn revision(value: &Value) -> Result<String> {
    let value = value
        .as_str()
        .filter(|value| {
            value.len() == 40
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
        .ok_or("来源验收提交必须是精确SHA")?;
    Ok(value.into())
}

fn inputs(args: &Value, existing: bool) -> Result<Inputs> {
    let mut fields = vec![
        "coordinator",
        "execution",
        "directory",
        "coordinator_source",
        "execution_sha",
        "tool",
    ];
    if existing {
        fields.push("descriptor");
    }
    documents::exact(args, &fields, "来源staging参数")?;
    let coordinator = path(&args["coordinator"])?;
    let execution = path(&args["execution"])?;
    validate_absolute_path(&coordinator, LocalTestPathKind::ExistingDirectory)?;
    validate_absolute_path(&execution, LocalTestPathKind::ExistingDirectory)?;
    let directory = path(&args["directory"])?;
    validate_local_test_path(
        &directory,
        &coordinator,
        LocalTestPathKind::ExistingDirectory,
    )?;
    let root = directory.join("source-tools");
    validate_local_test_path(
        &root,
        &coordinator,
        if existing {
            LocalTestPathKind::ExistingDirectory
        } else {
            LocalTestPathKind::NewDirectory
        },
    )?;
    let binding = &args["coordinator_source"];
    validate_source(binding)?;
    let execution_sha = revision(&args["execution_sha"])?;
    let tool = tool(&args["tool"])?;
    let blob = contract(&execution, &execution_sha)?;
    Ok(Inputs {
        coordinator,
        execution,
        root,
        source: binding.clone(),
        execution_sha,
        tool,
        blob,
    })
}

pub(super) fn validate_source(value: &Value) -> Result<()> {
    documents::exact(
        value,
        &["snapshot", "worktree_fingerprint", "fingerprints"],
        "来源验收协调器",
    )?;
    // 复用完整来源模型，避免另一套SHA、路径与fingerprint解释。
    let inventory = source::validate_inventory(&json!({
        "source":{"snapshot":value["snapshot"],"worktree_fingerprint":value["worktree_fingerprint"]},
        "files":[],"guard":{"head":value["snapshot"]["head"],"index_sha256":"0".repeat(64),"modes_sha256":"0".repeat(64)},
    }))?;
    documents::exact(
        &value["fingerprints"],
        &["product", "test_tools", "support"],
        "来源分域摘要",
    )?;
    for group in ["product", "test_tools", "support"] {
        let item = &value["fingerprints"][group];
        documents::exact(item, &["sha256", "files"], "来源分域摘要")?;
        if item["files"].as_u64().is_none()
            || item["sha256"].as_str().is_none_or(|value| {
                value.len() != 64
                    || !value
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            })
        {
            return Err("来源分域摘要无效".into());
        }
    }
    if !inventory.source.snapshot.clean {
        return Err("来源工具staging要求干净协调器".into());
    }
    Ok(())
}

fn tool(value: &Value) -> Result<Value> {
    documents::exact(value, &["path", "sha256"], "来源验收原生工具")?;
    let path = path(&value["path"])?;
    validate_absolute_path(&path, LocalTestPathKind::ExistingFile)?;
    let actual = descriptor(&path)?;
    if actual["sha256"] != value["sha256"] {
        return Err("来源验收原生工具摘要变化".into());
    }
    Ok(actual)
}

pub(super) fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let mut command = Command::new("git");
    command.arg("-C").arg(root).args(args);
    for (key, _) in std::env::vars_os() {
        if key
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
        {
            command.env_remove(key);
        }
    }
    command
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_REPLACE_OBJECTS", "1");
    let result = command.output().map_err(|_| "无法读取来源验收Git对象")?;
    if !result.status.success() {
        return Err("来源验收Git对象不存在或不可读取".into());
    }
    Ok(result.stdout)
}

fn contract(root: &Path, sha: &str) -> Result<Vec<u8>> {
    let tree = git(root, &["ls-tree", "-r", "-z", sha, "--", CONTRACT])?;
    let entries = tree
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
        .collect::<Vec<_>>();
    if entries.len() != 1 {
        return Err("来源验收执行快照缺少唯一OpenAPI契约".into());
    }
    let entry = std::str::from_utf8(entries[0]).map_err(|_| "来源Git树路径无效")?;
    let (header, name) = entry.split_once('\t').ok_or("来源Git树路径无效")?;
    let parts = header.split(' ').collect::<Vec<_>>();
    if parts.len() != 3
        || !matches!(parts[0], "100644" | "100755")
        || parts[1] != "blob"
        || name != CONTRACT
    {
        return Err("来源验收Git树包含非普通文件或无效路径".into());
    }
    git(root, &["cat-file", "blob", &format!("{sha}:{CONTRACT}")])
}

fn descriptor(path: &Path) -> Result<Value> {
    let actual = digest::file(path)?;
    Ok(json!({"path":path,"bytes":actual.bytes,"sha256":actual.sha256}))
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut stream = OpenOptions::new().create_new(true).write(true).open(path)?;
    stream.write_all(bytes)?;
    stream.sync_all()?;
    Ok(())
}

impl Inputs {
    fn manifest(&self) -> Value {
        let contract = self.root.join(CONTRACT);
        json!({"format_version":2,"kind":"restore-source-native-staging",
            "coordinator_source":self.source,"execution_sha":self.execution_sha,"entry":ENTRY,
            "contract":contract,"runtime_modules":[self.tool],
            "files":[{"path":contract,"origin":"execution","bytes":self.blob.len(),
                "sha256":hex::encode(Sha256::digest(&self.blob))}]})
    }

    fn unchanged(&self, args: &Value) -> Result<()> {
        if tool(&args["tool"])? != self.tool
            || contract(&self.execution, &self.execution_sha)? != self.blob
        {
            return Err("来源staging读取期间工具或Git对象变化".into());
        }
        Ok(())
    }
}

pub(super) fn create(args: &Value) -> Result<Value> {
    let inputs = inputs(args, false)?;
    // create_dir不复用既有目录；中途失败留下原样证据，调用方不得覆盖重试。
    fs::create_dir(&inputs.root)?;
    fs::create_dir(inputs.root.join("openapi"))?;
    write_new(&inputs.root.join(CONTRACT), &inputs.blob)?;
    inputs.unchanged(args)?;
    let mut bytes = serde_json::to_vec_pretty(&inputs.manifest())?;
    bytes.push(b'\n');
    write_new(&inputs.root.join(MANIFEST), &bytes)?;
    let output = descriptor(&inputs.root.join(MANIFEST))?;
    verify(&{
        let mut args = args.clone();
        args["descriptor"] = output.clone();
        args
    })?;
    Ok(output)
}

fn files(root: &Path, directory: &Path, result: &mut BTreeSet<String>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        let metadata = fs::symlink_metadata(&path)?;
        if digest::linked(&metadata) {
            return Err("来源工具staging包含链接或重解析点".into());
        }
        if metadata.is_dir() {
            if path != root.join("openapi") {
                return Err("来源工具staging包含未知目录".into());
            }
            files(root, &path, result)?;
        } else if metadata.is_file() {
            result.insert(
                path.strip_prefix(root)?
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        } else {
            return Err("来源工具staging包含非普通文件".into());
        }
    }
    Ok(())
}

pub(super) fn verify(args: &Value) -> Result<Value> {
    let inputs = inputs(args, true)?;
    let path = inputs.root.join(MANIFEST);
    documents::exact(
        &args["descriptor"],
        &["path", "bytes", "sha256"],
        "来源工具staging绑定",
    )?;
    if args["descriptor"] != descriptor(&path)? {
        return Err("来源工具staging清单摘要或路径不同".into());
    }
    validate_local_test_path(
        &inputs.root.join(CONTRACT),
        &inputs.coordinator,
        LocalTestPathKind::ExistingFile,
    )?;
    if fs::read(inputs.root.join(CONTRACT))? != inputs.blob {
        return Err("来源工具staging与已登记Git blob不同".into());
    }
    let mut observed = BTreeSet::new();
    files(&inputs.root, &inputs.root, &mut observed)?;
    if observed != [MANIFEST.into(), CONTRACT.into()].into_iter().collect() {
        return Err("来源工具staging文件集合包含缺失或未知项".into());
    }
    let manifest = documents::read(&path)?;
    if manifest != inputs.manifest() || descriptor(&path)? != args["descriptor"] {
        return Err("来源工具staging清单、来源或摘要不同".into());
    }
    inputs.unchanged(args)?;
    Ok(
        json!({"root":inputs.root,"entry":ENTRY,"contract":inputs.root.join(CONTRACT),
        "manifest":manifest,"descriptor":args["descriptor"]}),
    )
}
