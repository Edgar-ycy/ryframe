use super::{Checked, model};
use serde_json::Value;
use std::{io::Write, path::Path, process::Command};

const CONTAINER_CLIENT: &str = "umask 077\nconfig=$(mktemp) || exit 1\ntrap 'rm -f -- \"$config\"' EXIT\ntrap 'exit 130' HUP INT TERM\nIFS= read -r header && IFS= read -r credential || exit 1\nprintf '%s\\n' \"$header\" \"$credential\" > \"$config\" || exit 1\nclient=$1\nshift\n\"$client\" --defaults-file=\"$config\" \"$@\"\n";

pub(super) enum Client<'a> {
    Container(&'a str),
    #[cfg(test)]
    Local(&'a Path),
}

pub(super) struct Invocation {
    pub command: Command,
    pub prefix: Vec<u8>,
    // 必须比客户端进程活得更久，结束或取消时自动删除。
    pub _options: tempfile::NamedTempFile,
}

pub(super) fn invocation(
    binding: &Value,
    client: Client<'_>,
    directory: &Path,
) -> Checked<Invocation> {
    model::validate_binding(binding)?;
    let password = binding["password"].as_str().ok_or("binding_invalid")?;
    let content = options(password);
    let mut file = tempfile::NamedTempFile::new_in(directory).map_err(|_| "client_options")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|_| "client_options")?;
    }
    file.write_all(content.as_bytes())
        .and_then(|()| file.as_file().sync_all())
        .map_err(|_| "client_options")?;
    let (mut command, port, prefix) = match client {
        Client::Container(id) => {
            if !model::container(id) {
                return Err("ci_context");
            }
            let mut command = Command::new("docker");
            command.args([
                "exec",
                "-i",
                id,
                "sh",
                "-c",
                CONTAINER_CLIENT,
                "ryframe-mysql",
                "mysql",
            ]);
            (command, 3306, content.into_bytes())
        }
        #[cfg(test)]
        Client::Local(path) => {
            use crate::local_test_path::{LocalTestPathKind, validate_absolute_path};
            validate_absolute_path(path, LocalTestPathKind::ExistingFile)
                .map_err(|_| "client_path")?;
            let mut command = Command::new(path);
            command.arg(format!("--defaults-file={}", file.path().display()));
            (
                command,
                binding["port"].as_u64().ok_or("binding_invalid")?,
                Vec::new(),
            )
        }
    };
    append_flags(&mut command, binding, port)?;
    environment(&mut command);
    Ok(Invocation {
        command,
        prefix,
        _options: file,
    })
}

fn append_flags(command: &mut Command, binding: &Value, port: u64) -> Checked<()> {
    command.args([
        format!(
            "--host={}",
            binding["host"].as_str().ok_or("binding_invalid")?
        ),
        format!("--port={port}"),
        format!(
            "--user={}",
            binding["username"].as_str().ok_or("binding_invalid")?
        ),
        "--protocol=TCP".into(),
        "--default-character-set=utf8mb4".into(),
        format!(
            "--ssl-mode={}",
            binding["tls_mode"]
                .as_str()
                .ok_or("binding_invalid")?
                .to_ascii_uppercase()
        ),
        "--skip-reconnect".into(),
        "--connect-timeout=5".into(),
    ]);
    for (field, flag) in [
        ("tls_ca", "--ssl-ca"),
        ("tls_client_cert", "--ssl-cert"),
        ("tls_client_key", "--ssl-key"),
    ] {
        if let Some(value) = binding[field].as_str().filter(|v| !v.is_empty()) {
            command.arg(format!("{flag}={value}"));
        }
    }
    Ok(())
}

pub(super) fn options(password: &str) -> String {
    let escaped = password
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
        .replace('\u{8}', "\\b");
    format!("[client]\npassword=\"{escaped}\"\n")
}

pub(super) fn environment(command: &mut Command) {
    command.env_clear();
    for (key, value) in std::env::vars_os() {
        if matches!(
            key.to_string_lossy().to_ascii_uppercase().as_str(),
            "PATH"
                | "PATHEXT"
                | "SYSTEMROOT"
                | "WINDIR"
                | "COMSPEC"
                | "HOME"
                | "USERPROFILE"
                | "HOMEDRIVE"
                | "HOMEPATH"
                | "TEMP"
                | "TMP"
                | "DOCKER_HOST"
                | "DOCKER_CONFIG"
                | "DOCKER_TLS_VERIFY"
                | "DOCKER_CERT_PATH"
        ) {
            command.env(key, value);
        }
    }
}

pub(super) fn input_file(directory: &Path, bytes: &[u8]) -> Checked<tempfile::NamedTempFile> {
    // 输入也可能包含密码；与选项文件共享已收紧权限的目录。
    let mut file = tempfile::NamedTempFile::new_in(directory).map_err(|_| "client_input")?;
    file.write_all(bytes)
        .and_then(|()| file.as_file().sync_all())
        .map_err(|_| "client_input")?;
    Ok(file)
}
