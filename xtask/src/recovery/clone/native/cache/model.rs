use super::super::{digest, json as document, resources::common};
use crate::Result;
use regex::Regex;
use serde_json::{Value, json};
use std::path::{Component, Path, PathBuf};

pub(super) const FIELDS: &[&str] = &[
    "scope_id",
    "wsl",
    "distribution",
    "launcher",
    "executable",
    "sha256",
    "configuration",
    "directory",
    "previous_identity",
    "previous_boot_id",
    "previous_run_id",
    "port",
    "password",
    "timeout_seconds",
    "python",
];

const WINDOWS_FIELDS: &[&str] = &[
    "format_version",
    "scope_id",
    "tool",
    "configuration",
    "directory",
    "previous_identity",
    "previous_run_id",
    "port",
    "password",
    "timeout_seconds",
];

pub(super) fn matches(value: &Value, pattern: &str) -> Result<bool> {
    Ok(value
        .as_str()
        .is_some_and(|text| Regex::new(pattern).expect("固定缓存规则").is_match(text)))
}

pub(super) fn password(value: &Value) -> Result<&str> {
    value
        .as_str()
        .filter(|text| !text.trim().is_empty() && !text.contains('\0'))
        .ok_or_else(|| "Redis password 必须是配置文件中的非空 UTF-8 文本，不含 NUL".into())
}

pub(in crate::recovery) fn render_configuration(
    directory_argument: &str,
    port: u16,
    password: &str,
) -> String {
    format!(
        "bind 127.0.0.1\nport {port}\ndir {directory_argument}\ndatabases 1\nprotected-mode yes\nsave \"\"\nappendonly no\ndaemonize no\nrequirepass {}\n",
        quoted(password)
    )
}

pub(in crate::recovery) fn quoted(value: &str) -> String {
    let mut result = Vec::from(b"\"".as_slice());
    for byte in value.bytes() {
        match byte {
            b'\\' | b'"' => {
                result.push(b'\\');
                result.push(byte);
            }
            0..=31 | 127 => result.extend_from_slice(format!("\\x{byte:02x}").as_bytes()),
            _ => result.push(byte),
        }
    }
    result.push(b'"');
    // 输入是 UTF-8；只替换 ASCII 字节，非 ASCII 编码保持不变。
    String::from_utf8(result).expect("Redis 配置转义保持 UTF-8")
}

pub(super) fn validate(backend: &Path, request: &Value, files: bool) -> Result<()> {
    shape(request)?;
    if files {
        common::directory_identity(backend, &request["directory"])?;
        file_bound(&request["configuration"])?;
        if request["format_version"] == 3 {
            return Ok(());
        }
        let wsl = plain(Path::new(document::text(
            &request["wsl"]["path"],
            "WSL 工具",
        )?))?;
        if digest::file(&wsl)?.sha256 != document::text(&request["wsl"]["sha256"], "WSL 摘要")? {
            return Err("WSL 实际工具变化".into());
        }
    }
    Ok(())
}

pub(super) fn fields(request: &Value) -> Vec<&'static str> {
    if request["format_version"] == 3 {
        WINDOWS_FIELDS.to_vec()
    } else if request["format_version"] == 2 {
        FIELDS
            .iter()
            .copied()
            .filter(|field| *field != "python")
            .chain(["format_version", "native_tool"])
            .collect()
    } else {
        FIELDS.to_vec()
    }
}

