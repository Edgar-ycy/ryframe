//! 仅创建或核对既有私有运行配置和交接文件；不启动任何产品进程。
use super::context::Context;
use crate::{
    Result,
    ci::databases::files,
    recovery::{
        clone::native::{
            json as document,
            resources::common,
            target_verify::{self, guard::Inputs},
        },
        fixture_control,
    },
};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

pub(super) fn run(context: &Context<'_>) -> Result<Value> {
    let product = context.product_root()?;
    super::images::verify(context, "before")?;
    let build = common::read_bound(
        context.backend,
        &context.post.request["backend_build"],
        "产品构建",
    )?;
    let maintenance = common::read_bound(
        context.backend,
        &context.target["maintenance_build"],
        "维护构建",
    )?;
    let binaries = json!({"ryframe":build["artifacts"]["api"]["executable"],
        "ryframe-worker":build["artifacts"]["worker"]["executable"],
        "ryframe-reset":maintenance["artifacts"]["reset"]["executable"],
        "ryframe-migrate":maintenance["artifacts"]["migrate"]["executable"]});
    let runtime = &context.post.runtime;
    if !runtime.try_exists()? {
        fs::create_dir(runtime)?;
    }
    require_runtime_receipts(context.backend, runtime)?;
    context.check()?;
    super::files::publish(context.backend, &runtime.join("binaries.json"), &binaries)?;
    let (api, worker) = snapshot(context)?;
    let contract = fixture_control::full_stack_runtime_contract(&product, runtime, &api, &worker)?;
    super::files::publish(context.backend, &runtime.join("runtime.json"), &contract)?;
    if fixture_control::verify_full_stack_runtime(&product, runtime)? != contract {
        return Err("准备发布后运行合同或配置漂移；保留现场，不自动重放".into());
    }
    let handoff = json!({"registration":context.post.descriptor,"runtime":common::binding(&runtime.join("runtime.json"))?,
        "binaries":common::binding(&runtime.join("binaries.json"))?,"api_url":context.selected["api_url"],"worker_started":false});
    context.check()?;
    super::files::publish(context.backend, &runtime.join("handoff.json"), &handoff)?;
    let frozen = Inputs::open([
        runtime.join("binaries.json"),
        runtime.join("runtime.json"),
        runtime.join("handoff.json"),
        api.join("app.toml"),
        api.join("app.test.toml"),
        worker.join("app.toml"),
        worker.join("app.test.toml"),
    ])?;
    super::images::verify(context, "after")?;
    if fixture_control::verify_full_stack_runtime(&product, runtime)? != contract {
        return Err("准备后复核运行配置或完整产物不同".into());
    }
    frozen.check()?;
    context.check()?;
    Ok(
        json!({"status":"post_copy_api_prepared","handoff":common::binding(&runtime.join("handoff.json"))?,
        "worker_started":false,"restore_qualified":false}),
    )
}

pub(in crate::recovery::clone::native) fn require_runtime_receipts(
    backend: &Path,
    runtime: &Path,
) -> Result<()> {
    common::local_directory(backend, runtime, "原生运行准备目录")?;
    for entry in fs::read_dir(runtime)? {
        let entry = entry?;
        if !["binaries.json", "runtime.json", "handoff.json"]
            .contains(&entry.file_name().to_str().ok_or("运行文件名无效")?)
        {
            return Err("runtime已有生产者、未知文件或控制证据，不能重新准备或覆盖".into());
        }
        common::common_file(backend, &entry.path(), "已有准备文件")?;
    }
    Ok(())
}

