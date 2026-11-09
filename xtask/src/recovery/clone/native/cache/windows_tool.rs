//! Windows Redis 的显式工具来源，复核同目录的运行库和实际版本。
use super::super::json as document;
use crate::Result;
use serde_json::{Value, json};
use std::path::Path;

const SETTING: &str = "verification.windows_redis";

pub(in crate::recovery) fn validate(value: &Value) -> Result<()> {
    document::exact(
        value,
        &["platform", "path", "sha256", "version", "dlls"],
        "Windows Redis 工具",
    )?;
    let path = document::text(&value["path"], "Windows Redis 程序")?;
    let version = document::text(&value["version"], "Windows Redis 版本")?;
    let dlls = value["dlls"]
        .as_object()
        .ok_or("Windows Redis 缺少 DLL 清单")?;
    if value["platform"] != "windows"
        || !program_path(path)
        || !value["sha256"].as_str().is_some_and(sha256)
        || !version_line(version)
        || dlls.is_empty()
        || !dlls
            .iter()
            .all(|(name, sha)| dll_name(name) && sha.as_str().is_some_and(sha256))
    {
        return Err("Windows Redis 工具路径、摘要、版本或 DLL 清单无效".into());
    }
    Ok(())
}

pub(in crate::recovery) fn package_bindings(tool: &Value) -> Result<Vec<Value>> {
    validate(tool)?;
    let program = document::text(&tool["path"], "Windows Redis 程序")?;
    let separator = program
        .rfind(['/', '\\'])
        .ok_or("Windows Redis 程序缺少父目录")?;
    let parent = &program[..=separator];
    let mut dlls = document::object(&tool["dlls"], "Windows Redis DLL 清单")?
        .iter()
        .collect::<Vec<_>>();
    dlls.sort_unstable_by_key(|&(name, _)| name);
    let mut bindings = vec![json!({"path":program,"sha256":tool["sha256"]})];
    bindings.extend(
        dlls.into_iter()
            .map(|(name, sha)| json!({"path":format!("{parent}{name}"),"sha256":sha})),
    );
    Ok(bindings)
}

pub(super) fn observe(backend: &Path) -> Result<Option<Value>> {
    crate::recovery::runtime::dataset_authority::reads::tool_settings(backend)?;
    let Some(settings) = ryframe_config::ToolSettings::load(backend)?
        .verification
        .windows_redis
    else {
        return Ok(None);
    };
    #[cfg(windows)]
    {
        let observed = local::observe(&settings);
        if ryframe_config::ToolSettings::load(backend)?
            .verification
            .windows_redis
            .as_ref()
            != Some(&settings)
        {
            return Err(format!("工具文件中的 {SETTING} 在版本核验期间变化").into());
        }
        observed.map(Some)
    }
    #[cfg(not(windows))]
    {
        drop(settings);
        Err(format!("{SETTING} 仅支持本机 Windows").into())
    }
}

#[cfg(windows)]
pub(super) fn verify_registered(backend: &Path, expected: &Value) -> Result<()> {
    validate(expected)?;
    crate::recovery::runtime::dataset_authority::reads::tool_settings(backend)?;
    let settings = ryframe_config::ToolSettings::load(backend)?
        .verification
        .windows_redis
        .ok_or("Windows Redis 缺少已登记工具配置")?;
    if expected["path"] != serde_json::to_value(&settings.program)?
        || expected["sha256"] != settings.sha256
        || expected["dlls"] != serde_json::to_value(&settings.dlls)?
    {
        return Err("Windows Redis 工具与冻结请求不同".into());
    }
    local::verify_registered(&settings)
}

#[cfg(windows)]
pub(super) fn verify_frozen(expected: &Value) -> Result<()> {
    validate(expected)?;
    let settings = ryframe_config::WindowsRedisSettings {
        program: document::text(&expected["path"], "原 Windows Redis 程序")?.into(),
        sha256: document::text(&expected["sha256"], "原 Windows Redis 程序摘要")?.into(),
        dlls: serde_json::from_value(expected["dlls"].clone())?,
    };
    local::verify_registered(&settings)
}

fn program_path(path: &str) -> bool {
    let Some((drive, rest)) = path.split_at_checked(3) else {
        return false;
    };
    let drive = drive.as_bytes();
    drive[0].is_ascii_alphabetic()
        && drive[1] == b':'
        && matches!(drive[2], b'/' | b'\\')
        && rest.split(['/', '\\']).all(|part| {
            !matches!(part, "" | "." | "..")
                && !part.ends_with([' ', '.'])
                && !part.contains([':', '<', '>', '"', '|', '?', '*'])
                && !part.chars().any(char::is_control)
        })
        && rest
            .rsplit(['/', '\\'])
            .next()
            .is_some_and(|name| name.eq_ignore_ascii_case("redis-server.exe"))
}

fn dll_name(name: &str) -> bool {
    name.strip_suffix(".dll").is_some_and(|stem| {
        !stem.is_empty()
            && !stem.starts_with('.')
            && !stem.ends_with('.')
            && stem.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'-' | b'_' | b'+')
            })
    })
}

fn sha256(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn version_line(line: &str) -> bool {
    let Some((version, build)) = line
        .strip_prefix("Redis server v=")
        .and_then(|value| value.split_once(' '))
    else {
        return false;
    };
    let parts: Vec<_> = version.split('.').collect();
    matches!(parts.as_slice(), ["7", minor, patch]
    if [minor, patch].into_iter().all(|part| {
        !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit())
    })) && build.split_ascii_whitespace().any(|part| part == "bits=64")
        && line.len() <= 256
        && line
            .bytes()
            .all(|byte| byte == b' ' || byte.is_ascii_graphic())
}

