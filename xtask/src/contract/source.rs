use std::{collections::BTreeSet, fmt, fs, path::Path, process::Output};

use serde::de::{Error as _, MapAccess, SeqAccess, Visitor};

use crate::{Result, process::child_command};

use super::model::sha256_hex;

const EXPECTED_OPENAPI_PATH: &str = "openapi/openapi.json";
const SOURCE_KEYS: [&str; 6] = [
    "backend_commit",
    "backend_repository",
    "openapi_path",
    "openapi_version",
    "schema_version",
    "sha256",
];

#[derive(Debug, PartialEq, Eq)]
struct ContractSource {
    backend_commit: String,
    openapi_path: String,
    openapi_version: String,
    sha256: String,
}

struct StrictJsonValue(serde_json::Value);

struct StrictJsonVisitor;

impl<'de> serde::Deserialize<'de> for StrictJsonValue {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictJsonVisitor).map(Self)
    }
}

impl<'de> Visitor<'de> for StrictJsonVisitor {
    type Value = serde_json::Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("不含重复对象键的 JSON 值")
    }

    fn visit_bool<E>(self, value: bool) -> std::result::Result<Self::Value, E> {
        Ok(value.into())
    }

    fn visit_i64<E>(self, value: i64) -> std::result::Result<Self::Value, E> {
        Ok(value.into())
    }

    fn visit_u64<E>(self, value: u64) -> std::result::Result<Self::Value, E> {
        Ok(value.into())
    }

    fn visit_f64<E>(self, value: f64) -> std::result::Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(serde_json::Value::Number)
            .ok_or_else(|| E::custom("JSON 数字必须是有限值"))
    }

    fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E> {
        Ok(value.into())
    }

    fn visit_string<E>(self, value: String) -> std::result::Result<Self::Value, E> {
        Ok(value.into())
    }

    fn visit_unit<E>(self) -> std::result::Result<Self::Value, E> {
        Ok(serde_json::Value::Null)
    }

    fn visit_seq<A>(self, mut sequence: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut values = Vec::with_capacity(sequence.size_hint().unwrap_or_default());
        while let Some(value) = sequence.next_element::<StrictJsonValue>()? {
            values.push(value.0);
        }
        Ok(serde_json::Value::Array(values))
    }

    fn visit_map<A>(self, mut entries: A) -> std::result::Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut object = serde_json::Map::new();
        while let Some(key) = entries.next_key::<String>()? {
            if object.contains_key(&key) {
                return Err(A::Error::custom(format!(
                    "来源元数据包含重复对象键 {key:?}"
                )));
            }
            let value = entries.next_value::<StrictJsonValue>()?;
            object.insert(key, value.0);
        }
        Ok(serde_json::Value::Object(object))
    }
}

pub(crate) fn verify_contract_source(
    backend_worktree: &Path,
    backend_head: &str,
    expected_repository: &str,
    source_metadata: &Path,
    frontend_openapi: &Path,
    candidate_openapi: &Path,
) -> Result<String> {
    if !valid_repository(expected_repository) {
        return Err("期望后端仓库必须是 owner/repository".into());
    }
    let source = parse_source_metadata(source_metadata, expected_repository)?;
    let head = verified_commit(backend_worktree, backend_head, "后端 HEAD")?;
    let checkout_head = git_text(
        backend_worktree,
        &["rev-parse", "--verify", "HEAD^{commit}"],
    )?;
    if checkout_head.trim() != head {
        return Err("后端工作树 HEAD 与待校验提交不一致".into());
    }
    verified_commit(backend_worktree, &source.backend_commit, "正式契约来源提交")?;
    verify_ancestor(backend_worktree, &source.backend_commit, &head)?;

    let object = format!("{}:{}", source.backend_commit, source.openapi_path);
    let source_bytes = git_output(backend_worktree, &["show", &object])?;
    let frontend_bytes = load_bytes(frontend_openapi, "前端正式 OpenAPI")?;
    let candidate_bytes = load_bytes(candidate_openapi, "后端候选 OpenAPI")?;
    if source_bytes != frontend_bytes {
        return Err("来源提交中的 OpenAPI 与前端正式 OpenAPI 不一致".into());
    }
    if source_bytes != candidate_bytes {
        return Err("来源提交中的 OpenAPI 与本次后端候选 OpenAPI 不一致".into());
    }
    let actual_digest = sha256_hex(&frontend_bytes);
    if source.sha256 != actual_digest {
        return Err(format!(
            "前端正式契约摘要不匹配：元数据 {}，实际 {actual_digest}",
            source.sha256
        )
        .into());
    }
    verify_openapi_version(&frontend_bytes, &source.openapi_version)?;
    Ok(source.backend_commit)
}

