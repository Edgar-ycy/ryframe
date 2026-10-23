//! 失败库存目录的闭集：原始四库前缀、命令及 stdout 必须全部可归属。
use super::*;

const KEYS: [&str; 4] = ["shared-control", "shared", "dedicated-a", "dedicated-b"];

pub(super) fn files(
    backend: &Path,
    target: &Path,
    relative: &str,
    request: &Value,
    available: &BTreeSet<String>,
) -> Result<BTreeSet<String>> {
    check(
        !relative.contains(['/', '\\', ':']) && !matches!(relative, "" | "." | ".."),
        "库存失败目录包含未知证据",
    )?;
    let directory = target.join(relative);
    let prefix = format!("{relative}/");
    let mut paths = available
        .iter()
        .filter(|name| name.starts_with(&prefix))
        .cloned()
        .collect::<BTreeSet<_>>();
    check(
        available.contains(relative)
            && paths.contains(&format!("{prefix}failure.json"))
            && !paths.contains(&format!("{prefix}inventory.json")),
        "库存失败目录缺少唯一失败收据或错误包含成功收据",
    )?;
    failure(&read(backend, &directory.join("failure.json"))?)?;
    let mut known = BTreeSet::from(["failure.json".to_owned()]);
    let mut counts = Vec::new();
    for phase in ["before", "after"] {
        let mut found = Vec::new();
        for key in KEYS {
            let name = format!("{phase}-target-{key}.json");
            if paths.contains(&format!("{prefix}{name}")) {
                raw_target(&read(backend, &directory.join(&name))?, request, key)?;
                found.push(key);
            }
            known.insert(name);
        }
        check(
            found == KEYS[..found.len()],
            "库存失败原始四库输出不是连续固定前缀",
        )?;
        counts.push(found.len());
    }
    check(
        counts[1] == 0 || counts[0] == 4,
        "库存 after 输出不能越过未完成的 before",
    )?;
    let before = paths.contains(&format!("{prefix}binding-before.json"));
    let after = paths.contains(&format!("{prefix}binding-after.json"));
    check(!after || before, "库存失败来源绑定缺失或前后变化")?;
    if before {
        let original = read(backend, &directory.join("binding-before.json"))?;
        if after {
            check(
                original == read(backend, &directory.join("binding-after.json"))?,
                "库存失败来源绑定缺失或前后变化",
            )?;
        }
        known.insert("binding-before.json".into());
    }
    if after {
        known.insert("binding-after.json".into());
    }
    let pattern = regex::Regex::new(r"^command-[a-f0-9]{32}\.json$")?;
    let mut stdout_files = BTreeSet::new();
    let mut references = BTreeSet::new();
    for path in &paths {
        let name = &path[prefix.len()..];
        if pattern.is_match(name) {
            if let Some(stdout) =
                command(backend, &directory, &read(backend, &directory.join(name))?)?
            {
                check(
                    references.insert(stdout),
                    "库存失败 stdout 文件被多个命令收据重复引用",
                )?;
            }
        } else if !name.contains('/') && name.ends_with(".stdout") {
            stdout_files.insert(name.to_owned());
        } else {
            check(known.contains(name), "库存失败目录包含未知证据")?;
        }
    }
    check(
        stdout_files == references,
        "库存失败 stdout 文件集合与命令收据引用不匹配",
    )?;
    paths.insert(relative.to_owned());
    Ok(paths)
}

