//! 只为精确冻结的 B0 API 和 Worker 发布独立文件与短期子进程变量。
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    process::Command,
};

use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::super::b0;
use super::{
    Inputs, check, digest,
    legacy::{LegacyConfig, project},
    path, strict, table,
};
use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_local_test_path},
};

const BASE: &str = "# B0 的有效配置位于 app.test.toml。\n";
const ROLES: [&str; 2] = ["api", "worker"];

pub(super) fn recipe(config: &LegacyConfig) -> Value {
    let names = config.passwords.keys().collect::<Vec<_>>();
    json!({"worker_id":config.worker_id,"password_variables":names,
        "files":{"app.toml":describe(BASE.as_bytes()),"app.test.toml":describe(config.selected.as_bytes())}})
}

fn describe(bytes: &[u8]) -> Value {
    json!({"bytes":bytes.len(),"sha256":hex::encode(Sha256::digest(bytes))})
}

fn selected(request: &Value) -> bool {
    b0::matches(&request["authority"])
}

pub(in crate::recovery::runtime) fn materialize(
    inputs: &mut Inputs,
    request: &Value,
    generation: &Path,
) -> Result<()> {
    if !selected(request) {
        return Ok(());
    }
    let root = path(&request["tools"]["coordinator_root"])?;
    for role in ROLES {
        let config = load(inputs, request, role)?;
        let directory = generation.join(b0::config_directory(role).ok_or("B0 配置角色无效")?);
        validate_local_test_path(&directory, &root, LocalTestPathKind::NewDirectory)?;
        create_directory(&directory)?;
        for (name, bytes) in [
            ("app.toml", BASE.as_bytes()),
            ("app.test.toml", config.selected.as_bytes()),
        ] {
            let file = directory.join(name);
            validate_local_test_path(&file, &root, LocalTestPathKind::OutputFile)?;
            write_file(&file, bytes)?;
        }
    }
    verify_projected(inputs, request, generation)
}

fn create_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(path)?;
    }
    #[cfg(not(unix))]
    fs::create_dir(path)?;
    Ok(())
}

fn write_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
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

pub(in crate::recovery::runtime) fn verify_projected(
    inputs: &mut Inputs,
    request: &Value,
    generation: &Path,
) -> Result<()> {
    if selected(request) {
        for role in ROLES {
            verify_role(inputs, request, generation, role)?;
        }
    }
    Ok(())
}

pub(in crate::recovery::runtime) fn configure(
    inputs: &mut Inputs,
    request: &Value,
    generation: &Path,
    role: &str,
    command: &mut Command,
) -> Result<()> {
    if !selected(request) || !ROLES.contains(&role) {
        return Ok(());
    }
    let config = load(inputs, request, role)?;
    verify_role(inputs, request, generation, role)?;
    let directory = generation.join(b0::config_directory(role).ok_or("B0 配置角色无效")?);
    command
        .env("APP_CONFIG_DIR", directory)
        .env("SNOWFLAKE_WORKER_ID", config.worker_id.to_string())
        .envs(config.passwords);
    Ok(())
}

fn load(inputs: &mut Inputs, request: &Value, role: &str) -> Result<LegacyConfig> {
    let snapshot = &request["environment"]["configurations"][role];
    let directory = path(&snapshot["directory"])?;
    for (name, expected) in strict::object(&snapshot["files"], "B0 配置前像")? {
        check(
            !name.contains(['/', '\\']) && name.ends_with(".toml"),
            "B0 配置前像文件名无效",
        )?;
        protected(inputs, &directory.join(name), expected)?;
    }
    let config = project(table(&directory)?)?;
    check(
        recipe(&config) == snapshot["legacy"],
        "B0 配置投影与登记的启动合同不同",
    )?;
    Ok(config)
}

fn verify_role(inputs: &mut Inputs, request: &Value, generation: &Path, role: &str) -> Result<()> {
    let root = path(&request["tools"]["coordinator_root"])?;
    let directory = generation.join(b0::config_directory(role).ok_or("B0 配置角色无效")?);
    validate_local_test_path(&directory, &root, LocalTestPathKind::ExistingDirectory)?;
    let expected = &request["environment"]["configurations"][role]["legacy"]["files"];
    strict::exact(expected, &["app.toml", "app.test.toml"], "B0 启动配置文件")?;
    let names = fs::read_dir(&directory)?.collect::<std::io::Result<Vec<_>>>()?;
    check(names.len() == 2, "B0 启动配置包含未登记文件")?;
    for (name, expected) in strict::object(expected, "B0 启动配置文件")? {
        let file = directory.join(name);
        validate_local_test_path(&file, &root, LocalTestPathKind::ExistingFile)?;
        protected(inputs, &file, expected)?;
    }
    Ok(())
}

fn protected(inputs: &mut Inputs, path: &Path, expected: &Value) -> Result<()> {
    inputs.protect(path)?;
    let actual = digest::file(path)?;
    check(
        expected == &json!({"bytes":actual.bytes,"sha256":actual.sha256}),
        "B0 启动配置文件与登记摘要不同",
    )
}
