use super::*;

type Counts = BTreeMap<String, usize>;

fn read_record(path: &Path) -> Result<Value> {
    documents::read(path).map_err(|_| "来源动态采集记录不是有效的唯一字段JSON普通文件".into())
}

fn counter(values: impl IntoIterator<Item = String>) -> Counts {
    let mut counts = Counts::new();
    for value in values {
        *counts.entry(value).or_default() += 1;
    }
    counts
}

fn subset(actual: &Counts, expected: &Counts, complete: bool, message: &str) -> Result<()> {
    if actual
        .iter()
        .any(|(key, count)| *count > *expected.get(key).unwrap_or(&0))
        || complete && actual != expected
    {
        return Err(message.into());
    }
    Ok(())
}

pub(super) fn query_paths(
    directory: &Path,
    inputs: &[Value],
    observed: &[Value],
    allowed: &mut BTreeSet<String>,
) -> Result<()> {
    if directory.join("recovery-audit").exists()
        && !directory
            .join("source-verification-recovery-intent.json")
            .is_file()
    {
        return Err("恢复登录审计必须继承明确恢复 intent".into());
    }
    let pattern = r"mysql-shared-control-[a-f0-9]{32}\.command\.json";
    let audit_pattern = regex::Regex::new(&format!("^audit/{pattern}$"))?;
    let candidates = observed
        .iter()
        .filter_map(|row| row["path"].as_str())
        .filter(|path| path.starts_with("recovery-audit/") || audit_pattern.is_match(path))
        .collect::<Vec<_>>();
    if candidates.is_empty() {
        return Ok(());
    }
    let commands = selected(inputs, &format!("^audit/{pattern}$"))?
        .into_iter()
        .map(|name| Ok(read_record(&relative(directory, name))?["command"].clone()))
        .collect::<Result<Vec<_>>>()?;
    let command = commands
        .first()
        .ok_or("恢复登录审计缺少原同一控制库查询命令")?;
    if commands.iter().any(|item| item != command) {
        return Err("恢复登录审计缺少原同一控制库查询命令".into());
    }
    let allowed_pattern = regex::Regex::new(&format!("^(?:audit|recovery-audit)/{pattern}$"))?;
    for name in candidates {
        if !allowed_pattern.is_match(name) {
            return Err("恢复登录审计包含未知文件或子目录".into());
        }
        let value = read_record(&relative(directory, name))?;
        documents::exact(
            &value,
            &["command", "returncode", "error_type", "stdout", "stderr"],
            "恢复控制库查询记录",
        )?;
        if value["command"] != *command
            || !value["stdout"].is_string()
            || !value["stderr"].is_string()
            || !value["returncode"].is_null() && value["returncode"].as_i64().is_none()
            || !value["error_type"].is_null() && !value["error_type"].is_string()
        {
            return Err("恢复登录审计改变了原查询命令或记录格式".into());
        }
        allowed.insert(name.into());
    }
    Ok(())
}

fn capture_identity(path: &Path, category: &str) -> Result<String> {
    let mut value = read_record(path)?;
    if category == "storage-layout" {
        let mut fields = vec![
            "process_receipt",
            "launch_receipt",
            "actual_arguments_sha256",
            "data_dir",
            "environment_proof",
        ];
        if value.get("runtime_transition").is_some() {
            fields.push("runtime_transition");
        }
        documents::exact(&value, &fields, "完整像存储布局记录")?;
    } else {
        documents::exact(
            &value,
            &["command", "returncode", "error_type", "stdout", "stderr"],
            "完整像缓存身份命令",
        )?;
        let command = documents::array(&value["command"], "缓存身份命令")?;
        if command.len() < 2
            || command
                .iter()
                .any(|part| part.as_str().is_none_or(str::is_empty))
            || value["returncode"].as_i64() != Some(0)
            || !value["error_type"].is_null()
            || value["stderr"] != ""
            || !value["stdout"].is_string()
        {
            return Err("完整像缓存身份采集命令未成功或字段无效".into());
        }
        let proc_pattern = regex::Regex::new(r"^/proc/([1-9][0-9]*)/stat$")?;
        if command[command.len() - 2] == "/usr/bin/cat"
            && let Some(matched) = command
                .last()
                .and_then(Value::as_str)
                .and_then(|s| proc_pattern.captures(s))
        {
            let stdout = documents::text(&value["stdout"], "缓存内核身份输出")?.trim();
            let (process, suffix) = stdout
                .rsplit_once(')')
                .ok_or("完整像缓存内核身份输出不完整")?;
            let fields = suffix.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 20
                || fields[19].is_empty()
                || !fields[19].bytes().all(|b| b.is_ascii_digit())
                || !process.starts_with(&format!("{} (", &matched[1]))
            {
                return Err("完整像缓存内核身份输出不完整".into());
            }
            value["stdout"] = json!({"process":format!("{process})"),"started":fields[19]});
        }
    }
    Ok(serde_json::to_string(&value)?)
}