#[cfg(windows)]
mod local {
    use super::*;
    use crate::{
        local_test_path::validate_external_existing_file,
        recovery::{clone::native::digest, monitoring::execution::ProtectedInputs},
    };
    use ryframe_config::WindowsRedisSettings;
    use std::{
        collections::{BTreeMap, BTreeSet},
        path::PathBuf,
        time::Duration,
    };

    pub(super) fn observe(settings: &WindowsRedisSettings) -> Result<Value> {
        let program = settings
            .program
            .to_str()
            .ok_or("Windows Redis 路径不是 UTF-8")?;
        if !program_path(program)
            || !sha256(&settings.sha256)
            || settings.dlls.is_empty()
            || !settings
                .dlls
                .iter()
                .all(|(name, sha)| dll_name(name) && sha256(sha))
        {
            return Err(format!("工具文件中的 {SETTING} 路径或摘要清单无效").into());
        }
        let parent = settings
            .program
            .parent()
            .ok_or("Windows Redis 程序缺少父目录")?;
        let files = package_files(settings, parent);
        for (path, _) in &files {
            validate_external_existing_file(path)?;
        }
        let guard = ProtectedInputs::open(files.iter().map(|(path, _)| path.clone()))?;
        let result = (|| {
            verify_package(parent, &settings.dlls, &files)?;
            let version = version(program, parent);
            verify_package(parent, &settings.dlls, &files)?;
            let observed = json!({
                "platform":"windows", "path":settings.program, "sha256":settings.sha256,
                "version":version?, "dlls":settings.dlls,
            });
            validate(&observed)?;
            Ok(observed)
        })();
        guard.finish(result)
    }

    pub(super) fn verify_registered(settings: &WindowsRedisSettings) -> Result<()> {
        let parent = settings
            .program
            .parent()
            .ok_or("Windows Redis 程序缺少父目录")?;
        verify_package(parent, &settings.dlls, &package_files(settings, parent))
    }

    fn package_files<'a>(
        settings: &'a WindowsRedisSettings,
        parent: &Path,
    ) -> Vec<(PathBuf, &'a str)> {
        std::iter::once((settings.program.clone(), settings.sha256.as_str()))
            .chain(
                settings
                    .dlls
                    .iter()
                    .map(|(name, sha)| (parent.join(name), sha.as_str())),
            )
            .collect()
    }

    fn verify_package(
        parent: &Path,
        dlls: &BTreeMap<String, String>,
        files: &[(PathBuf, &str)],
    ) -> Result<()> {
        let mut actual = BTreeSet::new();
        for entry in crate::recovery::runtime::dataset_authority::reads::entries(parent)? {
            let entry = entry?;
            if entry
                .path()
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("dll"))
            {
                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| "Windows Redis DLL 名称不是 UTF-8")?;
                actual.insert(name);
            }
        }
        if !actual.iter().eq(dlls.keys()) {
            return Err(format!("Windows Redis 同目录 DLL 集合与 {SETTING}.dlls 不符").into());
        }
        for (path, expected) in files {
            validate_external_existing_file(path)?;
            if digest::file(path)?.sha256 != *expected {
                return Err(format!(
                    "Windows Redis 文件摘要与 {SETTING} 不符：{}",
                    path.display()
                )
                .into());
            }
        }
        Ok(())
    }

    fn version(program: &str, parent: &Path) -> Result<String> {
        let output = crate::process::command_output_with_input_timeout(
            parent,
            program,
            &["--version"],
            &[],
            None,
            Duration::from_secs(20),
        )
        .map_err(|_| "Windows Redis 固定版本命令未完成")?;
        if !output.status.success() || output.stdout.len() > 512 {
            return Err("Windows Redis 固定版本命令失败或超限".into());
        }
        let line = std::str::from_utf8(&output.stdout)?.trim_end_matches(['\r', '\n']);
        if !version_line(line) {
            return Err("Windows Redis 版本输出未确认 7.x 64 位构建".into());
        }
        Ok(line.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_tool_binding_is_a_distinct_exact_contract() {
        let value = json!({
            "platform":"windows",
            "path":"D:/Redis 本地/redis-server.exe",
            "sha256":"0".repeat(64),
            "version":"Redis server v=7.4.11 sha=00000000:0 malloc=libc bits=64 build=5bd2587910691cb8",
            "dlls":{"msys-2.0.dll":"1".repeat(64),"msys-stdc++-6.dll":"2".repeat(64)}
        });
        validate(&value).unwrap();
        assert_eq!(
            package_bindings(&value).unwrap(),
            [
                json!({"path":"D:/Redis 本地/redis-server.exe","sha256":"0".repeat(64)}),
                json!({"path":"D:/Redis 本地/msys-2.0.dll","sha256":"1".repeat(64)}),
                json!({"path":"D:/Redis 本地/msys-stdc++-6.dll","sha256":"2".repeat(64)}),
            ],
        );
        let mut backslash = value.clone();
        backslash["path"] = json!(r"D:\Redis 本地\redis-server.exe");
        assert_eq!(
            package_bindings(&backslash).unwrap()[1]["path"],
            r"D:\Redis 本地\msys-2.0.dll",
        );
        for (key, replacement) in [
            ("platform", json!("wsl")),
            ("path", json!("D:/tools/../redis-server.exe")),
            ("path", json!("redis-server.exe")),
            ("sha256", json!("A".repeat(64))),
            ("version", json!("Redis server v=7.4.11 bits=32")),
            ("dlls", json!({"../unbound.dll":"1".repeat(64)})),
            ("dlls", json!({})),
            ("distribution", json!("Ubuntu-24.04")),
        ] {
            let mut invalid = value.clone();
            invalid[key] = replacement;
            assert!(package_bindings(&invalid).is_err(), "{key}");
        }
    }
}
