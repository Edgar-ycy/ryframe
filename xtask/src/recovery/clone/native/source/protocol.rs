use serde_json::{Value, json};

use super::{Inventory, domains, native_json};
use crate::Result;

/// 完整来源清单的私有委托；不是编译绑定 source-inventory.json 的解析入口。
pub(crate) fn request(args: &Value) -> Result<Value> {
    let action = native_json::text(&args["action"], "来源清单操作")?;
    match action {
        "domain" => {
            native_json::exact(args, &["action", "path"], "来源路径分类")?;
            Ok(json!(domains::source_domain(native_json::text(
                &args["path"],
                "源码路径"
            )?)?))
        }
        "fingerprints" => {
            native_json::exact(args, &["action", "files"], "来源分组摘要")?;
            super::fingerprints(&super::parse_files(&args["files"], "完整来源文件")?)
        }
        "validate" => {
            native_json::exact(args, &["action", "inventory"], "来源清单校验")?;
            Ok(super::inventory_value(&super::validate_inventory(
                &args["inventory"],
            )?))
        }
        "domains" | "validate-domains" => domain_request(args, action),
        "snapshot"
        | "source-snapshot"
        | "file-inventory"
        | "capture"
        | "worktree-fingerprint"
        | "maintenance-target-directory" => observation(args, action),
        _ => Err("来源清单操作无效".into()),
    }
}

fn domain_request(args: &Value, action: &str) -> Result<Value> {
    let field = if action == "domains" {
        "inventory"
    } else {
        "value"
    };
    native_json::exact(args, &["action", field, "repository"], "来源角色分域")?;
    let repository = native_json::text(&args["repository"], "来源仓库类型")?;
    if !matches!(repository, "backend" | "frontend") {
        return Err("构建来源仓库类型无效".into());
    }
    let value = &args[field];
    let inventory = super::validate_inventory(if action == "domains" {
        value
    } else {
        native_json::exact(value, &["product", "tools", "full"], "构建来源分域")?;
        &value["full"]
    })?;
    let expected = build_domains(&inventory, repository)?;
    if action == "validate-domains" && value != &expected {
        return Err("构建来源分域与完整来源清单不一致".into());
    }
    Ok(expected)
}

pub(super) fn build_domains(inventory: &Inventory, repository: &str) -> Result<Value> {
    if repository == "backend" {
        return super::backend_domains(inventory);
    }
    let (files, _) = super::file_inventory_for(&inventory.files)?;
    let product = inventory
        .files
        .iter()
        .filter(|file| domains::frontend_product(&file.path))
        .cloned()
        .collect::<Vec<_>>();
    let tools = inventory
        .files
        .iter()
        .filter(|file| domains::frontend_tool(&file.path))
        .cloned()
        .collect::<Vec<_>>();
    Ok(json!({
        "product":{"frontend":super::source_group(&files, &product)?},
        "tools":super::source_group(&files, &tools)?,
        "full":super::inventory_value(inventory),
    }))
}

fn observation(args: &Value, action: &str) -> Result<Value> {
    let fields = if action == "worktree-fingerprint" {
        vec!["action", "root", "commit"]
    } else if action == "capture" {
        vec!["action", "root", "source"]
    } else {
        vec!["action", "root"]
    };
    native_json::exact(args, &fields, "Git 来源观察")?;
    let root = std::path::Path::new(native_json::text(&args["root"], "源码根目录")?);
    if !root.is_absolute() {
        return Err("源码根目录必须是绝对路径".into());
    }
    let top = super::text_output(super::git(root, &["rev-parse", "--show-toplevel"])?)?;
    if std::fs::canonicalize(top)? != std::fs::canonicalize(root)? {
        return Err("恢复构建必须使用实际 Git 仓库根目录".into());
    }
    match action {
        "maintenance-target-directory" => {
            Ok(json!(super::super::maintenance::target_directory(root)?))
        }
        "worktree-fingerprint" => Ok(json!(super::worktree_fingerprint(
            root,
            native_json::text(&args["commit"], "源码提交")?
        )?)),
        "source-snapshot" => Ok(super::snapshot_value(&super::snapshot(root)?)),
        "snapshot" => {
            let snapshot = super::snapshot(root)?;
            let patch = super::git(root, &["diff", "--binary", "HEAD"])?;
            use sha2::{Digest, Sha256};
            if hex::encode(Sha256::digest(&patch)) != snapshot.patch_sha256 {
                return Err("采集快照期间源码补丁发生变化".into());
            }
            let mut value = super::snapshot_value(&snapshot);
            value.as_object_mut().expect("固定快照对象").remove("clean");
            Ok(json!({"snapshot":value,"patch":patch}))
        }
        "file-inventory" => {
            let (files, guard) = super::file_inventory(root)?;
            Ok(json!({"files":super::files_value(&files),"head":guard.head,
                "index_sha256":guard.index_sha256,"modes_sha256":guard.modes_sha256}))
        }
        "capture" => {
            let inventory = super::capture(root)?;
            if !args["source"].is_null()
                && args["source"] != super::snapshot_value(&inventory.source.snapshot)
            {
                return Err("采集指纹期间完整源码发生变化".into());
            }
            Ok(super::inventory_value(&inventory))
        }
        _ => Err("Git 来源观察操作无效".into()),
    }
}