fn snapshot(context: &Context<'_>) -> Result<(PathBuf, PathBuf)> {
    let environment = context.environment()?;
    let inputs = target_verify::storage::StorageInputs {
        backend: context.backend,
        request: &context.target,
        environment: &environment,
        observation_dir: &context.output,
    };
    let execution = inputs.execution_backend()?;
    let (source, original) = target_verify::input::request_configuration::fresh_files(
        &execution,
        &environment,
        &context.target,
    )?;
    let source_files = [source.join("app.toml"), source.join("app.test.toml")];
    let protected = Inputs::open(source_files.clone())?;
    let table = crate::rate_limit::load_app_table(&execution, Some(&source), "test")
        .map_err(|_| "目标文件配置读取失败；核对app.test.toml，不回显正文")?;
    let id = table
        .get("snowflake_worker_id")
        .and_then(toml::Value::as_integer)
        .filter(|v| (0..=1023).contains(v) && *v != 2)
        .ok_or("API实例编号无效或与Worker2冲突")?;
    let name = context
        .post
        .runtime
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or("运行目录名无效")?;
    let configuration = context
        .post
        .runtime
        .parent()
        .ok_or("运行目录父级缺失")?
        .join(format!("{name}-configuration"));
    if configuration.try_exists()? {
        common::local_directory(context.backend, &configuration, "原私有配置根")?;
        exact_names(&configuration, &["config", "worker-config"])?;
    } else {
        private(&configuration)?;
    }
    for (name, id) in [("config", u64::try_from(id)?), ("worker-config", 2)] {
        let destination = configuration.join(name);
        if destination.try_exists()? {
            common::local_directory(context.backend, &destination, "原角色配置目录")?;
            exact_names(&destination, &["app.toml", "app.test.toml"])?;
        } else {
            private(&destination)?;
        }
        for source in &source_files {
            protected.check()?;
            let filename = source.file_name().ok_or("源配置文件名缺失")?;
            let mut bytes = fs::read(source)?;
            if filename == "app.test.toml" {
                bytes = instance(&bytes, id)?;
            }
            write(context.backend, &destination.join(filename), &bytes)?;
            protected.check()?;
        }
    }
    protected.check()?;
    context.check()?;
    if target_verify::input::request_configuration::fresh_files(
        &execution,
        &environment,
        &context.target,
    )? != (source, original)
    {
        return Err("目标原配置在私有快照期间变化；保留现场，禁止重放".into());
    }
    let canonical = target_verify::input::request_configuration::canonical_path;
    Ok((
        canonical(&configuration.join("config"))?,
        canonical(&configuration.join("worker-config"))?,
    ))
}

fn instance(bytes: &[u8], id: u64) -> Result<Vec<u8>> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| "目标配置不是UTF-8")?
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let value = files::dispatch("instance-text", json!({"text":text,"worker_id":id}))
        .map_err(|_| "角色实例配置改写失败；核对TOML根字段，不回显正文")?;
    let text = document::text(&value["text"], "角色实例文本")?;
    Ok(if cfg!(windows) {
        text.replace('\n', "\r\n").into_bytes()
    } else {
        text.as_bytes().to_vec()
    })
}

fn private(path: &Path) -> Result<()> {
    files::private_directory(path)
        .map_err(|_| "无法独占创建私有配置目录；保留已有现场，禁止覆盖".into())
}

fn exact_names(path: &Path, expected: &[&str]) -> Result<()> {
    let actual = fs::read_dir(path)?
        .map(|v| Ok(v?.file_name()))
        .collect::<Result<std::collections::BTreeSet<_>>>()?;
    if actual
        != expected
            .iter()
            .map(|name| std::ffi::OsString::from(*name))
            .collect()
    {
        return Err("已有配置目录包含未知或缺失文件，禁止覆盖部分准备".into());
    }
    Ok(())
}

fn write(backend: &Path, path: &Path, bytes: &[u8]) -> Result<()> {
    if path.try_exists()? {
        let before = common::binding(&common::common_file(backend, path, "已有私有配置")?)?;
        if fs::read(path)? != bytes {
            return Err("已有私有配置与原目标文件不同，拒绝覆盖".into());
        }
        common::bound_file(backend, &before, "已有私有配置后像")?;
        return Ok(());
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}
