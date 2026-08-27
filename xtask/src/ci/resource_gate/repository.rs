use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    path::Path,
};

use serde::Deserialize;

use crate::process::command_output;

use super::model::{
    ChangeStatus, ChangedFile, OWNERSHIP_PATH, OwnershipEntry, OwnershipManifest,
    ResourceDefinition, ResourceGateInput,
};
use crate::check::load_workspace_graph;

#[derive(Debug)]
pub(crate) struct LoadedRepository {
    pub(crate) base: String,
    pub(crate) head: String,
    pub(crate) input: ResourceGateInput,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnershipWire {
    format_version: u16,
    generator_version: String,
    entries: Vec<OwnershipEntryWire>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnershipEntryWire {
    resource: String,
    root: String,
    path: String,
    source_hash: String,
    #[serde(default)]
    schema_hash: Option<String>,
    #[serde(default)]
    schema_revision: Option<String>,
    content_hash: String,
}

pub(crate) fn load(root: &Path) -> Result<LoadedRepository, String> {
    let base_ref = configured_base_ref()
        .ok_or_else(|| "缺少 RYFRAME_CI_BASE_SHA 或 GITHUB_BASE_SHA".to_owned())?;
    let head_ref = env::var("GITHUB_SHA")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "HEAD".to_owned());
    let base = resolve_commit(root, &base_ref)
        .map_err(|reason| format!("base ref 无效（{base_ref}）：{reason}"))?;
    let head = resolve_commit(root, &head_ref)
        .map_err(|reason| format!("head ref 无效（{head_ref}）：{reason}"))?;
    let changes = load_changes(root, &base, &head)?;
    let base_resources = load_resources(root, &base)?;
    let head_resources = load_resources(root, &head)?;
    let base_ownership = load_ownership(root, &base)?;
    let head_ownership = load_ownership(root, &head)?;
    let workspace_graph = load_workspace_graph(root).map_err(|error| error.to_string())?;
    Ok(LoadedRepository {
        base,
        head,
        input: ResourceGateInput {
            changes,
            base_resources,
            head_resources,
            base_ownership,
            head_ownership,
            workspace_graph,
        },
    })
}

fn configured_base_ref() -> Option<String> {
    env::var("RYFRAME_CI_BASE_SHA")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            env::var("GITHUB_BASE_SHA")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
}

fn resolve_commit(root: &Path, reference: &str) -> Result<String, String> {
    let commit_ref = format!("{reference}^{{commit}}");
    let output = command_output(root, "git", &["rev-parse", "--verify", &commit_ref])
        .map_err(|error| error.to_string())?;
    let commit = output.trim();
    if commit.len() != 40 || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("解析结果不是 40 位 Git commit".to_owned());
    }
    Ok(commit.to_ascii_lowercase())
}

fn load_changes(root: &Path, base: &str, head: &str) -> Result<Vec<ChangedFile>, String> {
    let output = command_output(
        root,
        "git",
        &[
            "diff",
            "--name-status",
            "-z",
            "--find-renames",
            base,
            head,
            "--",
        ],
    )
    .map_err(|error| error.to_string())?;
    parse_name_status(&output)
}

pub(crate) fn parse_name_status(output: &str) -> Result<Vec<ChangedFile>, String> {
    let tokens = output
        .split('\0')
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    let mut changes = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let status = tokens[index];
        index += 1;
        let code = status
            .bytes()
            .next()
            .ok_or_else(|| "git diff 返回空状态".to_owned())?;
        match code {
            b'A' | b'M' => {
                let path = tokens
                    .get(index)
                    .ok_or_else(|| format!("git diff 状态 {status} 缺少路径"))?;
                index += 1;
                changes.push(ChangedFile {
                    status: if code == b'A' {
                        ChangeStatus::Added
                    } else {
                        ChangeStatus::Modified
                    },
                    path: normalize(path),
                    old_path: None,
                });
            }
            b'D' => {
                let path = tokens
                    .get(index)
                    .ok_or_else(|| "git diff 删除状态缺少路径".to_owned())?;
                index += 1;
                changes.push(ChangedFile {
                    status: ChangeStatus::Deleted,
                    path: normalize(path),
                    old_path: None,
                });
            }
            b'R' | b'C' => {
                let old_path = tokens
                    .get(index)
                    .ok_or_else(|| "git diff 重命名状态缺少旧路径".to_owned())?;
                let path = tokens
                    .get(index + 1)
                    .ok_or_else(|| "git diff 重命名状态缺少新路径".to_owned())?;
                index += 2;
                changes.push(ChangedFile {
                    status: ChangeStatus::Renamed,
                    path: normalize(path),
                    old_path: Some(normalize(old_path)),
                });
            }
            _ => return Err(format!("不支持的 git diff 状态：{status}")),
        }
    }
    Ok(changes)
}

