use std::path::{Path, PathBuf};

use crate::{
    local_test_path::{LocalTestPathKind, validate_local_test_path},
    workspace::root_dir,
};

use super::super::super::model::{
    CliError, FIXTURE_RUNTIME_USAGE, FixtureBrowserBinding, FixtureRuntimeCommand,
    FixtureRuntimePaths, FixtureServer,
};

#[derive(Default)]
struct Options {
    environment: Option<PathBuf>,
    output: Option<PathBuf>,
    browser_binding: Option<PathBuf>,
    run_id: Option<String>,
    server: Option<FixtureServer>,
    write: bool,
}

pub(super) fn parse_fixture_runtime(args: &[String]) -> Result<FixtureRuntimeCommand, CliError> {
    let Some((operation, values)) = args.split_first() else {
        return Err(CliError::new(FIXTURE_RUNTIME_USAGE));
    };
    if matches!(operation.as_str(), "--help" | "-h") && values.is_empty() {
        return Ok(FixtureRuntimeCommand::Help);
    }
    if ![
        "build",
        "verify",
        "start",
        "stop",
        "status",
        "bind",
        "browser",
        "browser-verify",
        "browser-close",
    ]
    .contains(&operation.as_str())
    {
        return Err(CliError::new(format!(
            "未知 recovery fixture runtime 子操作：{operation}"
        )));
    }
    let options = parse_options(values)?;
    validate_write(operation, options.write)?;
    build_command(operation, options)
}

fn parse_options(args: &[String]) -> Result<Options, CliError> {
    let mut options = Options::default();
    let mut index = 0;
    while index < args.len() {
        let name = args[index].as_str();
        if name == "--write" {
            if options.write {
                return Err(CliError::new("--write 不能重复"));
            }
            options.write = true;
            index += 1;
            continue;
        }
        let value = args
            .get(index + 1)
            .filter(|value| valid_value(value))
            .ok_or_else(|| CliError::new(format!("{name} 缺少有效取值")))?;
        match name {
            "--environment" => set_path(&mut options.environment, name, value)?,
            "--output" => set_path(&mut options.output, name, value)?,
            "--browser-binding" => set_path(&mut options.browser_binding, name, value)?,
            "--run-id" => set_string(&mut options.run_id, name, value)?,
            "--server" => set_server(&mut options.server, name, value)?,
            _ => return Err(CliError::new(format!("未知 fixture runtime 参数：{name}"))),
        }
        index += 2;
    }
    Ok(options)
}

fn build_command(operation: &str, options: Options) -> Result<FixtureRuntimeCommand, CliError> {
    let paths = runtime_paths(operation, &options)?;
    match operation {
        "build" => require_plain(options, FixtureRuntimeCommand::Build(paths)),
        "verify" => require_plain(options, FixtureRuntimeCommand::Verify(paths)),
        "start" => require_plain(options, FixtureRuntimeCommand::Start(paths)),
        "stop" => require_plain(options, FixtureRuntimeCommand::Stop(paths)),
        "status" => require_plain(options, FixtureRuntimeCommand::Status(paths)),
        "bind" => bind_command(options, paths),
        "browser" => browser_command(options, paths, "browser"),
        "browser-verify" => browser_command(options, paths, "browser-verify"),
        "browser-close" => browser_command(options, paths, "browser-close"),
        _ => unreachable!("operation 已由调用方核验"),
    }
}

fn runtime_paths(operation: &str, options: &Options) -> Result<FixtureRuntimePaths, CliError> {
    let environment = options
        .environment
        .clone()
        .ok_or_else(|| CliError::new("fixture runtime 缺少必需参数 --environment"))?;
    let output = options
        .output
        .clone()
        .ok_or_else(|| CliError::new("fixture runtime 缺少必需参数 --output"))?;
    let root = root_dir();
    validate(
        &environment,
        &root,
        LocalTestPathKind::ExistingFile,
        "环境收据",
    )?;
    let output_kind = if operation == "build" {
        LocalTestPathKind::NewDirectory
    } else {
        LocalTestPathKind::ExistingDirectory
    };
    validate(&output, &root, output_kind, "运行目录")?;
    Ok(FixtureRuntimePaths {
        environment,
        output,
    })
}

fn require_plain(
    options: Options,
    command: FixtureRuntimeCommand,
) -> Result<FixtureRuntimeCommand, CliError> {
    if options.browser_binding.is_some() || options.run_id.is_some() || options.server.is_some() {
        Err(CliError::new(
            "--browser-binding、--run-id 与 --server 只用于浏览器阶段",
        ))
    } else {
        Ok(command)
    }
}

