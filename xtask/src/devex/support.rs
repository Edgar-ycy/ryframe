use std::{
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

use sha2::{Digest, Sha256};

use crate::{Result, workspace::remove_isolated_directory};

use super::model::{CacheState, DevexSuite, StepDefinition};

pub(super) fn frontend_fast_arguments(
    step: &StepDefinition,
    frontend: &Path,
    runner: &Path,
    target: &Path,
) -> Vec<String> {
    let token = Sha256::digest(target.as_os_str().as_encoded_bytes());
    let cache = frontend
        .join(".local-tests/devex-fast")
        .join(hex::encode(&token[..12]));
    let frontend = node_path(frontend);
    let runner = node_path(runner);
    let cache = node_path(&cache);
    step.args
        .iter()
        .map(|arg| {
            arg.replace("{frontend-cache}", &cache)
                .replace("{runner-frontend}", &runner)
                .replace("{frontend}", &frontend)
        })
        .collect()
}

fn node_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if cfg!(windows) {
        if let Some(share) = text.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{share}");
        }
        if let Some(drive) = text.strip_prefix(r"\\?\")
            && drive.as_bytes().get(1) == Some(&b':')
        {
            return drive.to_owned();
        }
    }
    text.into_owned()
}

#[cfg(all(test, windows))]
#[test]
fn common_frontend_cli_receives_node_compatible_windows_paths() {
    assert_eq!(node_path(Path::new(r"\\?\D:\ryframe")), r"D:\ryframe");
    assert_eq!(
        node_path(Path::new(r"\\?\UNC\server\share")),
        r"\\server\share"
    );
}

pub(crate) fn sample_target(
    run_dir: &Path,
    suite: DevexSuite,
    cache_state: CacheState,
    sequence: usize,
) -> PathBuf {
    let target_root = compiler_target_root(run_dir);
    if suite == DevexSuite::RustSccache {
        // 每轮成功后都会精确删除该目录。保持同一个目标路径能让 sccache
        // 复用编译键，同时又不会把 Cargo target 的增量产物误当作缓存命中。
        return target_root.join("sccache-measure");
    }
    if suite.uses_sccache() {
        return target_root.join(format!("sccache-measure-{sequence:03}"));
    }
    match cache_state {
        CacheState::Cold => target_root.join(format!("cold-{sequence:03}")),
        CacheState::Warm => target_root.join("warm"),
    }
}

pub(crate) fn compiler_target_root(run_dir: &Path) -> PathBuf {
    let Some(local_tests) = run_dir
        .ancestors()
        .find(|path| path.file_name().is_some_and(|name| name == ".local-tests"))
    else {
        return run_dir.join("cache");
    };
    let run_name = run_dir.file_name().unwrap_or_default().to_string_lossy();
    let digest = Sha256::digest(run_name.as_bytes());
    let token = digest
        .iter()
        .take(6)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    // Windows build-script 路径会继续嵌套 Cargo 包名和哈希；将临时 target
    // 放在 .local-tests/d 下，避免长工作树使 build-script 无法启动。
    local_tests.join("d").join(token)
}

pub(crate) fn cleanup_successful_sample_target(
    run_dir: &Path,
    target: &Path,
    suite: DevexSuite,
    cache_state: CacheState,
) -> Result<()> {
    if suite.is_runtime() || (cache_state != CacheState::Cold && !suite.uses_sccache()) {
        return Ok(());
    }
    remove_isolated_directory(&compiler_target_root(run_dir), target)
}

pub(super) fn display_step(step: &StepDefinition) -> String {
    format!("{} {}", step.program, step.args.join(" "))
}

pub(super) fn metric(value: Option<f64>) -> String {
    value.map_or_else(|| "n/a".to_owned(), |value| format!("{value:.1} ms"))
}

#[cfg(windows)]
pub(super) fn success_status() -> std::io::Result<ExitStatus> {
    Command::new("cmd").args(["/C", "exit", "0"]).status()
}

#[cfg(not(windows))]
pub(super) fn success_status() -> std::io::Result<ExitStatus> {
    Command::new("true").status()
}