pub(super) fn shape(request: &Value) -> Result<()> {
    document::exact(request, &fields(request), "Redis 进程请求")?;
    if request["format_version"] == 3 {
        return windows_shape(request);
    }
    if request["format_version"] == 2 {
        super::native_tool::from_request(request)?;
    } else {
        // 已登记 v1 证据仍可只读核验；所有原生写入口另行强制 v2。
        document::exact(request, FIELDS, "Redis 历史进程请求")?;
        legacy_interpreter(request)?;
    }
    password(&request["password"])?;
    if !matches(&request["scope_id"], r"^[a-z0-9][a-z0-9_-]{2,47}$")?
        || !matches(&request["distribution"], r"^[A-Za-z0-9._-]+$")?
        || request["launcher"] != "/usr/bin/redis-server"
        || !matches(&request["executable"], r"^/usr/(?:local/)?bin/[a-z0-9-]+$")?
        || !valid_endpoint(request)
        || !matches(&request["sha256"], r"^[a-f0-9]{64}$")?
    {
        return Err("Redis 原进程、端点或工具声明无效".into());
    }
    predecessor(request)?;
    document::exact(
        &request["directory"],
        &["path", "device", "inode"],
        "Redis 目录",
    )?;
    document::exact(&request["wsl"], &["path", "sha256"], "WSL 工具")?;
    document::exact(
        &request["configuration"],
        &["path", "bytes", "sha256"],
        "Redis 配置",
    )?;
    let directory = linux_path(document::text(&request["directory"]["path"], "Redis 目录")?)?;
    let config = linux_path(document::text(
        &request["configuration"]["path"],
        "Redis 配置",
    )?)?;
    linux_path(document::text(&request["wsl"]["path"], "WSL 工具")?)?;
    if config != format!("{directory}/redis.conf") {
        return Err("Redis 仅使用原服务目录中的配置文件".into());
    }
    if !valid_storage_binding(request)? || !matches(&request["wsl"]["sha256"], r"^[a-f0-9]{64}$")? {
        return Err("缓存文件或目录绑定无效".into());
    }
    Ok(())
}

fn valid_endpoint(request: &Value) -> bool {
    request["port"]
        .as_u64()
        .is_some_and(|v| (1024..=65535).contains(&v))
        && request["timeout_seconds"]
            .as_u64()
            .is_some_and(|v| (1..=180).contains(&v))
}

fn valid_storage_binding(request: &Value) -> Result<bool> {
    Ok(request["directory"]["device"].as_u64().is_some()
        && request["directory"]["inode"].as_u64().is_some()
        && request["configuration"]["bytes"]
            .as_u64()
            .is_some_and(|n| n <= 16 * 1024 * 1024)
        && matches(&request["configuration"]["sha256"], r"^[a-f0-9]{64}$")?)
}

fn windows_shape(request: &Value) -> Result<()> {
    super::windows_tool::validate(&request["tool"])?;
    password(&request["password"])?;
    if !matches(&request["scope_id"], r"^[a-z0-9][a-z0-9_-]{2,47}$")? || !valid_endpoint(request) {
        return Err("Windows Redis 范围、端口或期限无效".into());
    }
    let previous = &request["previous_identity"];
    if previous.is_null() {
        if !request["previous_run_id"].is_null() {
            return Err("首代 Windows Redis 不得带前驱运行代次".into());
        }
    } else {
        identity(previous, false)?;
        let image = document::text(&previous["executable"], "前驱 Windows Redis 映像")?;
        let tool = document::text(&request["tool"]["path"], "Windows Redis 工具路径")?;
        if !cygwin_path(image)?.eq_ignore_ascii_case(&cygwin_path(tool)?)
            || !matches(&request["previous_run_id"], r"^[a-f0-9]{40}$")?
        {
            return Err("Windows Redis 前驱映像或运行代次不符".into());
        }
    }
    document::exact(
        &request["directory"],
        &["path", "device", "inode"],
        "Redis 目录",
    )?;
    document::exact(
        &request["configuration"],
        &["path", "bytes", "sha256"],
        "Redis 配置",
    )?;
    let directory = service_path(request, "directory", "Redis 目录")?;
    if configuration_file(request)? != format!("{directory}/redis.conf") {
        return Err("Redis 仅使用原服务目录中的配置文件".into());
    }
    if !valid_storage_binding(request)? {
        return Err("缓存文件或目录绑定无效".into());
    }
    Ok(())
}

fn legacy_interpreter(request: &Value) -> Result<()> {
    document::exact(
        &request["python"],
        &["path", "executable", "sha256"],
        "历史 Linux 解释器",
    )?;
    if request["python"]["path"] != "/usr/bin/python3"
        || !matches(
            &request["python"]["executable"],
            r"^/usr/bin/python3\.[0-9]+$",
        )?
        || !matches(&request["python"]["sha256"], r"^[a-f0-9]{64}$")?
    {
        return Err("历史解释器绑定不完整".into());
    }
    Ok(())
}