pub(super) fn image_paths(
    directory: &Path,
    inputs: &[Value],
    observed: &[Value],
    allowed: &mut BTreeSet<String>,
) -> Result<()> {
    let complete = observed.iter().any(|row| row["path"] == "after/image.json");
    for (category, pattern, count) in [
        (
            "redis-kernel",
            r"redis-kernel-[a-f0-9]{32}\.command\.json",
            10,
        ),
        ("storage-layout", r"storage-layout-[a-f0-9]{32}\.json", 2),
    ] {
        let original = selected(inputs, &format!("^before/{pattern}$"))?;
        let current = selected(observed, &format!("^after/{pattern}$"))?;
        if original.is_empty() && current.is_empty() {
            continue;
        }
        if original.len() != count {
            return Err("完整像动态采集记录缺少原始完整类别与数量".into());
        }
        let identities = |paths: &[&str]| -> Result<Counts> {
            Ok(counter(
                paths
                    .iter()
                    .map(|name| capture_identity(&relative(directory, name), category))
                    .collect::<Result<Vec<_>>>()?,
            ))
        };
        subset(
            &identities(&current)?,
            &identities(&original)?,
            complete,
            "完整像动态采集记录改变了原类别、命令、身份或数量",
        )?;
        allowed.extend(current.into_iter().map(str::to_owned));
    }
    Ok(())
}

fn database_identity(path: &Path, category: &str) -> Result<(String, Option<String>)> {
    let mut value = read_record(path)?;
    let mut stdout_path = None;
    if let Some(stdout) = value.get("stdout_file").cloned().filter(|v| !v.is_null()) {
        documents::exact(
            &stdout,
            &["path", "bytes", "sha256"],
            "完整像数据库标准输出",
        )?;
        let bound = PathBuf::from(documents::text(&stdout["path"], "数据库标准输出路径")?);
        let pattern = regex::Regex::new(r"^mysql-(identity|ownership)-[a-f0-9]{32}\.stdout$")?;
        let matched = bound
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(|s| pattern.captures(s))
            .ok_or("完整像数据库标准输出未绑定明确采集文件")?;
        if bound.parent() != path.parent() || descriptor(&bound)? != stdout {
            return Err("完整像数据库标准输出未绑定同目录明确采集文件".into());
        }
        value["stdout_file"]["path"] = json!(format!("mysql-{}-UUID.stdout", &matched[1]));
        stdout_path = Some(bound);
    }
    if category == "command" {
        let mut fields = vec![
            "command",
            "returncode",
            "error_type",
            "stdin",
            "stdout",
            "stderr",
        ];
        if stdout_path.is_some() {
            fields.push("stdout_file");
        }
        documents::exact(&value, &fields, "完整像数据库命令")?;
        let command = documents::array(&value["command"], "数据库命令")?;
        if command.is_empty()
            || command
                .iter()
                .any(|part| part.as_str().is_none_or(str::is_empty))
            || !value["stdout"].is_string()
            || value["stderr"] != ""
        {
            return Err("完整像数据库命令格式无效".into());
        }
        if let Some(index) = command.iter().position(|part| part == "--output") {
            let output = Path::new(
                command
                    .get(index + 1)
                    .and_then(Value::as_str)
                    .ok_or("数据库输出参数缺失")?,
            );
            let name = output
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or("数据库输出文件名缺失")?;
            if output.parent() != path.parent() || !regex::Regex::new(r"^(?:before|after)-target-(?:shared-control|shared|dedicated-a|dedicated-b)\.json$")?.is_match(name) {
                return Err("完整像数据库命令输出越出明确目标目录".into());
            }
            value["command"][index + 1] = json!(format!("<databases>/{name}"));
        }
    } else {
        documents::exact(
            &value,
            &[
                "format_version",
                "kind",
                "check",
                "target_key",
                "returncode",
                "error_type",
                "stderr_bytes",
                "stderr_sha256",
                "stdout_file",
            ],
            "完整像数据库验证输出",
        )?;
        if value["format_version"].as_u64() != Some(1)
            || value["kind"] != "mysql-verification-output"
            || value["check"] != category
            || !["shared-control", "shared", "dedicated-a", "dedicated-b"]
                .iter()
                .any(|key| value["target_key"] == *key)
            || value["stderr_bytes"].as_u64() != Some(0)
            || value["stderr_sha256"] != hex::encode(sha2::Sha256::digest([]))
            || stdout_path.as_deref() != Some(path.with_extension("stdout").as_path())
        {
            return Err("完整像数据库验证输出类别、目标或错误状态无效".into());
        }
    }
    if value["returncode"].as_i64() != Some(0) || !value["error_type"].is_null() {
        return Err("完整像数据库采集未成功".into());
    }
    Ok((
        serde_json::to_string(&value)?,
        stdout_path.and_then(|p| p.file_name().and_then(|s| s.to_str()).map(str::to_owned)),
    ))
}

