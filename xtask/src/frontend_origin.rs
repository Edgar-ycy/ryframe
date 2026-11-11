//! 前端 origin 的只读原生校验与编译来源描述入口。

use std::{
    env,
    fs::File,
    io::{Read, Write},
};

use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use url::Url;

#[path = "frontend_origin/manifest.rs"]
pub(crate) mod manifest;
#[path = "frontend_origin/source_guard.rs"]
mod source_guard;
#[path = "frontend_origin/sources.rs"]
pub(crate) mod sources;

const INPUT_LIMIT: usize = 64 * 1024 * 1024;
const INVALID_ARGUMENTS: &str = "参数无效：仅接受 __frontend-origin check、describe、verify-sources 或 prepare --write，请核对操作及明确写入授权。";
const INVALID_REQUEST: &str = "请求格式无效：请通过标准输入提供唯一的 JSON 对象，仅包含 format_version、value 和 allow_empty，核对字段类型并移除重复或未知字段。";
const INVALID_VERSION: &str = "请求版本无效：请使用 format_version 为整数 1 的请求。";
const INPUT_TOO_LARGE: &str = "请求过大：标准输入不得超过 64 MiB，请缩小请求后重试。";
const INPUT_UNREADABLE: &str = "请求读取失败：请检查标准输入管道并重新提供完整 JSON 请求。";
const EMPTY_ORIGIN: &str = "地址为空：请提供 HTTP(S) origin，或显式允许空值。";
const INVALID_URL: &str = "地址格式无效：请提供有效的绝对 HTTP(S) origin 并核对主机与端口。";
const INVALID_SCHEME: &str = "地址协议无效：请使用具有非空 origin 的 HTTP 或 HTTPS 地址。";
const INVALID_USERINFO: &str = "地址包含身份信息：请移除非空用户名和密码。";
const INVALID_COMPONENTS: &str = "地址包含额外内容：请移除根路径以外的路径及非空查询和片段。";
const EXECUTABLE_PATH: &str =
    "可执行文件定位失败（current_exe）：请从可访问的绝对普通文件启动同一版本工具。";
const EXECUTABLE_READ: &str =
    "可执行文件摘要读取失败（current_exe）：请核对工具文件读取权限与文件完整性后重试。";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    format_version: u32,
    value: String,
    allow_empty: bool,
}

pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    let mut arguments = env::args_os().skip(1);
    if arguments.next().as_deref() != Some(std::ffi::OsStr::new("__frontend-origin")) {
        return None;
    }
    let operation = arguments.next();
    let option = arguments.next();
    let operation = operation.as_deref().and_then(std::ffi::OsStr::to_str);
    let valid = arguments.next().is_none()
        && if operation == Some("prepare") {
            option.as_deref() == Some(std::ffi::OsStr::new("--write"))
        } else {
            option.is_none()
        };
    let (response, code) = if !valid {
        (failure(INVALID_ARGUMENTS), 2)
    } else {
        match operation {
            Some("check") => run_check(),
            Some("verify-sources") => source_guard::run(),
            Some("prepare") => prepare(),
            Some("describe") => match describe() {
                Ok(value) => (value, 0),
                Err(reason) => {
                    let mut value = failure(reason);
                    value["kind"] = json!("frontend-origin-tool");
                    (value, 2)
                }
            },
            _ => (failure(INVALID_ARGUMENTS), 2),
        }
    };
    Some(if emit(&response).is_ok() { code } else { 2 })
}

fn prepare() -> (Value, i32) {
    let root = crate::workspace::root_dir();
    let result = crate::check::consumer_context::prepare_python_origin(&root)
        .and_then(|binding| binding.finish(Ok(())));
    match result {
        Ok(()) => (
            json!({"format_version":1,"kind":"frontend-origin-ready"}),
            0,
        ),
        Err(error) => (
            json!({"format_version":1,"valid":false,"error":format!(
                "来源绑定准备失败：{error}；请核对当前工具与完整源码，保留 python-origin-runs 中的失败证据；不覆盖历史或自动重放。"
            )}),
            2,
        ),
    }
}

