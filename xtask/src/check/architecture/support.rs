use std::{
    ffi::OsStr,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub(super) fn read_text(path: &Path, label: &str, errors: &mut Vec<String>) -> String {
    match fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) => {
            errors.push(format!(
                "无法读取{label} {}：{error}",
                display_relative(path)
            ));
            String::new()
        }
    }
}

pub(super) fn collect_files(
    root: &Path,
    directory: &Path,
    recursive: bool,
    predicate: impl Fn(&Path) -> bool,
) -> Result<Vec<PathBuf>, String> {
    let mut result = Vec::new();
    collect_into(root, directory, recursive, &predicate, &mut result)?;
    result.sort();
    Ok(result)
}

fn collect_into(
    root: &Path,
    directory: &Path,
    recursive: bool,
    predicate: &impl Fn(&Path) -> bool,
    result: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("无法枚举目录 {}：{error}", display_relative(directory)))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| format!("无法枚举目录 {}：{error}", display_relative(directory)))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("无法读取文件类型 {}：{error}", display_relative(&path)))?;
        if file_type.is_dir() {
            if recursive && !file_type.is_symlink() {
                collect_into(root, &path, true, predicate, result)?;
            }
        } else if file_type.is_file() && predicate(&path) {
            result.push(path.strip_prefix(root).unwrap_or(&path).to_path_buf());
        }
    }
    Ok(())
}

pub(super) fn relative_posix(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

pub(super) fn display_relative(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub(super) fn command_stdout(
    directory: &Path,
    executable: &OsStr,
    args: &[&str],
    input: Option<&str>,
) -> Result<String, String> {
    let mut command = Command::new(executable);
    command
        .args(args)
        .current_dir(directory)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if input.is_some() {
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("无法启动 {}：{error}", executable.to_string_lossy()))?;
    if let Some(input) = input {
        child
            .stdin
            .take()
            .ok_or_else(|| "无法打开子进程标准输入".to_owned())?
            .write_all(input.as_bytes())
            .map_err(|error| format!("无法写入子进程标准输入：{error}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("无法等待 {}：{error}", executable.to_string_lossy()))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if !output.status.success() {
        return Err(format!(
            "{} 执行失败（exit={}）：{}",
            executable.to_string_lossy(),
            output.status.code().unwrap_or(1),
            if stderr.is_empty() {
                stdout.trim()
            } else {
                &stderr
            }
        ));
    }
    Ok(stdout)
}

pub(super) fn is_rust_file(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "rs")
}