fn load_resources(
    root: &Path,
    commit: &str,
) -> Result<BTreeMap<String, ResourceDefinition>, String> {
    let output = command_output(
        root,
        "git",
        &[
            "ls-tree",
            "-r",
            "--name-only",
            commit,
            "--",
            "catalog/resources",
        ],
    )
    .map_err(|error| error.to_string())?;
    let mut resources = BTreeMap::new();
    for path in output.lines().map(str::trim).filter(|path| {
        path.starts_with("catalog/resources/")
            && path.ends_with(".toml")
            && *path != OWNERSHIP_PATH
            && !path["catalog/resources/".len()..].contains('/')
    }) {
        let source = show_file(root, commit, path)?;
        let definition = parse_resource_definition(path, &source)?;
        if resources.insert(path.to_owned(), definition).is_some() {
            return Err(format!("Git tree 包含重复资源清单：{path}"));
        }
    }
    Ok(resources)
}

pub(crate) fn parse_resource_definition(
    path: &str,
    source: &str,
) -> Result<ResourceDefinition, String> {
    let document = toml::from_str::<toml::Value>(source)
        .map_err(|error| format!("资源清单 {path} 不是有效 TOML：{error}"))?;
    let name = document
        .get("resource")
        .and_then(toml::Value::as_table)
        .and_then(|resource| resource.get("name"))
        .and_then(toml::Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .ok_or_else(|| format!("资源清单 {path} 缺少 resource.name"))?
        .to_owned();
    let relations = document
        .get("relations")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
        .map(|relation| {
            relation
                .as_table()
                .and_then(|table| table.get("target_resource"))
                .and_then(toml::Value::as_str)
                .filter(|target| !target.trim().is_empty())
                .map(str::to_owned)
                .ok_or_else(|| format!("资源清单 {path} 的 relation 缺少 target_resource"))
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    Ok(ResourceDefinition {
        name,
        relations,
        document,
    })
}

fn load_ownership(root: &Path, commit: &str) -> Result<OwnershipManifest, String> {
    let source = show_file(root, commit, OWNERSHIP_PATH)
        .map_err(|reason| format!("无法读取 {commit} 的 ownership manifest：{reason}"))?;
    parse_ownership(&source)
}

pub(crate) fn parse_ownership(source: &str) -> Result<OwnershipManifest, String> {
    let wire = toml::from_str::<OwnershipWire>(source)
        .map_err(|error| format!("ownership manifest 格式错误：{error}"))?;
    let entries = wire
        .entries
        .into_iter()
        .map(|entry| OwnershipEntry {
            resource: entry.resource,
            root: entry.root,
            path: normalize(&entry.path),
            fingerprint: [
                entry.source_hash,
                entry.schema_hash.unwrap_or_default(),
                entry.schema_revision.unwrap_or_default(),
                entry.content_hash,
            ]
            .join(":"),
        })
        .collect();
    Ok(OwnershipManifest {
        format_version: wire.format_version,
        generator_version: wire.generator_version,
        entries,
    })
}

fn show_file(root: &Path, commit: &str, path: &str) -> Result<String, String> {
    let object = format!("{commit}:{path}");
    command_output(root, "git", &["show", &object]).map_err(|error| error.to_string())
}

fn normalize(path: &str) -> String {
    path.replace('\\', "/")
}
