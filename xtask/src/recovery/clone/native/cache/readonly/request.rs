//! 只读缓存请求装载复用原模型；真实 Generation 引用不能由 JSON 锁编号替代。
use super::super::{
    super::{json as document, resources::common, target_verify},
    model, request,
};
use crate::Result;
use serde_json::{Value, json};
use std::path::Path;

pub(super) fn validate(
    backend: &Path,
    run: &Path,
    value: &Value,
    owner: Option<&target_verify::guard::Generation>,
) -> Result<Value> {
    request::run(
        "request-head",
        &json!({"request":value,
        "manifest":common::binding(&run.join("manifest.json"))?,
        "value":document::read(&run.join("manifest.json"))?}),
    )?;
    let filename = common::bound_file(backend, &value["initialized"], "缓存初始化历史")?;
    let (initial, original) = match owner {
        Some(owner)
            if owner
                .protects(filename.parent().ok_or("初始化历史没有父目录")?)
                .is_ok() =>
        {
            target_verify::published::owned_history(backend, &filename, owner)?
        }
        _ => target_verify::published::unlocked_history(backend, &filename)?,
    };
    let binding = target_verify::published::request_binding(backend, &original)?;
    request::run(
        "request-model",
        &json!({"request":value,"initial":initial,
        "original":original,"selected":binding["selected"]}),
    )?;
    let private = environment(backend, value, &original)?;
    process(backend, value, &binding["review"], None, true)?;
    Ok(private)
}

pub(super) fn environment(backend: &Path, value: &Value, original: &Value) -> Result<Value> {
    let private = common::read_bound(backend, &value["target_environment"], "缓存私有环境")?;
    let environment = request::run("environment-select", &json!({"private":private}))?;
    model::validate(backend, &value["process"], false)?;
    let execution = target_verify::input::request::execution_backend(
        backend,
        original.get("execution_backend"),
        &target_verify::input::request::SystemSourceObserver,
    )?
    .0;
    let environment_map = serde_json::from_value(environment.clone())?;
    let (directory, files) = target_verify::input::request_configuration::fresh_files(
        &execution,
        &environment_map,
        original,
    )?;
    let config = crate::rate_limit::load_app_table(&execution, Some(&directory), "test")
        .map_err(|_| "缓存配置加载失败；核对绑定 TOML，禁止环境覆盖")?;
    request::run(
        "environment-model",
        &json!({"request":value,"redis":config.get("redis")}),
    )?;
    if target_verify::input::request_configuration::fresh_files(
        &execution,
        &environment_map,
        original,
    )? != (directory, files)
    {
        return Err("缓存绑定文件在配置读取期间变化".into());
    }
    Ok(environment)
}

pub(super) fn successor(
    backend: &Path,
    run: &Path,
    value: &Value,
    source: &Value,
    effective: &Value,
    observe_files: bool,
) -> Result<Value> {
    request::run(
        "request-head",
        &json!({"request":value,
        "manifest":common::binding(&run.join("manifest.json"))?,"value":document::read(&run.join("manifest.json"))?}),
    )?;
    let original = &source["seed_target"];
    request::run(
        "pending-target",
        &json!({"target":common::read_bound(backend,
        &document::read(&common::bound_file(backend,&value["initialized"],"缓存初始化")?.parent()
            .ok_or("初始化没有父目录")?.join("prepare.json"))?["request"],"缓存原请求")?,
        "seed_target":original}),
    )?;
    let binding = target_verify::binding_request::validate(
        backend,
        original,
        &source["review_successor"]["predecessor_review"],
    )?;
    request::run(
        "request-model",
        &json!({"request":value,"initial":source["initialization"],
        "original":original,"selected":binding["selected"]}),
    )?;
    let private = environment(backend, value, original)?;
    process(
        backend,
        value,
        &binding["review"],
        Some(effective),
        observe_files,
    )?;
    Ok(private)
}

pub(super) fn process(
    backend: &Path,
    value: &Value,
    review: &Value,
    effective: Option<&Value>,
    observe_files: bool,
) -> Result<()> {
    model::validate(backend, &value["process"], effective.is_none())?;
    if let Some(effective) = effective {
        model::validate(backend, effective, observe_files)?;
    }
    let receipt = common::read_bound(backend, &value["process_receipt"], "缓存原进程收据")?;
    let directory = common::directory_identity(backend, &value["process"]["directory"])?;
    common::local_directory(
        backend,
        Path::new(document::text(
            &review["services"]["redis"]["directory"],
            "原缓存目录",
        )?),
        "原缓存目录",
    )?;
    let config = common::bound_file(
        backend,
        &value["previous"]["configuration"],
        "原 Redis 配置",
    )?;
    if effective.is_none() {
        target_verify::input::request::external_file(&value["previous"]["wsl"])?;
    }
    let binding = common::binding(&config)?;
    let text = std::fs::read_to_string(&config)?;
    let options = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(words)
        .collect::<Result<Vec<_>>>()?;
    request::run(
        "process-model",
        &json!({"request":value,"review":review,"receipt":receipt,
        "directory":directory,"config":config,"options":options}),
    )?;
    common::bound_file(backend, &binding, "原 Redis 配置")?;
    Ok(())
}

pub(in crate::recovery::clone::native::cache) fn words(line: &str) -> Result<Vec<String>> {
    let mut result = Vec::new();
    let mut word = String::new();
    let mut quoted = None;
    let mut started = false;
    let mut input = line.chars().peekable();
    while let Some(character) = input.next() {
        match (quoted, character) {
            (None, '#') => break,
            (Some('\''), '\'') | (Some('"'), '"') => quoted = None,
            (None, '\'' | '"') => {
                quoted = Some(character);
                started = true;
            }
            (Some('\''), _) => word.push(character),
            (_, '\\') => {
                let escaped = input.next().ok_or("Redis 配置包含未完成转义")?;
                if quoted == Some('"') && !matches!(escaped, '\\' | '"') {
                    word.push('\\');
                }
                word.push(escaped);
                started = true;
            }
            (None, ' ' | '\t' | '\r' | '\n') => {
                if started {
                    result.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            _ => {
                word.push(character);
                started = true;
            }
        }
    }
    if quoted.is_some() {
        return Err("Redis 配置存在未闭合引号".into());
    }
    if started {
        result.push(word);
    }
    Ok(result)
}