fn predecessor(request: &Value) -> Result<()> {
    let previous = &request["previous_identity"];
    if previous.is_null() {
        if !request["previous_boot_id"].is_null() || !request["previous_run_id"].is_null() {
            return Err("首代 Redis 不得带前驱启动或运行代次".into());
        }
    } else {
        identity(previous, false)?;
        if previous["executable"] != request["executable"]
            || !matches(&request["previous_run_id"], r"^[a-f0-9]{40}$")?
            || !boot(&request["previous_boot_id"])?
        {
            return Err("Redis 原始 Linux 创建身份不完整".into());
        }
    }
    Ok(())
}

pub(super) fn boot(value: &Value) -> Result<bool> {
    matches(value, r"^[a-f0-9]{8}(?:-[a-f0-9]{4}){3}-[a-f0-9]{12}$")
}

pub(super) fn identity(value: &Value, linux: bool) -> Result<()> {
    let fields: &[&str] = if linux {
        &["pid", "started", "executable", "boot_id"]
    } else {
        &["pid", "started", "executable"]
    };
    document::exact(value, fields, "缓存创建身份")?;
    if !value["pid"]
        .as_u64()
        .is_some_and(|v| v > 1 && u32::try_from(v).is_ok())
        || !matches(&value["started"], r"^[0-9]+$")?
        || value["executable"].as_str().is_none_or(|v| v.is_empty())
        || linux && !boot(&value["boot_id"])?
    {
        return Err("缓存创建身份字段不完整".into());
    }
    Ok(())
}

pub(super) fn plain(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute()
        || !path.exists()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
        || digest::linked_path(path)?.into_iter().any(|linked| linked)
    {
        return Err("缓存控制只接受无链接的已有绝对路径".into());
    }
    Ok(path.to_path_buf())
}

pub(super) fn file_bound(value: &Value) -> Result<PathBuf> {
    document::exact(value, &["path", "bytes", "sha256"], "缓存文件绑定")?;
    let path = plain(Path::new(document::text(&value["path"], "缓存文件")?))?;
    if common::binding(&path)? != *value {
        return Err("缓存控制文件摘要变化".into());
    }
    Ok(path)
}

pub(super) fn linux_path(value: &str) -> Result<String> {
    let normalized = value.replace('\\', "/");
    let bytes = normalized.as_bytes();
    if bytes.len() < 3
        || !bytes[0].is_ascii_alphabetic()
        || &bytes[1..3] != b":/"
        || normalized[3..].split('/').any(|part| {
            matches!(part, "" | "." | "..")
                || part.contains(':')
                || part.contains(['\0', '\r', '\n'])
        })
    {
        return Err("WSL 仅映射已登记 Windows 盘符绝对路径".into());
    }
    Ok(format!(
        "/mnt/{}/{}",
        char::from(bytes[0]).to_ascii_lowercase(),
        &normalized[3..]
    ))
}

pub(in crate::recovery) fn cygwin_path(value: &str) -> Result<String> {
    const INVALID: &str = "Windows Redis 路径必须是无点跳转的盘符绝对路径";
    let (drive, rest) = value.split_at_checked(3).ok_or(INVALID)?;
    let drive = drive.as_bytes();
    if !drive[0].is_ascii_alphabetic()
        || drive[1] != b':'
        || !matches!(drive[2], b'/' | b'\\')
        || rest.split(['/', '\\']).any(|part| {
            matches!(part, "" | "." | "..")
                || part.ends_with([' ', '.'])
                || part.contains([':', '<', '>', '"', '|', '?', '*'])
                || part.chars().any(char::is_control)
        })
    {
        return Err(INVALID.into());
    }
    Ok(format!(
        "/cygdrive/{}/{}",
        char::from(drive[0]).to_ascii_lowercase(),
        rest.replace('\\', "/")
    ))
}

fn service_path(request: &Value, field: &str, label: &str) -> Result<String> {
    let path = document::text(&request[field]["path"], label)?;
    if request["format_version"] == 3 {
        cygwin_path(path)
    } else {
        linux_path(path)
    }
}

pub(super) fn configuration_file(request: &Value) -> Result<String> {
    service_path(request, "configuration", "Redis 配置")
}