fn run_check() -> (Value, i32) {
    let mut input = Vec::new();
    if std::io::stdin()
        .lock()
        .take((INPUT_LIMIT + 1) as u64)
        .read_to_end(&mut input)
        .is_err()
    {
        return (failure(INPUT_UNREADABLE), 2);
    }
    if input.len() > INPUT_LIMIT {
        return (failure(INPUT_TOO_LARGE), 2);
    }
    // 派生的结构反序列化也支持数组；协议只接受对象。
    if input
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace())
        != Some(b'{')
    {
        return (failure(INVALID_REQUEST), 2);
    }
    let request = match serde_json::from_slice::<Request>(&input) {
        Ok(request) => request,
        Err(_) => return (failure(INVALID_REQUEST), 2),
    };
    if request.format_version != 1 {
        return (failure(INVALID_VERSION), 2);
    }
    match normalize_origin(&request.value, request.allow_empty) {
        Ok(origin) => (
            json!({"format_version": 1, "valid": true, "origin": origin}),
            0,
        ),
        Err(reason) => (failure(reason), 1),
    }
}

pub(crate) fn normalize_origin(value: &str, allow_empty: bool) -> Result<String, &'static str> {
    // JS trim 不移除 U+0085，但会移除 U+FEFF。
    let value = value.trim_matches(|character: char| {
        (character.is_whitespace() && character != '\u{85}') || character == '\u{feff}'
    });
    if value.is_empty() {
        return if allow_empty {
            Ok(String::new())
        } else {
            Err(EMPTY_ORIGIN)
        };
    }
    let url = Url::parse(value).map_err(|_| INVALID_URL)?;
    let origin = url.origin().ascii_serialization();
    if !matches!(url.scheme(), "http" | "https") || origin == "null" || origin.is_empty() {
        return Err(INVALID_SCHEME);
    }
    if !url.username().is_empty() || url.password().is_some_and(|password| !password.is_empty()) {
        return Err(INVALID_USERINFO);
    }
    if url.path() != "/"
        || url.query().is_some_and(|query| !query.is_empty())
        || url.fragment().is_some_and(|fragment| !fragment.is_empty())
    {
        return Err(INVALID_COMPONENTS);
    }
    Ok(origin)
}

pub(crate) fn verify_compiled_sources(
    root: &std::path::Path,
    files: &Value,
) -> Result<Value, &'static str> {
    let bytes = serde_json::to_vec(&json!({"format_version":1,"root":root,"sources":files}))
        .map_err(|_| EXECUTABLE_READ)?;
    let token =
        source_guard::execute(&mut std::io::Cursor::new(bytes)).map_err(|_| EXECUTABLE_READ)?;
    serde_json::to_value(token).map_err(|_| EXECUTABLE_READ)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompiledSources {
    // 根身份必须参与编译输入，即使两棵树的文件字节完全相同。
    root: std::path::PathBuf,
    files: Vec<Value>,
}

/// 只读取当前程序嵌入的真实编译根，不从运行目录推导工具来源。
pub(crate) fn compiled_root() -> Result<std::path::PathBuf, &'static str> {
    let snapshot: CompiledSources = serde_json::from_str(include_str!(concat!(
        env!("OUT_DIR"),
        "/frontend-origin-sources.json"
    )))
    .map_err(|_| EXECUTABLE_READ)?;
    Ok(snapshot.root)
}

pub(crate) fn describe() -> Result<Value, &'static str> {
    let path = env::current_exe()
        .and_then(std::fs::canonicalize)
        .map_err(|_| EXECUTABLE_PATH)?;
    if !path.is_absolute() {
        return Err(EXECUTABLE_PATH);
    }
    let mut executable = File::open(&path).map_err(|_| EXECUTABLE_READ)?;
    if !executable
        .metadata()
        .map_err(|_| EXECUTABLE_READ)?
        .is_file()
    {
        return Err(EXECUTABLE_PATH);
    }
    #[cfg(windows)]
    let description_path = windows_description_path(&path)?;
    #[cfg(not(windows))]
    let description_path = &path;
    let executable_hash = hash_executable(&mut executable)?;
    let path = description_path.to_str().ok_or(EXECUTABLE_PATH)?;
    let snapshot: CompiledSources = serde_json::from_str(include_str!(concat!(
        env!("OUT_DIR"),
        "/frontend-origin-sources.json"
    )))
    .map_err(|_| EXECUTABLE_READ)?;
    Ok(json!({
        "format_version": 1,
        "kind": "frontend-origin-tool",
        "executable": {"path": path, "sha256": executable_hash},
        "sources": snapshot.files,
    }))
}

#[cfg(windows)]
fn windows_description_path(
    canonical: &std::path::Path,
) -> Result<std::path::PathBuf, &'static str> {
    use std::path::{Component, PathBuf, Prefix};

    let mut components = canonical.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return Ok(canonical.to_path_buf());
    };
    let Prefix::VerbatimDisk(drive) = prefix.kind() else {
        return Ok(canonical.to_path_buf());
    };
    // 只转换本机盘符前缀；其余组件与原规范路径保持一致。
    let mut ordinary = PathBuf::from(format!("{}:", char::from(drive)));
    for component in components {
        ordinary.push(component.as_os_str());
    }
    if !ordinary.is_absolute()
        || std::fs::canonicalize(&ordinary).map_err(|_| EXECUTABLE_PATH)? != canonical
    {
        return Err(EXECUTABLE_PATH);
    }
    Ok(ordinary)
}

