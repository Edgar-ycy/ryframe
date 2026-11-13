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
    ci::databases::files::private_directory,
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
    crate::recovery::b0::matches(&request["authority"])
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
    private_directory(path).map_err(|_| "无法创建 B0 私有启动配置目录；保留原现场".into())
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

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn projected_b0_configuration_directory_removes_inherited_windows_aces() {
        let local = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let temporary = tempfile::tempdir_in(local).unwrap();
        let generation = temporary.path().join("generation");
        private_directory(&generation).unwrap();

        let ordinary = generation.join("ordinary");
        fs::create_dir(&ordinary).unwrap();
        assert!(has_inherited_aces(&ordinary), "普通测试目录应继承访问权限");

        let directory = generation.join(b0::config_directory("api").unwrap());
        create_directory(&directory).unwrap();
        assert!(!has_inherited_aces(&directory), "B0 配置目录继承了访问权限");
    }

    fn has_inherited_aces(path: &Path) -> bool {
        let system_root = std::env::var_os("SystemRoot").unwrap();
        let executable = Path::new(&system_root).join("System32/icacls.exe");
        assert!(executable.is_absolute(), "Windows 系统目录必须为绝对路径");
        let output = Command::new(executable)
            .arg(path)
            .env_clear()
            .env("SystemRoot", system_root)
            .output()
            .unwrap();
        assert!(output.status.success(), "无法读取测试目录 ACL");
        output.stdout.windows(3).any(|marker| marker == b"(I)")
    }
}
