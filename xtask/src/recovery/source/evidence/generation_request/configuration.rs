//! 请求绑定的配置窗口；新请求不从执行根或环境变量推断配置。
use crate::{
    Result,
    recovery::clone::native::{
        json as document, resources::common, s3::app_configuration, seed::generation::coordinator,
    },
};
use serde_json::Value;
use std::path::{Path, PathBuf};

pub(crate) fn capture(backend: &Path, directory: &Path) -> Result<Value> {
    let canonical = crate::recovery::clone::native::target_verify::input::request_configuration::canonical_path(directory)?;
    if canonical != directory {
        return Err("来源配置必须是规范绝对目录".into());
    }
    for name in ["app.toml", "app.test.toml"] {
        let path = directory.join(name);
        match std::fs::symlink_metadata(&path) {
            Err(error) if name == "app.toml" && error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
            Ok(_) => {
                crate::local_test_path::validate_external_existing_file(&path)?;
            }
        }
    }
    app_configuration::configuration_value(backend, directory, "test")
}

pub(crate) fn verify(backend: &Path, request: &Value) -> Result<Value> {
    coordinator::request(request)?;
    if request["format_version"].as_u64() == Some(1) {
        let execution = Path::new(document::text(&request["execution_backend"], "历史执行根")?);
        return app_configuration::configuration_value(
            execution,
            &execution.join("config"),
            "test",
        );
    }
    let expected = &request["configuration"];
    let directory = Path::new(document::text(&expected["directory"], "来源配置目录")?);
    let first = capture(backend, directory)?;
    if first != *expected {
        return Err("来源配置的目录、文件或缺省共同配置状态发生漂移；保留原请求，禁止重放".into());
    }
    let mut snapshots = Vec::new();
    app_configuration::load(backend, expected, &mut snapshots)?;
    if capture(backend, directory)? != first {
        return Err("来源配置在读取期间发生漂移，包括原缺省app.toml的出现".into());
    }
    app_configuration::unchanged(&snapshots)?;
    Ok(first)
}

pub(crate) fn directory(backend: &Path, request: &Value) -> Result<PathBuf> {
    let binding = verify(backend, request)?;
    Ok(PathBuf::from(document::text(
        &binding["directory"],
        "来源配置目录",
    )?))
}

pub(crate) fn for_start(backend: &Path, start: &Value) -> Result<(PathBuf, Value)> {
    let receipt = common::read_bound(backend, start, "来源原START")?;
    let request = common::read_bound(backend, &receipt["request"], "来源原请求")?;
    let binding = verify(backend, &request)?;
    let guard = if request["format_version"].as_u64() == Some(1) {
        PathBuf::from(document::text(&request["execution_backend"], "历史执行根")?)
    } else {
        backend.to_owned()
    };
    Ok((guard, binding))
}

pub(crate) fn execution_source(backend: &Path, request: &Value) -> Result<()> {
    coordinator::request(request)?;
    if request["format_version"].as_u64() == Some(1) {
        return Ok(());
    }
    let execution = Path::new(document::text(&request["execution_backend"], "执行源码根")?);
    if request["execution_source"].is_null() {
        if execution.parent().is_some_and(|parent| {
            parent.join("fixture.json").exists() || parent.join("execution-source.json").exists()
        }) || execution.join("execution-source.json").exists()
        {
            return Err(
                "生成业务执行根必须显式绑定正式execution-source.json；不能作为普通源码略过发布证明"
                    .into(),
            );
        }
        return Ok(());
    }
    let manifest = crate::recovery::fixture_control::source_pair::execution::verify(
        backend,
        &request["execution_source"],
    )?;
    if manifest["format_version"].as_u64() != Some(1)
        || manifest["kind"] != "reference-fixture-execution-source"
        || manifest["published"]["backend"]["path"] != request["execution_backend"]
        || manifest["published"]["backend"]["head"] != request["expected_backend_sha"]
    {
        return Err("执行源码发布收据与请求的规范后端根或精确提交不同".into());
    }
    common::bound_file(backend, &request["execution_source"], "执行源码发布收据")?;
    Ok(())
}