fn parse_source_metadata(path: &Path, expected_repository: &str) -> Result<ContractSource> {
    let bytes = load_bytes(path, "前端正式契约来源元数据")?;
    let document = serde_json::from_slice::<StrictJsonValue>(&bytes)
        .map_err(|error| format!("无法读取前端正式契约来源元数据 {}：{error}", path.display()))?;
    let object = document
        .0
        .as_object()
        .ok_or("前端正式契约来源元数据必须是对象")?;
    verify_source_keys(object)?;
    if object
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        return Err("前端正式契约来源 schema_version 必须为 1".into());
    }

    let repository = required_string(object, "backend_repository")?;
    if !valid_repository(repository) {
        return Err("backend_repository 必须是 owner/repository".into());
    }
    if repository != expected_repository {
        return Err(format!(
            "正式契约后端仓库不匹配：期望 {expected_repository}，实际 {repository}"
        )
        .into());
    }
    let backend_commit = required_string(object, "backend_commit")?;
    if !valid_lowercase_hex(backend_commit, 40) {
        return Err("backend_commit 必须是小写 40 位 Git SHA".into());
    }
    let openapi_path = required_string(object, "openapi_path")?;
    verify_openapi_path(openapi_path)?;
    let openapi_version = required_string(object, "openapi_version")?;
    if !openapi_version.starts_with("3.") {
        return Err("openapi_version 必须是 OpenAPI 3 版本".into());
    }
    let sha256 = required_string(object, "sha256")?;
    if !valid_lowercase_hex(sha256, 64) {
        return Err("sha256 必须是小写 64 位十六进制摘要".into());
    }
    Ok(ContractSource {
        backend_commit: backend_commit.to_owned(),
        openapi_path: openapi_path.to_owned(),
        openapi_version: openapi_version.to_owned(),
        sha256: sha256.to_owned(),
    })
}

fn verify_source_keys(object: &serde_json::Map<String, serde_json::Value>) -> Result<()> {
    let expected = SOURCE_KEYS.into_iter().collect::<BTreeSet<_>>();
    let actual = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if actual == expected {
        return Ok(());
    }
    let missing = expected.difference(&actual).copied().collect::<Vec<_>>();
    let extra = actual.difference(&expected).copied().collect::<Vec<_>>();
    Err(format!("前端正式契约来源元数据字段不匹配，缺少={missing:?}，多余={extra:?}").into())
}

fn required_string<'a>(
    object: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<&'a str> {
    object
        .get(key)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("{key} 必须是字符串").into())
}

fn verify_openapi_path(path: &str) -> Result<()> {
    let invalid_segment = path
        .split('/')
        .any(|segment| segment.is_empty() || matches!(segment, "." | ".."));
    if !path.ends_with(".json") || path.starts_with('/') || path.contains('\\') || invalid_segment {
        return Err("openapi_path 必须是不含路径穿越的相对 JSON 路径".into());
    }
    if path != EXPECTED_OPENAPI_PATH {
        return Err(format!("openapi_path 必须是 {EXPECTED_OPENAPI_PATH}，实际 {path}").into());
    }
    Ok(())
}

fn verify_openapi_version(openapi: &[u8], expected: &str) -> Result<()> {
    let document: serde_json::Value = serde_json::from_slice(openapi)
        .map_err(|error| format!("前端正式 OpenAPI 不是有效 UTF-8 JSON：{error}"))?;
    if document
        .as_object()
        .and_then(|object| object.get("openapi"))
        .and_then(serde_json::Value::as_str)
        != Some(expected)
    {
        return Err("前端正式 OpenAPI 版本与来源元数据不一致".into());
    }
    Ok(())
}

fn verified_commit(worktree: &Path, value: &str, label: &str) -> Result<String> {
    if !valid_lowercase_hex(value, 40) {
        return Err(format!("{label}必须是小写 40 位 Git SHA").into());
    }
    let object = format!("{value}^{{commit}}");
    let actual = git_text(worktree, &["rev-parse", "--verify", &object])?;
    let actual = actual.trim();
    if actual != value {
        return Err(format!("{label}未解析到指定提交：期望 {value}，实际 {actual}").into());
    }
    Ok(actual.to_owned())
}

fn verify_ancestor(worktree: &Path, source: &str, head: &str) -> Result<()> {
    let arguments = ["merge-base", "--is-ancestor", source, head];
    let output = execute_git(worktree, &arguments)?;
    match output.status.code() {
        Some(0) => Ok(()),
        Some(1) => Err("正式契约来源提交不是当前后端 HEAD 的祖先".into()),
        _ => Err(format!(
            "无法验证正式契约来源祖先关系：{}",
            git_failure_detail(&output)
        )
        .into()),
    }
}

fn git_text(worktree: &Path, arguments: &[&str]) -> Result<String> {
    let bytes = git_output(worktree, arguments)?;
    if !bytes.is_ascii() {
        return Err("Git 命令输出不是 ASCII".into());
    }
    String::from_utf8(bytes).map_err(Into::into)
}

fn git_output(worktree: &Path, arguments: &[&str]) -> Result<Vec<u8>> {
    let output = execute_git(worktree, arguments)?;
    if output.status.success() {
        return Ok(output.stdout);
    }
    Err(format!(
        "Git 命令失败（git {}）：{}",
        arguments.join(" "),
        git_failure_detail(&output)
    )
    .into())
}

fn execute_git(worktree: &Path, arguments: &[&str]) -> Result<Output> {
    child_command("git")
        .args(arguments)
        .current_dir(worktree)
        .output()
        .map_err(|error| {
            format!("无法执行 Git 命令（git {}）：{error}", arguments.join(" ")).into()
        })
}

fn git_failure_detail(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if stderr.is_empty() {
        output.status.to_string()
    } else {
        stderr
    }
}

fn load_bytes(path: &Path, label: &str) -> Result<Vec<u8>> {
    fs::read(path).map_err(|error| format!("无法读取{label} {}：{error}", path.display()).into())
}

fn valid_repository(value: &str) -> bool {
    let mut segments = value.split('/');
    let Some(owner) = segments.next() else {
        return false;
    };
    let Some(repository) = segments.next() else {
        return false;
    };
    segments.next().is_none()
        && valid_repository_segment(owner)
        && valid_repository_segment(repository)
}

fn valid_repository_segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn valid_lowercase_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}
