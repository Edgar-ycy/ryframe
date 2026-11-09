//! 全栈 CI 只准备隔离文件，不启动服务或连接远端。
use super::integration_configuration::files;
use crate::{
    Result,
    check::{TaskExecutor, TaskPlan},
    cli::FullStackConfigOptions,
};
use serde_json::json;
use std::{
    fs,
    net::TcpListener,
    path::{Component, Path},
};

#[path = "full_stack_config/source.rs"]
mod source;

pub(crate) fn plan(options: &FullStackConfigOptions) -> Result<TaskPlan> {
    for (field, path) in [
        ("template", &options.template),
        ("runtime-dir", &options.runtime_dir),
        ("source-backend", &options.source_backend),
        ("source-frontend", &options.source_frontend),
    ] {
        if !path.is_absolute()
            || path
                .to_str()
                .is_none_or(|value| value.chars().any(char::is_control))
            || path.components().any(|v| matches!(v, Component::ParentDir))
        {
            return Err(format!(
                "--{field} 必须是无控制字符、无父跳转的绝对路径；请修正该参数，不从当前目录猜测来源"
            )
            .into());
        }
    }
    for value in [&options.run_id, &options.attempt] {
        if value.is_empty()
            || !value.bytes().all(|b| b.is_ascii_digit())
            || value.parse::<u64>().ok().is_none_or(|v| v == 0)
        {
            return Err("运行编号和 attempt 必须是正整数字符串；请传入本次 Job 元数据".into());
        }
    }
    if options.source_sha.len() != 40
        || !options.source_sha.bytes().all(|b| b.is_ascii_hexdigit())
        || options.source_sha.bytes().all(|b| b == b'0')
    {
        return Err("source-sha 必须是本次40位非零提交；核对精确来源".into());
    }
    for value in [
        &options.mysql_container,
        &options.redis_container,
        &options.s3_container,
    ] {
        if value.is_empty()
            || value.len() > 128
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        {
            return Err("服务容器标识无效；请传入本次 Job 的容器元数据".into());
        }
    }
    if !options.plan && !options.write {
        return Err("准备全栈配置需要 --write；只读预览请使用 --plan".into());
    }
    TaskPlan::sequence(&[TaskExecutor::CiFullStackConfig])
}

pub(crate) fn run(
    root: &Path,
    options: &FullStackConfigOptions,
    expected: &TaskPlan,
) -> Result<()> {
    if plan(options)? != *expected {
        return Err("全栈配置任务图变化；重新核对参数".into());
    }
    if options.plan {
        return Err("预览不得执行文件准备任务".into());
    }
    let output = root.join(".local-tests/full-stack-config");
    execute(root, options, &output).map_err(|error| {
        format!("全栈配置准备失败；运行目录={}；配置目录={}；{error}；保留局部现场，核对来源与输入；新隔离场景须先正式record-pair再准备，不自动重放", options.runtime_dir.display(), output.display()).into()
    })
}

fn absent(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(files::io_error(path, "输出前像", error).into()),
        Ok(_) => Err(files::diagnostic(path, "输出前像", "输出已存在或是链接，禁止覆盖").into()),
    }
}

fn unchanged(path: &Path, original: &files::Image) -> Result<()> {
    if files::read_required(path)? != *original {
        return Err(files::diagnostic(path, "输入后像", "原始文件变化，禁止继续发布").into());
    }
    Ok(())
}

