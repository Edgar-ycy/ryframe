//! 前端有限命令的冻结输入；进程监督和整树回收仍归 full_stack 唯一引擎。
use super::{model::*, plan, process, runtime};
use crate::{Result, recovery::clone::native::json as strict};
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn publish(
    backend: &Path,
    invocation: &super::dependency::FrontendCommand<'_>,
) -> Result<(Value, std::fs::File)> {
    directory(
        invocation
            .directory
            .parent()
            .ok_or("前端命令目录缺少父目录")?,
    )?;
    safe(invocation.directory)?;
    std::fs::create_dir(invocation.directory)?;
    let value = json!({"format_version":1,"kind":"full-stack-frontend-command",
        "coordinator":backend,
        "binding":invocation.binding,"arguments":invocation.arguments,"environment":invocation.environment,
        "timeout_seconds":invocation.timeout});
    let path = invocation.directory.join("runtime.json");
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)?;
    serde_json::to_writer_pretty(&mut file, &value)?;
    use std::io::Write;
    file.write_all(b"\n")?;
    file.sync_all()?;
    let held = same_file::Handle::from_file(file.try_clone()?)?;
    let guard = snapshot(&path, MAX_BYTES)?;
    check(
        same_file::Handle::from_path(&path)? == held && document(&path)?.0 == value,
        "前端命令发布时被替换或改写",
    )?;
    Ok((guard, file))
}

pub(super) fn handoff(directory: &Path, guard: &Value) -> Result<()> {
    strict::exact(guard, &["descriptor", "state"], "前端命令交接绑定")?;
    check(
        path(&guard["descriptor"]["path"])? == directory.join("runtime.json"),
        "前端交接必须绑定本阶段 runtime.json",
    )?;
    unchanged(guard)
}

pub(super) fn contract(backend: &Path, output: &Path) -> Result<Value> {
    let (value, guard) = document(&output.join("runtime.json"))?;
    strict::exact(
        &value,
        &[
            "format_version",
            "kind",
            "coordinator",
            "binding",
            "arguments",
            "environment",
            "timeout_seconds",
        ],
        "前端命令合同",
    )?;
    check(
        value["format_version"] == 1 && value["kind"] == "full-stack-frontend-command",
        "前端命令合同类型无效",
    )?;
    check(
        path(&value["coordinator"])? == backend,
        "前端命令协调器不一致",
    )?;
    let binding = &value["binding"];
    let frontend = path(&binding["frontend"]["path"])?;
    let runtime_file = path(&binding["runtime"]["path"])?;
    let runtime_dir = runtime_file.parent().ok_or("浏览器运行绑定缺少父目录")?;
    let index = stage(binding, &frontend, runtime_dir, output)?;
    check(
        value["arguments"] == binding["commands"][index],
        "前端命令参数与冻结阶段不一致",
    )?;
    let boot = runtime::bootstrap(backend, &path(&binding["bootstrap"]["path"])?)?;
    check(
        binding["backend"]["path"] == boot["execution"],
        "前端命令后端来源不匹配",
    )?;
    let verified = runtime::Runtime::load(backend, &path(&boot["bootstrap"])?, runtime_dir)?;
    check(
        verified.receipt["scope_id"] == binding["scope_id"],
        "前端命令 scope 不匹配",
    )?;
    let mut guards = plan::input_guards(binding)?;
    guards.push(guard);
    let sources = [
        plan::SourceGuard::capture(&path(&boot["execution"])?, &binding["backend"]["source"])?,
        plan::SourceGuard::capture(&frontend, &binding["frontend"]["source"]["generated"])?,
    ];
    validate_environment(&value["environment"], &boot["private"], binding)?;
    let command = process::command(binding, &value["arguments"], cfg!(windows))?;
    let executable = path(&json!(command.first().ok_or("前端命令为空")?))?;
    let artifact = bound(&executable)?;
    check(
        value["timeout_seconds"]
            .as_u64()
            .is_some_and(|v| v > 0 && v <= 7200),
        "前端命令超时无效",
    )?;
    for source in sources {
        source.unchanged()?;
    }
    for guard in guards {
        unchanged(&guard)?;
    }
    verified.verify()?;
    Ok(
        json!({"scope_id":binding["scope_id"],"artifacts":{"frontend":artifact},
        "frontend_completion":"exit-and-members","command":command,"cwd":frontend,
        "environment":value["environment"],"timeout_seconds":value["timeout_seconds"]}),
    )
}

fn stage(binding: &Value, frontend: &Path, runtime_dir: &Path, output: &Path) -> Result<usize> {
    let run_id = text(&binding["run_id"])?;
    check(
        regex::Regex::new(r"^[a-z0-9][a-z0-9-]{0,63}$")?.is_match(run_id),
        "浏览器 run id 无效",
    )?;
    let server = text(&binding["server"])?;
    check(matches!(server, "dev" | "preview"), "浏览器运行模式无效")?;
    check(
        binding["commands"] == super::binding::commands(frontend, server)?,
        "前端命令集与正式浏览器阶段不一致",
    )?;
    let outputs = plan::outputs(frontend, runtime_dir, run_id, server);
    let stages = if server == "dev" {
        vec![("browser_process", 0)]
    } else {
        vec![
            ("build_process", 0),
            ("build_verify_before_process", 1),
            ("browser_process", 2),
            ("build_verify_after_process", 3),
        ]
    };
    let (_, index) = stages
        .iter()
        .find(|(name, _)| outputs.get(*name).is_some_and(|path| path == output))
        .ok_or("前端命令目录不是该浏览器阶段的固定输出")?;
    Ok(*index)
}

fn validate_environment(environment: &Value, private: &Value, binding: &Value) -> Result<()> {
    let values = environment.as_object().ok_or("前端运行环境必须为对象")?;
    for (key, value) in values {
        check(value.is_string(), "前端运行环境值必须为字符串")?;
        check(
            [
                "PATH",
                "PATHEXT",
                "SYSTEMROOT",
                "WINDIR",
                "COMSPEC",
                "HOME",
                "USERPROFILE",
                "HOMEDRIVE",
                "HOMEPATH",
                "TEMP",
                "TMP",
                "APPDATA",
                "LOCALAPPDATA",
                "PROGRAMFILES",
                "PROGRAMFILES(X86)",
                "PROGRAMDATA",
                "SYSTEMDRIVE",
                "OS",
                "NUMBER_OF_PROCESSORS",
                "APP_ENV",
                "COREPACK_ENABLE_NETWORK",
                "COREPACK_HOME",
            ]
            .contains(&key.to_ascii_uppercase().as_str()),
            "前端运行环境含未授权配置入口",
        )?;
    }
    check(
        environment["APP_ENV"] == "test"
            && environment["COREPACK_ENABLE_NETWORK"] == "0"
            && environment["COREPACK_HOME"] == binding["tools"]["corepack_home"],
        "前端运行环境与冻结配置不一致",
    )?;
    for key in ["TEMP", "TMP"] {
        if private[key].is_string() {
            check(
                environment[key] == private[key],
                "前端临时目录与冻结环境不一致",
            )?;
        }
    }
    Ok(())
}
