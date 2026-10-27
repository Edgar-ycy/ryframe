//! 只在正式恢复阶段创建私有配置快照；摘要和实例编号改写复用既有原生函数。
use super::frame::Frame;
use crate::{
    Result,
    ci::databases::files,
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::{clone::native::json as document, fixture_control},
};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub(super) enum Selection<'a> {
    Copy(&'a Path),
    Bound { api: &'a Path, worker: &'a Path },
}

pub(super) fn register(
    frame: &Frame<'_>,
    request: &Value,
    output: &Path,
    binaries: &Value,
    selection: Selection<'_>,
) -> Result<Value> {
    frame.checkpoint()?;
    let runtime = output.join("runtime");
    fs::create_dir(&runtime)
        .map_err(|error| format!("无法创建本次恢复 runtime：{error}；保留现场，不覆盖或重放"))?;
    document::write_new(&runtime.join("binaries.json"), binaries)?;
    let execution = Path::new(document::text(&request["execution_backend"], "恢复执行根")?);
    let guard = if request["format_version"].as_u64() == Some(2) {
        frame.backend
    } else {
        execution
    };
    let (api, worker) = match selection {
        Selection::Copy(source) => copy(guard, &output.join("runtime-configuration"), source)?,
        Selection::Bound { api, worker } => (api.to_owned(), worker.to_owned()),
    };
    let contract =
        fixture_control::full_stack_runtime_contract(execution, &runtime, &api, &worker)?;
    document::write_new(&runtime.join("runtime.json"), &contract)?;
    if fixture_control::verify_full_stack_runtime(execution, &runtime)? != contract {
        return Err("恢复 runtime 发布后与实际配置、产物不同，保留现场，禁止重放".into());
    }
    frame.checkpoint()?;
    Ok(contract)
}

fn copy(guard: &Path, output: &Path, source: &Path) -> Result<(PathBuf, PathBuf)> {
    validate_absolute_path(source, LocalTestPathKind::ExistingDirectory)?;
    let backend = source.parent().ok_or("源配置缺少执行根")?;
    let table = crate::rate_limit::load_app_table(backend, Some(source), "test")
        .map_err(|_| "源配置读取失败；检查显式 app.test.toml，不回显配置正文")?;
    let api_id = table
        .get("snowflake_worker_id")
        .and_then(toml::Value::as_integer)
        .filter(|value| (0..=1023).contains(value) && *value != 2)
        .ok_or("源 API 实例编号无效或与 Worker 编号2冲突")?;
    let names = ["app.toml", "app.test.toml"];
    let before = crate::recovery::source::evidence::generation_request::configuration::capture(
        guard, source,
    )?;
    files::private_directory(output)
        .map_err(|_| "无法创建私有恢复配置目录；检查已有现场、路径和权限，禁止覆盖")?;
    for (name, id) in [("config", u64::try_from(api_id)?), ("worker-config", 2)] {
        let destination = output.join(name);
        files::private_directory(&destination)
            .map_err(|_| "无法创建私有角色配置目录，保留已创建现场，禁止覆盖")?;
        for filename in names {
            if before["files"].get(filename).is_none() {
                continue;
            }
            let mut bytes = fs::read(source.join(filename))?;
            if filename == "app.test.toml" {
                let text = std::str::from_utf8(&bytes)
                    .map_err(|_| "源 app.test.toml 不是 UTF-8")?
                    .replace("\r\n", "\n")
                    .replace('\r', "\n");
                let result = files::dispatch("instance-text", json!({"text":text,"worker_id":id}))
                    .map_err(
                        |_| "角色实例配置改写失败；检查 TOML 根字段和实例编号，不回显配置正文",
                    )?;
                let text = document::text(&result["text"], "角色实例配置结果")?;
                bytes = if cfg!(windows) {
                    text.replace('\n', "\r\n").into_bytes()
                } else {
                    text.as_bytes().to_vec()
                };
            }
            write(&destination.join(filename), &bytes)?;
        }
    }
    if crate::recovery::source::evidence::generation_request::configuration::capture(guard, source)?
        != before
    {
        return Err(
            "源配置在快照期间漂移，包括原缺省app.toml的出现；保留私有快照，禁止继续或重放".into(),
        );
    }
    let canonical =
        crate::recovery::clone::native::target_verify::input::request_configuration::canonical_path;
    Ok((
        canonical(&output.join("config"))?,
        canonical(&output.join("worker-config"))?,
    ))
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
