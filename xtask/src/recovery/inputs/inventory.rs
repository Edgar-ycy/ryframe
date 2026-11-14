use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

use super::{
    document::{array, boolean, digest, exact, integer, linked, object, text},
    domain::{BUCKETS, EXCLUDED_TABLES, file_sha256, file_size, hash_40, identifier, timestamp},
};
use crate::Result;

mod held;
pub(in crate::recovery) use held::HeldArtifacts;

pub(crate) fn validate(plan: &Value, inventory: &Value) -> Result<()> {
    let inventory = object(inventory, "备份清单")?;
    if inventory.contains_key("observed_at") || !inventory.contains_key("quiesced_at") {
        return Err("正式备份必须使用停机库存，不能采用运行中观察库存".into());
    }
    let quiesced = timestamp(&inventory["quiesced_at"], "备份停机时间")?;
    let captured = timestamp(
        inventory.get("captured_at").ok_or("备份清单缺少采集时间")?,
        "备份采集时间",
    )?;
    if quiesced > captured
        || !quiesced.timestamp_subsec_nanos().is_multiple_of(1_000)
        || !captured.timestamp_subsec_nanos().is_multiple_of(1_000)
    {
        return Err("备份停机与采集时间必须有序且符合微秒精度".into());
    }
    hash_40(
        inventory.get("source_sha").ok_or("备份清单缺少源码 SHA")?,
        "备份源码 SHA",
    )?;
    validate_schema(inventory)?;
    if text(inventory.get("id").ok_or("备份清单缺少 ID")?, "备份清单 ID")?
        != text(&plan["id"], "参考计划 ID")?
        || text(
            inventory.get("scope_id").ok_or("备份清单缺少 scope")?,
            "备份清单 scope",
        )? != text(
            plan.get("source")
                .and_then(|value| value.get("scope_id"))
                .ok_or("参考计划缺少源 scope")?,
            "源 scope",
        )?
    {
        return Err("备份清单 ID 或 scope 与资源计划不一致".into());
    }
    let expected = array(
        plan.get("source")
            .and_then(|value| value.get("databases"))
            .ok_or("参考计划缺少源数据库")?,
        "源数据库",
    )?
    .iter()
    .map(|database| Ok((text(&database["key"], "源数据库键")?, database)))
    .collect::<Result<HashMap<_, _>>>()?;
    let databases = array(
        inventory.get("databases").ok_or("备份清单缺少数据库")?,
        "备份数据库",
    )?;
    if databases.len() != expected.len()
        || databases
            .iter()
            .map(|database| text(&database["key"], "备份数据库键"))
            .collect::<Result<HashSet<_>>>()?
            != expected.keys().copied().collect()
    {
        return Err("备份清单必须完整覆盖精确数据库目标".into());
    }
    for database in databases {
        let key = text(&database["key"], "备份数据库键")?;
        validate_database(database, expected[key])?;
    }
    let objects = array(
        inventory.get("objects").ok_or("备份清单缺少对象")?,
        "备份对象",
    )?;
    let names = objects
        .iter()
        .map(|value| text(&value["bucket"], "对象桶"))
        .collect::<Result<HashSet<_>>>()?;
    if objects.len() != BUCKETS.len() || names != BUCKETS.into_iter().collect() {
        return Err("备份清单必须恰好包含五个业务对象桶".into());
    }
    let prefix = format!(
        "{}/",
        text(inventory.get("scope_id").unwrap(), "备份 scope")?
    );
    for objects in objects {
        exact(objects, &["bucket", "prefix", "entries"], "备份对象桶")?;
        if text(objects.get("prefix").unwrap(), "对象前缀")? != prefix {
            return Err("对象清单越过明确 scope、重复或包含 ownership 标记".into());
        }
        let mut keys = HashSet::new();
        for entry in array(objects.get("entries").unwrap(), "对象条目")? {
            let entry = exact(entry, &["key", "bytes", "sha256"], "对象条目")?;
            let key = text(entry.get("key").ok_or("对象条目缺少 key")?, "对象键")?;
            let sha = digest(entry.get("sha256").ok_or("对象条目缺少摘要")?, "对象摘要")?;
            let bytes = integer(entry.get("bytes").ok_or("对象条目缺少大小")?, "对象大小")?;
            if !keys.insert(key)
                || !key.starts_with(&prefix)
                || key == format!("{prefix}.ryframe-owner")
                || key.chars().any(|character| (character as u32) < 32)
                || bytes == 0
            {
                return Err("对象清单越过明确 scope、重复或包含 ownership 标记".into());
            }
            digest(&Value::String(sha), "对象摘要")?;
        }
    }
    Ok(())
}

fn validate_schema(inventory: &serde_json::Map<String, Value>) -> Result<()> {
    let control = text(
        inventory
            .get("control_schema_fingerprint")
            .ok_or("备份清单缺少控制 schema 指纹")?,
        "控制 schema 指纹",
    )?;
    if control.len() != 16
        || !control
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("控制 schema 指纹必须为 16 位小写十六进制".into());
    }
    digest(
        inventory
            .get("tenant_schema_fingerprint")
            .ok_or("备份清单缺少租户 schema 指纹")?,
        "租户 schema 指纹",
    )?;
    Ok(())
}