fn failure(value: &Value) -> Result<()> {
    let mut fields = vec![
        "format_version",
        "status",
        "stage",
        "error_type",
        "remote_writes",
        "target_ready",
        "clone_verified",
    ];
    if value.get("database_verification").is_some() {
        fields.push("database_verification");
    }
    json_io::exact(value, &fields, "库存失败收据")?;
    check(
        value["format_version"].as_u64() == Some(1)
            && value["status"] == "side_inventory_failed"
            && value["stage"].as_str().is_some_and(|stage| {
                [
                    "inputs",
                    "binding_before",
                    "inventory_before",
                    "inventory_after",
                    "binding_after",
                    "publish",
                ]
                .contains(&stage)
            })
            && value["error_type"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
            && zero(&value["remote_writes"])
            && value["target_ready"] == false
            && value["clone_verified"] == false
            && value
                .get("database_verification")
                .is_none_or(Value::is_object),
        "库存失败收据类型、阶段或只读资格字段无效",
    )
}

fn raw_target(value: &Value, request: &Value, key: &str) -> Result<()> {
    json_io::exact(
        value,
        &[
            "scope_id",
            "control_schema_fingerprint",
            "tenant_schema_fingerprint",
            "target",
        ],
        "库存失败原始目标",
    )?;
    json_io::exact(
        &value["target"],
        &["database", "preserved_tables"],
        "库存失败目标",
    )?;
    let db = &value["target"]["database"];
    json_io::exact(
        db,
        &[
            "key",
            "kind",
            "server_uuid",
            "database",
            "shared",
            "placements",
            "tables",
        ],
        "库存失败数据库",
    )?;
    let declared = json_io::array(&request["target"]["databases"], "目标数据库")?
        .iter()
        .find(|db| db["key"] == key)
        .ok_or("库存失败原始目标输出不属于固定四库")?;
    check(
        value["scope_id"] == request["target"]["scope_id"]
            && ["key", "kind", "server_uuid", "database"]
                .iter()
                .all(|field| db[*field] == declared[*field])
            && db["shared"].as_bool() == Some(declared["mode"] == "shared")
            && db["placements"].is_array()
            && db["tables"].is_array()
            && value["target"]["preserved_tables"].is_array(),
        "库存失败原始目标输出不属于固定四库",
    )
}

fn command(backend: &Path, directory: &Path, value: &Value) -> Result<Option<String>> {
    let mut fields = vec![
        "command",
        "returncode",
        "error_type",
        "stdin",
        "stdout",
        "stderr",
    ];
    let capture_failed = value.get("stdout_capture_error").is_some();
    if value.get("stdout_file").is_some() {
        fields.push("stdout_file");
    }
    if capture_failed {
        fields.push("stdout_capture_error");
    }
    json_io::exact(value, &fields, "库存失败命令收据")?;
    let stdin = &value["stdin"];
    if !stdin.is_null() {
        json_io::exact(stdin, &["bytes", "sha256"], "库存失败标准输入")?;
    }
    check(
        value["command"].as_array().is_some_and(|items| {
            !items.is_empty()
                && items
                    .iter()
                    .all(|item| item.as_str().is_some_and(|item| !item.is_empty()))
        }) && (value["returncode"].is_null() || value["returncode"].as_i64().is_some())
            && (value["error_type"].is_null() || value["error_type"].is_string())
            && (value["stdout"].is_null() || value["stdout"].is_string())
            && value["stderr"].is_string(),
        "库存失败命令收据字段或退出状态无效",
    )?;
    if capture_failed {
        check(
            value.get("stdout_file").is_some()
                && value["stdout_capture_error"]
                    .as_str()
                    .is_some_and(|error| !error.is_empty())
                && value["stdout"].is_null(),
            "库存失败 stdout 采集错误字段无效",
        )?;
    }
    let Some(descriptor) = value.get("stdout_file") else {
        return Ok(None);
    };
    json_io::exact(
        descriptor,
        if capture_failed {
            &["path"]
        } else {
            &["path", "bytes", "sha256"]
        },
        "库存失败 stdout 描述",
    )?;
    let path = Path::new(json_io::text(&descriptor["path"], "库存 stdout 路径")?);
    check(
        path.parent() == Some(directory)
            && path
                .extension()
                .is_some_and(|extension| extension == "stdout"),
        "库存失败 stdout 文件越出采集目录或名称无效",
    )?;
    let actual = binding(backend, path)?;
    check(
        capture_failed || actual == *descriptor,
        "库存失败 stdout 文件绑定变化",
    )?;
    Ok(Some(
        path.file_name()
            .and_then(|value| value.to_str())
            .ok_or("库存失败 stdout 文件描述无效")?
            .to_owned(),
    ))
}