fn hash_executable(file: &mut File) -> Result<String, &'static str> {
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|_| EXECUTABLE_READ)?;
        if count == 0 {
            return Ok(hex::encode(hasher.finalize()));
        }
        hasher.update(&buffer[..count]);
    }
}

fn failure(error: &'static str) -> Value {
    json!({"format_version": 1, "valid": false, "error": error})
}

fn emit(value: &Value) -> std::io::Result<()> {
    emit_to(&mut std::io::stdout().lock(), value)
}

fn emit_to(output: &mut impl Write, value: &Value) -> std::io::Result<()> {
    let encoded = serde_json::to_vec(value)?;
    output.write_all(&encoded)?;
    output.write_all(b"\n")?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_url_normalization_without_narrowing_frontend_semantics() {
        let cases = [
            ("http://example.invalid", "http://example.invalid"),
            ("HTTPS://EXAMPLE.INVALID:443/", "https://example.invalid"),
            ("https://例子.测试", "https://xn--fsqu00a.xn--0zwm56d"),
            ("http://127.0.0.1:8080/", "http://127.0.0.1:8080"),
            ("http://127.1", "http://127.0.0.1"),
            ("http://0x7f000001", "http://127.0.0.1"),
            ("http://0177.0.0.1", "http://127.0.0.1"),
            ("http://2130706433", "http://127.0.0.1"),
            ("http://[2001:db8::1]:8080/", "http://[2001:db8::1]:8080"),
            ("http://example.invalid:0/", "http://example.invalid:0"),
            ("http://example.invalid/?", "http://example.invalid"),
            ("http://example.invalid/#", "http://example.invalid"),
            ("http://example.invalid/?#", "http://example.invalid"),
            ("http://@example.invalid/", "http://example.invalid"),
            ("http://:@example.invalid/", "http://example.invalid"),
            (r"http:\\example.invalid\", "http://example.invalid"),
            ("http://example.invalid/a/..", "http://example.invalid"),
            ("http://exa\tmple.invalid/", "http://example.invalid"),
        ];
        for (input, expected) in cases {
            assert_eq!(normalize_origin(input, false), Ok(expected.to_owned()));
        }
    }

    #[test]
    fn trims_ecmascript_whitespace_and_respects_empty_policy() {
        let whitespace = "\u{9}\u{a}\u{b}\u{c}\u{d}\u{20}\u{a0}\u{1680}\u{2000}\u{2001}\u{2002}\u{2003}\u{2004}\u{2005}\u{2006}\u{2007}\u{2008}\u{2009}\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}";
        let padded = format!("{whitespace}https://example.invalid/{whitespace}");
        assert_eq!(
            normalize_origin(&padded, false),
            Ok("https://example.invalid".into())
        );
        for input in ["", whitespace] {
            assert_eq!(normalize_origin(input, true), Ok(String::new()));
            assert_eq!(normalize_origin(input, false), Err(EMPTY_ORIGIN));
        }
        for input in [
            "\u{85}",
            "\u{85}http://example.invalid",
            "http://example.invalid/\u{85}",
        ] {
            assert!(normalize_origin(input, true).is_err());
        }
    }

    #[test]
    fn rejects_invalid_origin_components_with_fixed_safe_errors() {
        let cases = [
            ("ftp://example.invalid/", INVALID_SCHEME),
            ("data:text/plain,secret", INVALID_SCHEME),
            ("http://user@example.invalid/", INVALID_USERINFO),
            ("http://:password@example.invalid/", INVALID_USERINFO),
            ("http://user:@example.invalid/", INVALID_USERINFO),
            ("http://example.invalid/path", INVALID_COMPONENTS),
            ("http://example.invalid/?query", INVALID_COMPONENTS),
            ("http://example.invalid/#fragment", INVALID_COMPONENTS),
            ("http://example.invalid:65536/", INVALID_URL),
            ("http://example.invalid:port/", INVALID_URL),
            ("http://999.999.999.999", INVALID_URL),
            ("http://256.0.0.1", INVALID_URL),
            ("http://[::1", INVALID_URL),
            ("example.invalid", INVALID_URL),
        ];
        for (input, expected) in cases {
            assert_eq!(normalize_origin(input, false), Err(expected));
        }
    }
}
