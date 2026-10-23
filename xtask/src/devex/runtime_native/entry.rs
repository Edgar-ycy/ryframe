use super::{
    Input, execute,
    failure::{Failure, NativeResult},
    process::Cancellation,
};
use std::{
    io::{Read, Seek, SeekFrom, Write},
    process::{Command, Stdio},
};

pub(crate) const MARKER: &str = "__devex-runtime";

/// 私有原生驱动仍由既有 Job/cgroup 包裹，保留完整命令树内存证据。
pub(crate) fn command(input: &Input) -> NativeResult<Command> {
    input.validate()?;
    let mut file = tempfile::tempfile().map_err(|_| Failure::new("runtime_protocol_input"))?;
    let bytes = serde_json::to_vec(&input).map_err(|_| Failure::new("runtime_protocol_input"))?;
    if bytes.len() > 65536 {
        return Err(Failure::new("runtime_protocol_input_limit"));
    }
    file.write_all(&bytes)
        .and_then(|()| file.seek(SeekFrom::Start(0)).map(|_| ()))
        .map_err(|_| Failure::new("runtime_protocol_input"))?;
    let mut command =
        Command::new(std::env::current_exe().map_err(|_| Failure::new("native_tool_binding"))?);
    command.arg(MARKER).stdin(Stdio::from(file));
    scrub_environment(&mut command);
    command.env("APP_ENV", &input.app_env);
    Ok(command)
}

pub(crate) fn private_environment(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    name.starts_with("RYFRAME_")
        || name.starts_with("VITE_")
        || name.starts_with("SNOWFLAKE_")
        || (name.starts_with("APP_") && name != "APP_ENV")
}

pub(crate) fn allowed_environment(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    name.starts_with("CARGO_")
        || name.starts_with("RUSTUP_")
        || matches!(
            name.as_str(),
            "PATH"
                | "PATHEXT"
                | "SYSTEMROOT"
                | "WINDIR"
                | "SYSTEMDRIVE"
                | "COMSPEC"
                | "TEMP"
                | "TMP"
                | "TMPDIR"
                | "HOME"
                | "USERPROFILE"
                | "HOMEDRIVE"
                | "HOMEPATH"
                | "LOCALAPPDATA"
                | "APPDATA"
                | "PROGRAMDATA"
                | "PROGRAMFILES"
                | "PROGRAMFILES(X86)"
                | "NUMBER_OF_PROCESSORS"
                | "LD_LIBRARY_PATH"
                | "DYLD_LIBRARY_PATH"
                | "LANG"
                | "LC_ALL"
                | "TZ"
                | "RUSTFLAGS"
                | "CARGO_ENCODED_RUSTFLAGS"
                | "RUST_BACKTRACE"
                | "RUST_LIB_BACKTRACE"
                | "CC"
                | "CXX"
                | "CFLAGS"
                | "CXXFLAGS"
                | "INCLUDE"
                | "LIB"
                | "LIBPATH"
                | "NO_COLOR"
                | "FORCE_COLOR"
                | "TERM"
        )
}

fn scrub_environment(command: &mut Command) {
    command.env_clear();
    for (key, value) in std::env::vars_os() {
        if allowed_environment(&key.to_string_lossy()) {
            command.env(key, value);
        }
    }
}

pub(crate) fn run_hidden_if_requested() -> Option<i32> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.first().is_none_or(|value| value != MARKER) {
        return None;
    }
    let result = (|| {
        if args.len() != 1 {
            return Err(Failure::new("runtime_parameters"));
        }
        let mut bytes = Vec::new();
        std::io::stdin()
            .take(65537)
            .read_to_end(&mut bytes)
            .map_err(|_| Failure::new("runtime_protocol_input"))?;
        if bytes.len() > 65536 {
            return Err(Failure::new("runtime_protocol_input_limit"));
        }
        let input: Input =
            serde_json::from_slice(&bytes).map_err(|_| Failure::new("runtime_protocol_input"))?;
        input.validate()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| Failure::new("runtime_initialization"))?;
        runtime.block_on(async {
            let cancel = Cancellation::default();
            let trigger = cancel.clone();
            let signal = tokio::spawn(async move {
                if tokio::signal::ctrl_c().await.is_ok() {
                    trigger.cancel();
                }
            });
            let result = execute(&input, &cancel).await;
            signal.abort();
            result
        })
    })();
    Some(match result {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error}");
            if matches!(
                error.category,
                "runtime_parameters" | "runtime_protocol_input" | "runtime_protocol_input_limit"
            ) {
                2
            } else {
                1
            }
        }
    })
}

pub(crate) fn is_native(program: &str) -> bool {
    program == MARKER
}
