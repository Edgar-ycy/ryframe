use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{
    Result,
    process::{ChildGroup, child_command, stop_child},
};

use super::model::{Binaries, BuildResult};

pub(super) fn build_candidate(
    group: &ChildGroup,
    root: &Path,
    shutdown: &AtomicBool,
) -> Result<BuildResult> {
    let started = Instant::now();
    let mut build = child_command("cargo");
    build
        .args([
            "build",
            "--locked",
            "-p",
            "ryframe",
            "--bin",
            "ryframe",
            "--bin",
            "ryframe-worker",
            "--bin",
            "ryframe-migrate",
        ])
        .current_dir(root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let mut child = group.spawn(&mut build)?;
    let Some(status) = wait_command(&mut child, shutdown)? else {
        return Ok(BuildResult::Cancelled);
    };
    if !status.success() {
        println!("候选编译失败（{:.1}s）。", started.elapsed().as_secs_f64());
        return Ok(BuildResult::Failed);
    }

    let target = target_debug_dir(root);
    let migration_binary = target.join(binary_name("ryframe-migrate"));
    let mut verify = Command::new(&migration_binary);
    verify
        .args(["control", "verify"])
        .env("APP_ENV", "dev")
        .env("APP_DATABASE_MIGRATION_MODE", "verify")
        .current_dir(root)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let mut child = group.spawn(&mut verify).map_err(|error| {
        format!(
            "无法启动迁移校验工具 {}：{error}",
            migration_binary.display()
        )
    })?;
    let Some(status) = wait_command(&mut child, shutdown)? else {
        return Ok(BuildResult::Cancelled);
    };
    if !status.success() {
        println!("候选迁移 verify 失败；数据库未被修改。");
        return Ok(BuildResult::Failed);
    }

    let generation =
        root.join("target/xtask/dev")
            .join(format!("{}-{}", std::process::id(), nonce()?));
    fs::create_dir_all(&generation)?;
    let copied = (|| {
        let api = copy_binary(&target.join(binary_name("ryframe")), &generation)?;
        let worker = copy_binary(&target.join(binary_name("ryframe-worker")), &generation)?;
        Ok::<_, Box<dyn std::error::Error>>(Binaries { api, worker })
    })();
    let binaries = match copied {
        Ok(binaries) => binaries,
        Err(error) => {
            let _ = remove_generation_dir(root, &generation);
            return Err(error);
        }
    };
    println!(
        "候选构建与迁移校验完成（{:.1}s）。",
        started.elapsed().as_secs_f64()
    );
    Ok(BuildResult::Ready(binaries))
}

fn copy_binary(source: &Path, generation: &Path) -> Result<PathBuf> {
    if !source.is_file() {
        return Err(format!("Cargo 构建未生成预期二进制：{}", source.display()).into());
    }
    let name = source
        .file_name()
        .ok_or_else(|| format!("二进制路径缺少文件名：{}", source.display()))?;
    let target = generation.join(name);
    let staged = generation.join(format!(".{}.copying", name.to_string_lossy()));
    fs::copy(source, &staged)?;
    fs::rename(&staged, &target)?;
    Ok(target)
}

fn wait_command(child: &mut Child, shutdown: &AtomicBool) -> Result<Option<ExitStatus>> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if shutdown.load(Ordering::Acquire) {
            stop_child(child)?;
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn target_debug_dir(root: &Path) -> PathBuf {
    let target = env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"));
    if target.is_absolute() {
        target.join("debug")
    } else {
        root.join(target).join("debug")
    }
}

fn binary_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

pub(super) fn cleanup_binaries(root: &Path, binaries: &Binaries) -> Result<()> {
    let generation = binaries
        .api
        .parent()
        .ok_or_else(|| format!("API 二进制缺少版本目录：{}", binaries.api.display()))?;
    if binaries.worker.parent() != Some(generation) {
        return Err("API 与 Worker 不属于同一版本目录，拒绝清理".into());
    }
    remove_generation_dir(root, generation)
}

fn remove_generation_dir(root: &Path, generation: &Path) -> Result<()> {
    let allowed_parent = root.join("target/xtask/dev");
    if generation.parent() != Some(allowed_parent.as_path()) || generation.file_name().is_none() {
        return Err(format!("版本目录越过清理白名单：{}", generation.display()).into());
    }
    match fs::remove_dir_all(generation) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("无法清理版本目录 {}：{error}", generation.display()).into()),
    }
}

fn nonce() -> Result<u128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("系统时间早于 Unix epoch：{error}"))?
        .as_nanos())
}