pub(super) fn configuration(request: &Value) -> Result<Value> {
    Ok(
        json!({"bind":"127.0.0.1", "port":request["port"].as_u64().ok_or("Redis 端口无效")?.to_string(),
        "dir":service_path(request, "directory", "Redis 目录")?,
        "databases":"1", "protected-mode":"yes", "save":"", "appendonly":"no", "daemonize":"no"}),
    )
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(in super::super) fn windows_request() -> Value {
        json!({
            "format_version":3,"scope_id":"cache-test",
            "tool":{"platform":"windows","path":"D:/工具 包/redis-server.exe","sha256":"a".repeat(64),
                "version":"Redis server v=7.4.11 sha=00000000:0 malloc=libc bits=64 build=5bd2587910691cb8",
                "dlls":{"cygwin1.dll":"b".repeat(64)}},
            "configuration":{"path":r"D:\缓存 数据\redis.conf","bytes":5,"sha256":"c".repeat(64)},
            "directory":{"path":"D:/缓存 数据","device":1,"inode":2},
            "previous_identity":{"pid":36784,"started":"134000000000000000",
                "executable":r"D:\工具 包\redis-server.exe"},
            "previous_run_id":"a".repeat(40),"port":16390,"password":"test-auth","timeout_seconds":1
        })
    }

    #[test]
    fn windows_request_rejects_mixed_fields_and_unbound_predecessor() {
        let request = windows_request();
        shape(&request).unwrap();
        for field in [
            "wsl",
            "distribution",
            "native_tool",
            "python",
            "previous_boot_id",
        ] {
            let mut mixed = request.clone();
            mixed[field] = Value::Null;
            assert!(shape(&mixed).is_err(), "{field}");
        }
        let mut invalid = request.clone();
        invalid["previous_identity"]["executable"] = json!(r"D:\另一目录\redis-server.exe");
        assert!(shape(&invalid).is_err());
        invalid = request.clone();
        invalid["previous_identity"]["boot_id"] = Value::Null;
        assert!(shape(&invalid).is_err());
        let mut initial = request.clone();
        initial["previous_run_id"] = Value::Null;
        assert!(shape(&initial).is_err());
        initial["previous_identity"] = Value::Null;
        shape(&initial).unwrap();
        initial["previous_run_id"] = request["previous_run_id"].clone();
        assert!(shape(&initial).is_err());
    }

    #[test]
    fn windows_request_projects_exact_cygwin_configuration_paths() {
        let request = windows_request();
        assert_eq!(
            configuration(&request).unwrap()["dir"],
            "/cygdrive/d/缓存 数据"
        );
        assert_eq!(
            configuration_file(&request).unwrap(),
            "/cygdrive/d/缓存 数据/redis.conf"
        );
        for path in [
            "D:/另一个目录/redis.conf",
            "D:/缓存 数据/./redis.conf",
            "D:/缓存 数据/../缓存 数据/redis.conf",
            "D:/缓存 数据/.. /redis.conf",
            "D:/缓存\t数据/redis.conf",
            "D:缓存 数据/redis.conf",
            "/cygdrive/d/缓存 数据/redis.conf",
        ] {
            let mut invalid = request.clone();
            invalid["configuration"]["path"] = json!(path);
            assert!(shape(&invalid).is_err(), "{path:?}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_request_files_require_original_directory_and_config_bytes() {
        let root = crate::workspace::root_dir();
        let base = root.join(".local-tests");
        std::fs::create_dir_all(&base).unwrap();
        let directory = tempfile::tempdir_in(base).unwrap();
        let path = directory.path();
        let configuration = path.join("redis.conf");
        std::fs::write(&configuration, b"before").unwrap();
        let (device, inode) =
            common::device_inode_for(path, &std::fs::metadata(path).unwrap()).unwrap();
        let mut request = windows_request();
        request["directory"] = json!({"path":path,"device":device,"inode":inode});
        request["configuration"] = common::binding(&configuration).unwrap();
        validate(&root, &request, true).unwrap();
        let mut wrong = request.clone();
        wrong["directory"]["inode"] = json!(inode ^ 1);
        assert!(validate(&root, &wrong, true).is_err());
        std::fs::write(configuration, b"drift!").unwrap();
        assert!(validate(&root, &request, true).is_err());
    }
}
