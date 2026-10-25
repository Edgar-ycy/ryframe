//! 原生库存的分层MySQL收据必须指向同一通用命令与完整stdout，不扩大命令白名单。
use super::*;
use sha2::{Digest, Sha256};

pub(super) fn receipt(backend: &Path, directory: &Path, name: &str, request: &Value) -> Result<()> {
    let pattern = regex::Regex::new(r"^mysql-(identity|ownership)(-retry)?-[a-f0-9]{32}\.json$")?;
    let captures = pattern.captures(name).ok_or("库存失败原生收据名称无效")?;
    let value = read(backend, &directory.join(name))?;
    if captures.get(2).is_some() {
        return retry(backend, directory, &value);
    }
    let mut fields = vec![
        "format_version",
        "kind",
        "check",
        "target_key",
        "returncode",
        "error_type",
        "stderr_bytes",
        "stderr_sha256",
        "stdout_file",
    ];
    if value.get("stdout_capture_error").is_some() {
        fields.push("stdout_capture_error");
    }
    json_io::exact(&value, &fields, "原生MySQL失败收据")?;
    check(
        value["format_version"] == 1
            && value["kind"] == "mysql-verification-output"
            && value["check"].as_str() == Some(&captures[1])
            && (value["returncode"].is_null() || value["returncode"].as_i64().is_some())
            && (value["error_type"].is_null() || value["error_type"].is_string()),
        "原生MySQL失败收据类型无效",
    )?;
    let db = json_io::array(&request["target"]["databases"], "库存四库")?
        .iter()
        .find(|db| db["key"] == value["target_key"])
        .ok_or("原生MySQL失败收据目标未登记")?;
    let sql = super::super::super::database::evidence::verification_sql(&captures[1])?;
    let expected = json!([
        request["tools"]["mysql"]["path"],
        format!(
            "--defaults-file={}",
            json_io::text(&db["defaults_file"], "MySQL配置")?
        ),
        format!("--database={}", json_io::text(&db["database"], "物理库")?),
        "--batch",
        "--raw",
        "--skip-column-names",
        "--binary-mode",
        "--default-character-set=utf8mb4",
        "--execute",
        sql
    ]);
    let commands = super::names(directory, r"^command-[a-f0-9]{32}\.json$")?
        .iter()
        .map(|n| read(backend, &directory.join(n)))
        .collect::<Result<Vec<_>>>()?;
    let paired = commands
        .iter()
        .filter(|v| v.get("stdout_file") == Some(&value["stdout_file"]))
        .collect::<Vec<_>>();
    check(
        paired.len() == 1
            && paired[0]["command"] == expected
            && paired[0]["stdin"].is_null()
            && paired[0]["returncode"] == value["returncode"],
        "原生MySQL失败收据未绑定唯一固定只读命令",
    )?;
    let path = Path::new(json_io::text(
        &value["stdout_file"]["path"],
        "MySQL stdout",
    )?);
    check(
        path.parent() == Some(directory) && path.file_stem() == directory.join(name).file_stem(),
        "原生MySQL失败收据stdout不属于同一观察",
    )?;
    if value["stderr_bytes"].is_null() {
        check(
            value["stderr_sha256"].is_null(),
            "原生MySQL失败stderr描述不完整",
        )?;
    } else {
        check(
            value["stderr_bytes"].as_u64().is_some(),
            "原生MySQL失败stderr长度无效",
        )?;
        super::super::super::super::input::digest_shape(&value["stderr_sha256"])?;
    }
    if let Some(error) = value.get("stdout_capture_error") {
        check(
            error.as_str().is_some_and(|v| !v.is_empty())
                && paired[0].get("stdout_capture_error") == Some(error),
            "原生MySQL失败采集错误未绑定",
        )?;
    }
    Ok(())
}

fn retry(backend: &Path, directory: &Path, value: &Value) -> Result<()> {
    json_io::exact(
        value,
        &[
            "format_version",
            "kind",
            "check",
            "target_key",
            "reason",
            "first",
            "second",
        ],
        "原生MySQL只读重试",
    )?;
    check(
        value["format_version"] == 1
            && value["kind"] == "mysql-verification-retry"
            && value["reason"] == "first_returncode_zero_stdout_stderr_exactly_empty",
        "原生MySQL重试原因无效",
    )?;
    let empty = hex::encode(Sha256::digest([]));
    let mut paths = BTreeSet::new();
    for key in ["first", "second"] {
        let path = bound(backend, &value[key])?;
        check(
            path.parent() == Some(directory) && paths.insert(path.clone()),
            "原生MySQL重试不属于唯一观察",
        )?;
        let record = read(backend, &path)?;
        check(
            record["kind"] == "mysql-verification-output"
                && record["check"] == value["check"]
                && record["target_key"] == value["target_key"],
            "原生MySQL重试目标或检查变化",
        )?;
        if key == "first" {
            check(
                record["returncode"] == 0
                    && record["error_type"].is_null()
                    && record["stderr_bytes"] == 0
                    && record["stdout_file"]["bytes"] == 0
                    && record["stderr_sha256"] == empty
                    && record["stdout_file"]["sha256"] == empty,
                "原生MySQL重试前像不为空成功",
            )?;
        }
    }
    Ok(())
}