fn bind_command(
    options: Options,
    runtime: FixtureRuntimePaths,
) -> Result<FixtureRuntimeCommand, CliError> {
    let run_id = options
        .run_id
        .ok_or_else(|| CliError::new("fixture runtime bind 缺少必需参数 --run-id"))?;
    validate_run_id(&run_id)?;
    let server = options
        .server
        .ok_or_else(|| CliError::new("fixture runtime bind 缺少必需参数 --server"))?;
    let browser_binding = validate_binding(
        options.browser_binding,
        &runtime,
        LocalTestPathKind::OutputFile,
    )?;
    if browser_binding.file_name().and_then(|value| value.to_str())
        != Some(format!("browser-binding-{run_id}.json").as_str())
    {
        return Err(CliError::new(
            "fixture runtime bind 的绑定文件名必须与 run id 一致",
        ));
    }
    Ok(FixtureRuntimeCommand::Bind {
        binding: FixtureBrowserBinding {
            runtime,
            browser_binding,
        },
        run_id,
        server,
    })
}

fn browser_command(
    options: Options,
    runtime: FixtureRuntimePaths,
    operation: &str,
) -> Result<FixtureRuntimeCommand, CliError> {
    if options.run_id.is_some() || options.server.is_some() {
        return Err(CliError::new(
            "fixture runtime 浏览器消费阶段不接受 --run-id 或 --server",
        ));
    }
    let browser_binding = validate_binding(
        options.browser_binding,
        &runtime,
        LocalTestPathKind::ExistingFile,
    )?;
    let binding = FixtureBrowserBinding {
        runtime,
        browser_binding,
    };
    Ok(match operation {
        "browser" => FixtureRuntimeCommand::Browser(binding),
        "browser-verify" => FixtureRuntimeCommand::BrowserVerify(binding),
        "browser-close" => FixtureRuntimeCommand::BrowserClose(binding),
        _ => unreachable!("operation 已由调用方核验"),
    })
}

fn validate_binding(
    value: Option<PathBuf>,
    runtime: &FixtureRuntimePaths,
    kind: LocalTestPathKind,
) -> Result<PathBuf, CliError> {
    let value = value
        .ok_or_else(|| CliError::new("fixture runtime 浏览器阶段缺少必需参数 --browser-binding"))?;
    validate(&value, &root_dir(), kind, "浏览器绑定")?;
    if value.parent() != Some(runtime.output.as_path()) {
        return Err(CliError::new("浏览器绑定必须直接位于本次运行目录"));
    }
    Ok(value)
}

fn validate_write(operation: &str, write: bool) -> Result<(), CliError> {
    let required = matches!(operation, "build" | "start" | "stop" | "bind" | "browser");
    if required && !write {
        Err(CliError::new(format!(
            "fixture runtime {operation} 必须显式传入 --write"
        )))
    } else if !required && write {
        Err(CliError::new(format!(
            "fixture runtime {operation} 是只读操作，不接受 --write"
        )))
    } else {
        Ok(())
    }
}

fn set_path(target: &mut Option<PathBuf>, name: &str, value: &str) -> Result<(), CliError> {
    if target.is_some() {
        return Err(CliError::new(format!("{name} 不能重复")));
    }
    *target = Some(PathBuf::from(value));
    Ok(())
}

fn set_string(target: &mut Option<String>, name: &str, value: &str) -> Result<(), CliError> {
    if target.is_some() {
        return Err(CliError::new(format!("{name} 不能重复")));
    }
    *target = Some(value.to_owned());
    Ok(())
}

fn set_server(target: &mut Option<FixtureServer>, name: &str, value: &str) -> Result<(), CliError> {
    if target.is_some() {
        return Err(CliError::new(format!("{name} 不能重复")));
    }
    *target = Some(match value {
        "dev" => FixtureServer::Dev,
        "preview" => FixtureServer::Preview,
        _ => return Err(CliError::new("--server 只允许 dev 或 preview")),
    });
    Ok(())
}

fn validate_run_id(value: &str) -> Result<(), CliError> {
    if value.len() > 64
        || value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || !value
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    {
        return Err(CliError::new(
            "--run-id 必须以小写字母或数字开头，且只包含小写字母、数字或连字符，最长 64 字节",
        ));
    }
    Ok(())
}

fn validate(
    value: &Path,
    root: &Path,
    kind: LocalTestPathKind,
    label: &str,
) -> Result<(), CliError> {
    validate_local_test_path(value, root, kind)
        .map(|_| ())
        .map_err(|error| CliError::new(format!("{label}无效：{error}")))
}

fn valid_value(value: &str) -> bool {
    !value.is_empty() && !value.starts_with('-') && !value.contains(['\n', '\r', '\0'])
}