fn validate_database(database: &Value, source: &Value) -> Result<()> {
    let fields = exact(
        database,
        &[
            "key",
            "database",
            "server_uuid",
            "kind",
            "shared",
            "placements",
            "tables",
        ],
        "备份数据库",
    )?;
    for field in ["database", "server_uuid", "kind"] {
        if fields.get(field) != source.get(field) {
            return Err("备份数据库与计划中的物理身份不匹配".into());
        }
    }
    let shared = boolean(&fields["shared"], "备份共享模式")?;
    if shared != (source["mode"].as_str() == Some("shared")) {
        return Err("备份数据库共享模式与计划不匹配".into());
    }
    validate_placements(array(&fields["placements"], "备份 placement")?, shared)?;
    let mut tables = HashSet::new();
    for table in array(&fields["tables"], "备份表")? {
        let table = exact(table, &["table", "rows", "sha256"], "备份表")?;
        let name = text(&table["table"], "备份表名")?;
        identifier(name)?;
        integer(&table["rows"], "备份表行数")?;
        digest(&table["sha256"], "备份表摘要")?;
        if !tables.insert(name) || EXCLUDED_TABLES.contains(&name) {
            return Err("备份表缺失、重复或包含必须保留的目标元数据".into());
        }
    }
    if tables.is_empty() {
        return Err("备份表缺失、重复或包含必须保留的目标元数据".into());
    }
    Ok(())
}

fn validate_placements(placements: &[Value], shared: bool) -> Result<()> {
    if !shared && placements.len() > 1 {
        return Err("独立目标只能包含一个租户".into());
    }
    let mut tenants = HashSet::new();
    for placement in placements {
        let placement = exact(
            placement,
            &["tenant_id", "generation", "switch_token"],
            "备份 placement",
        )?;
        let tenant = text(&placement["tenant_id"], "租户 ID")?;
        identifier(tenant)?;
        identifier(text(&placement["switch_token"], "租户切换令牌")?)?;
        if !tenants.insert(tenant)
            || placement["generation"]
                .as_i64()
                .is_none_or(|value| value <= 0)
        {
            return Err("备份租户 placement 无效或重复".into());
        }
    }
    Ok(())
}

pub(crate) fn verify_artifacts(root: &Path, manifest: &Value) -> Result<()> {
    visit_artifacts(root, manifest, |path, bytes, sha| {
        if file_sha256(&path)? != sha || file_size(&path)? != bytes || bytes == 0 {
            return Err("备份文件大小或摘要被篡改".into());
        }
        Ok(())
    })
}

fn visit_artifacts(
    root: &Path,
    manifest: &Value,
    mut visit: impl FnMut(PathBuf, u64, &str) -> Result<()>,
) -> Result<()> {
    let manifest = object(manifest, "备份清单")?;
    let databases = array(
        manifest.get("databases").ok_or("备份清单缺少数据库")?,
        "备份数据库",
    )?;
    let mut required = databases
        .iter()
        .map(|database| {
            Ok(format!(
                "db:{}",
                text(database.get("key").unwrap(), "数据库键")?
            ))
        })
        .collect::<Result<HashSet<_>>>()?;
    required.extend(
        BUCKETS
            .into_iter()
            .map(|bucket| format!("objects:{bucket}")),
    );
    let mut resources = HashSet::new();
    let mut paths = HashSet::new();
    for artifact in array(
        manifest.get("artifacts").ok_or("备份清单缺少产物")?,
        "备份产物",
    )? {
        let artifact = exact(
            artifact,
            &["relative_path", "resource", "bytes", "sha256"],
            "备份产物",
        )?;
        let relative = text(artifact.get("relative_path").unwrap(), "产物路径")?;
        let resource = text(artifact.get("resource").unwrap(), "产物资源")?;
        if !paths.insert(relative.to_owned()) || !required.contains(resource) {
            return Err("备份产物重复或引用未知目标".into());
        }
        let path = safe_file(root, relative)?;
        let bytes = integer(artifact.get("bytes").unwrap(), "产物大小")?;
        let sha = digest(artifact.get("sha256").unwrap(), "产物摘要")?;
        visit(path, bytes, &sha)?;
        resources.insert(resource.to_owned());
    }
    if resources != required {
        return Err("备份产物未覆盖全部数据库和对象范围".into());
    }
    Ok(())
}

fn safe_file(root: &Path, relative: &str) -> Result<PathBuf> {
    if relative.is_empty()
        || relative.contains(['\\', ':'])
        || relative.chars().any(|character| (character as u32) < 32)
        || relative
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err("备份文件路径越界或格式无效".into());
    }
    let candidate = root.join(relative);
    let mut current = root.to_path_buf();
    for component in Path::new(relative).components() {
        current.push(component.as_os_str());
        let metadata =
            fs::symlink_metadata(&current).map_err(|_| "备份文件缺失或无法核验".to_owned())?;
        if linked(&metadata) {
            return Err("备份文件通过链接越界".into());
        }
    }
    if !fs::canonicalize(&candidate)?.starts_with(fs::canonicalize(root)?) {
        return Err("备份文件通过链接越界".into());
    }
    Ok(candidate)
}
