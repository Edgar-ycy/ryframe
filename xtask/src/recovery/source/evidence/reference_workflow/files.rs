use std::{
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::{exact, inputs, native, path, text, unlinked};
use crate::Result;

pub(super) fn work(args: &Value, create: bool) -> Result<Value> {
    exact(args, &["plan"])?;
    let plan = &args["plan"];
    let work = path(&plan["work_dir"])?;
    inputs::domain::identifier(text(&plan["id"])?)?;
    unlinked(work)?;
    let owner =
        json!({"format_version":1,"id":plan["id"],"plan_sha256":native::digest::plan_hash(plan)?});
    if work.exists() {
        if native::json::read(&work.join("reference-owner.json"))? != owner {
            return Err("演练目录已存在且ownership不匹配".into());
        }
    } else if create {
        if let Some(parent) = work.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::create_dir(work)?;
        native::json::write_new(&work.join("reference-owner.json"), &owner)?;
    } else {
        return Err("参考环境目录没有对应的计划ownership".into());
    }
    Ok(json!(work))
}

fn roots(args: &Value) -> Result<(&Path, &Path)> {
    let work = path(&args["work"])?;
    let root = path(&args["root"])?;
    unlinked(work)?;
    unlinked(root)?;
    let actual_work = fs::canonicalize(work)?;
    let actual_root = fs::canonicalize(root)?;
    if !actual_root.starts_with(&actual_work) || actual_root == actual_work {
        return Err("参考备份根目录必须位于本次明确演练目录内".into());
    }
    Ok((work, root))
}

pub(super) fn copy(args: &Value) -> Result<Value> {
    exact(args, &["work", "root", "manifest", "copy_id"])?;
    let (work, root) = roots(args)?;
    let id = text(&args["copy_id"])?;
    inputs::domain::identifier(id)?;
    let manifest = &args["manifest"];
    super::artifact_shape(manifest)?;
    if manifest["id"] == id {
        return Err("失败演练必须使用独立备份ID".into());
    }
    inputs::inventory::verify_artifacts(root, manifest)?;
    let target = work.join(id);
    unlinked(&target)?;
    fs::create_dir(&target)?;
    for artifact in native::json::array(&manifest["artifacts"], "备份产物")? {
        let relative = text(&artifact["relative_path"])?;
        let source = native::digest::safe_file(root, relative)?;
        let destination = target.join(relative);
        fs::create_dir_all(destination.parent().ok_or("备份产物缺少父目录")?)?;
        unlinked(&destination)?;
        let mut input = fs::File::open(source)?;
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&destination)?;
        std::io::copy(&mut input, &mut output)?;
        output.sync_all()?;
    }
    let mut copied = manifest.clone();
    copied["id"] = json!(id);
    inputs::inventory::verify_artifacts(&target, &copied)?;
    inputs::inventory::verify_artifacts(root, manifest)?;
    native::json::write_new(&target.join("manifest.json"), &copied)?;
    Ok(
        json!({"manifest":target.join("manifest.json"),"backup_root":target,
        "next":"先登记有效副本并执行 restore-begin，然后用 damage 注入损坏，再执行 restore-verify-data"}),
    )
}

pub(super) fn damage(args: &Value) -> Result<Value> {
    exact(args, &["work", "root", "manifest", "relative", "missing"])?;
    let (work, root) = roots(args)?;
    if fs::canonicalize(root)? == fs::canonicalize(work.join("backup"))? {
        return Err("只允许损坏当前演练目录内的独立备份副本".into());
    }
    let manifest = &args["manifest"];
    super::artifact_shape(manifest)?;
    inputs::inventory::verify_artifacts(root, manifest)?;
    let relative = text(&args["relative"])?;
    if !native::json::array(&manifest["artifacts"], "备份产物")?
        .iter()
        .any(|entry| entry["relative_path"] == relative)
    {
        return Err("损坏文件必须精确登记在该副本清单".into());
    }
    let file = native::digest::safe_file(root, relative)?;
    let metadata = fs::symlink_metadata(&file)?;
    if native::resources::common::link_count_for(&file, &metadata)? != 1 {
        return Err("损坏注入不能修改共享硬链接产物".into());
    }
    let missing = native::json::boolean(&args["missing"], "missing")?;
    if missing {
        fs::remove_file(file)?;
    } else {
        let mut stream = OpenOptions::new().read(true).write(true).open(file)?;
        let mut first = [0];
        stream.read_exact(&mut first)?;
        stream.seek(SeekFrom::Start(0))?;
        first[0] ^= 1;
        stream.write_all(&first)?;
        stream.sync_all()?;
    }
    Ok(
        json!({"backup_id":manifest["id"],"artifact":relative,"injected":if missing {"missing"} else {"corrupt"}}),
    )
}

pub(super) fn verify_artifact(
    root: &Path,
    manifest: &Value,
    relative: &str,
) -> Result<(PathBuf, native::digest::FileDigest)> {
    let file = native::digest::safe_file(root, relative)?;
    let artifact = native::json::array(&manifest["artifacts"], "备份产物")?
        .iter()
        .find(|entry| entry["relative_path"] == relative)
        .ok_or("恢复产物未登记")?;
    let digest = native::digest::file(&file)?;
    if artifact["bytes"] != digest.bytes || artifact["sha256"] != digest.sha256 {
        return Err("恢复写入前当前产物字节与备份清单不同".into());
    }
    Ok((file, digest))
}