pub(super) fn database_paths(
    directory: &Path,
    inputs: &[Value],
    observed: &[Value],
    registered: &mut BTreeSet<String>,
) -> Result<()> {
    let mut allowed = BTreeSet::new();
    let complete = observed.iter().any(|row| row["path"] == "after/image.json");
    let mut references = BTreeMap::new();
    for (category, pattern, count) in [
        ("command", r"command-[a-f0-9]{32}\.json", 48),
        ("identity", r"mysql-identity-[a-f0-9]{32}\.json", 16),
        ("ownership", r"mysql-ownership-[a-f0-9]{32}\.json", 24),
    ] {
        let original = selected(inputs, &format!("^before/databases/{pattern}$"))?;
        let current = selected(observed, &format!("^after/databases/{pattern}$"))?;
        if original.is_empty() && current.is_empty() {
            continue;
        }
        if original.len() != count {
            return Err("完整像数据库动态采集缺少原始完整类别与数量".into());
        }
        let expected = counter(
            original
                .iter()
                .map(|name| Ok(database_identity(&relative(directory, name), category)?.0))
                .collect::<Result<Vec<_>>>()?,
        );
        let mut actual = Vec::new();
        let mut stdout = Vec::new();
        for name in current {
            let (identity, output) = database_identity(&relative(directory, name), category)?;
            actual.push(identity);
            allowed.insert(name.into());
            if let Some(output) = output {
                allowed.insert(format!("after/databases/{output}"));
                stdout.push(output);
            }
        }
        subset(
            &counter(actual),
            &expected,
            complete,
            "完整像数据库动态采集改变了原命令、响应或数量",
        )?;
        references.insert(category, counter(stdout));
    }
    let commands = references.remove("command").unwrap_or_default();
    let mut diagnostics = references.remove("identity").unwrap_or_default();
    for (name, count) in references.remove("ownership").unwrap_or_default() {
        *diagnostics.entry(name).or_default() += count;
    }
    if commands
        .values()
        .chain(diagnostics.values())
        .any(|count| *count != 1)
        || complete && commands != diagnostics
    {
        return Err("完整像数据库命令与诊断未一一绑定同一标准输出文件".into());
    }
    for (category, count) in [("identity", 16), ("ownership", 24)] {
        let pattern = format!("^after/databases/mysql-{category}-[a-f0-9]{{32}}\\.stdout$");
        let current = selected(observed, &pattern)?
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        let pattern = regex::Regex::new(&pattern)?;
        let referenced = allowed
            .iter()
            .filter(|name| pattern.is_match(name))
            .cloned()
            .collect::<BTreeSet<_>>();
        if current != referenced || complete && !current.is_empty() && current.len() != count {
            return Err("完整像数据库动态标准输出集合或数量不完整".into());
        }
    }
    registered.extend(allowed);
    Ok(())
}

use sha2::Digest;
