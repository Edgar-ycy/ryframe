use super::{
    Input, execute,
    failure::{Failure, NativeResult},
    process::Cancellation,
};
use std::{
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
    process::{Command, Stdio},
};

pub(crate) const MARKER: &str = "__devex-runtime";

/// 私有原生驱动仍由既有 Job/cgroup 包裹，保留完整命令树内存证据。
pub(crate) fn command(
    arguments: &[String],
    environment: &BTreeMap<String, String>,
) -> NativeResult<Command> {
    let input = parse(arguments, environment)?;
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
    scrub_environment(&mut command, environment);
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

fn scrub_environment(command: &mut Command, environment: &BTreeMap<String, String>) {
    for (key, _) in std::env::vars_os() {
        if private_environment(&key.to_string_lossy()) {
            command.env_remove(key);
        }
    }
    for key in environment.keys() {
        if private_environment(key) {
            command.env_remove(key);
        }
    }
}

pub(crate) fn parse(
    arguments: &[String],
    environment: &BTreeMap<String, String>,
) -> NativeResult<Input> {
    if arguments.len() != 10 {
        return Err(Failure::new("runtime_parameters"));
    }
    let mut fields = BTreeMap::new();
    for pair in arguments.as_chunks::<2>().0 {
        if !matches!(
            pair[0].as_str(),
            "--suite" | "--backend" | "--frontend" | "--runner-frontend" | "--output"
        ) || fields.insert(pair[0].as_str(), pair[1].as_str()).is_some()
        {
            return Err(Failure::new("runtime_parameters"));
        }
    }
    let parameter = |name| {
        fields
            .get(name)
            .copied()
            .ok_or_else(|| Failure::new("runtime_parameters"))
    };
    let binding = |name| {
        environment
            .get(name)
            .cloned()
            .ok_or_else(|| Failure::new("runtime_parameters"))
    };
    let suite = parameter("--suite")?.to_owned();
    let input = Input {
        backend: PathBuf::from(parameter("--backend")?),
        frontend: PathBuf::from(parameter("--frontend")?),
        runner_frontend: PathBuf::from(parameter("--runner-frontend")?),
        driver: crate::workspace::root_dir(),
        output: PathBuf::from(parameter("--output")?),
        concurrency: if suite == "homepage" {
            0
        } else {
            binding("RYFRAME_DEVEX_CONCURRENCY")?
                .parse()
                .map_err(|_| Failure::new("runtime_parameters"))?
        },
        suite,
        cache: binding("RYFRAME_DEVEX_CACHE")?,
        input_sha256: binding("RYFRAME_DEVEX_RUNTIME_INPUT_SHA256")?,
        driver_fingerprint: binding("RYFRAME_DEVEX_DRIVER_FINGERPRINT")?,
        runner_frontend_fingerprint: binding("RYFRAME_DEVEX_RUNNER_FRONTEND_FINGERPRINT")?,
        source_fingerprints: serde_json::from_str(&binding("RYFRAME_DEVEX_SOURCE_FINGERPRINTS")?)
            .map_err(|_| Failure::new("runtime_parameters"))?,
        app_env: binding("APP_ENV")?,
    };
    input.validate()?;
    Ok(input)
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