fn execute(root: &Path, options: &FullStackConfigOptions, output: &Path) -> Result<()> {
    files::require_directory(root)?;
    files::require_directory(options.runtime_dir.parent().ok_or("运行目录缺少父级")?)?;
    let common = root.join("config/app.toml");
    let base = files::read_required(&common)?;
    let template = files::read_required(&options.template)?;
    let mut table: toml::Table = files::parse(&template.bytes, &options.template)?;
    let source = source::Source::capture(options, &common)?;
    let control = root.join(".local-tests/full-stack-control.json");
    files::directory_available(output.parent().ok_or("配置目录缺少父级")?)?;
    for path in [output, &control] {
        absent(path)?;
    }
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .map_err(|e| files::io_error(&options.runtime_dir, "选择健康端口", e))?;
    derive(&mut table, options, listener.local_addr()?.port())?;
    let data = toml::to_string(&table).map_err(|_| "配置序列化失败；核对模板字段，不输出正文")?;
    unchanged(&common, &base)?;
    unchanged(&options.template, &template)?;
    source.check(options)?;
    files::create_parents(output.parent().ok_or("配置父级缺失")?)?;
    private_directory(output)?;
    for (path, bytes) in [
        (output.join("app.toml"), base.bytes.as_slice()),
        (output.join("app.test.toml"), data.as_bytes()),
    ] {
        unchanged(&common, &base)?;
        unchanged(&options.template, &template)?;
        source.check(options)?;
        files::publish(&path, bytes)?;
    }
    ryframe_config::AppConfig::load(output, ryframe_config::Environment::Test).map_err(|error| {
        format!("阶段=完整应用配置校验失败；{error}；请修正对应模板/公共配置字段；已写本地文件保留，不发布控制成功文件")
    })?;
    unchanged(&common, &base)?;
    unchanged(&options.template, &template)?;
    let value = json!({"format_version":1,"runtime_dir":options.runtime_dir,
        "mysql_container_id":options.mysql_container,"redis_container_id":options.redis_container,"s3_container_id":options.s3_container});
    source.check(options)?;
    files::publish(&control, &serde_json::to_vec_pretty(&value)?)?;
    for (path, expected) in [
        (output.join("app.toml"), base.bytes.as_slice()),
        (output.join("app.test.toml"), data.as_bytes()),
    ] {
        if files::read_required(&path)?.bytes != expected {
            return Err(files::diagnostic(&path, "输出后像", "发布配置变化，保留现场").into());
        }
    }
    if files::read_required(&control)?.bytes != serde_json::to_vec_pretty(&value)? {
        return Err(files::diagnostic(&control, "输出后像", "控制文件变化，保留现场").into());
    }
    unchanged(&common, &base)?;
    unchanged(&options.template, &template)?;
    source.check(options)?;
    drop(listener);
    Ok(())
}

fn private_directory(path: &Path) -> Result<()> {
    super::databases::files::private_directory(path)
        .map_err(|_| files::diagnostic(path, "创建私有目录", "目录/权限设置失败，保留现场").into())
}

fn derive(table: &mut toml::Table, options: &FullStackConfigOptions, port: u16) -> Result<()> {
    table.insert(
        "scope_id".into(),
        format!("ci-{}-{}", options.run_id, options.attempt).into(),
    );
    let changes = [
        (
            vec!["database", "primary", "database"],
            toml::Value::from(format!(
                "ryframe_e2e_{}_{}",
                options.run_id, options.attempt
            )),
        ),
        (
            vec!["jobs", "health_port"],
            toml::Value::from(i64::from(port)),
        ),
        (
            vec!["object_storage", "local_base_dir"],
            toml::Value::from(
                options
                    .runtime_dir
                    .join("storage")
                    .to_str()
                    .ok_or("运行路径必须UTF-8")?,
            ),
        ),
        (
            vec!["reset", "state_directory"],
            toml::Value::from(
                options
                    .runtime_dir
                    .join("reset-state")
                    .to_str()
                    .ok_or("运行路径必须UTF-8")?,
            ),
        ),
        (
            vec!["reset", "source_sha"],
            toml::Value::from(options.source_sha.clone()),
        ),
    ];
    for (keys, value) in changes {
        let mut current = &mut *table;
        for key in &keys[..keys.len() - 1] {
            current = current
                .get_mut(*key)
                .and_then(toml::Value::as_table_mut)
                .ok_or("CI模板缺少必需配置表；恢复正式模板，不输出正文")?;
        }
        current.insert(keys[keys.len() - 1].into(), value);
    }
    Ok(())
}
