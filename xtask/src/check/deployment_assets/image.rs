use std::{
    fs::{self, File},
    path::Path,
    process::Stdio,
};

use serde_json::Value;

use super::{execution_error, regex};
use crate::{
    Result,
    process::{PreservedFailure, child_command},
};

pub(super) fn violations(
    root: &Path,
    image: &str,
    expected_commit: Option<&str>,
) -> Result<Vec<String>> {
    if !regex(r"^[A-Za-z0-9._/@:+-]+$").is_match(image) {
        return Err("镜像引用格式无效".into());
    }
    if expected_commit.is_some_and(|commit| !regex(r"^[0-9a-f]{40}$").is_match(commit)) {
        return Err("--expected-commit 必须是 40 位小写十六进制提交".into());
    }
    let inspected = docker_capture(root, &["image", "inspect", image])?;
    let document: Value = serde_json::from_slice(&inspected)
        .map_err(|error| execution_error("解析镜像配置", error))?;
    let document = document
        .as_array()
        .filter(|values| values.len() == 1)
        .ok_or_else(|| "docker image inspect 返回了意外结果".to_owned())?;
    let config = document[0].get("Config").and_then(Value::as_object);
    let mut violations = Vec::new();
    if config
        .and_then(|config| config.get("User"))
        .and_then(Value::as_str)
        != Some("10001:10001")
    {
        violations.push("生产镜像必须以固定的数值非 root 用户 10001:10001 运行".to_owned());
    }
    let entrypoint = config
        .and_then(|config| config.get("Entrypoint"))
        .and_then(Value::as_array)
        .and_then(|values| values.iter().map(Value::as_str).collect::<Option<Vec<_>>>());
    if entrypoint.as_deref() != Some(&["/usr/local/bin/ryframe"]) {
        violations.push("生产镜像入口必须固定为 /usr/local/bin/ryframe".to_owned());
    }
    if let Some(expected) = expected_commit {
        let revision = config
            .and_then(|config| config.get("Labels"))
            .and_then(|labels| labels.get("org.opencontainers.image.revision"))
            .and_then(Value::as_str);
        if revision != Some(expected) {
            violations.push("生产镜像 revision 标签与源码提交不一致".to_owned());
        }
    }
    let inspection_root = root.join("target/image-inspection");
    fs::create_dir_all(&inspection_root)?;
    let temporary = tempfile::Builder::new()
        .prefix("ryframe-image-inspect-")
        .tempdir_in(inspection_root)?;
    let archive = temporary.path().join("filesystem.tar");
    export_image_filesystem(root, image, &archive)?;
    violations.extend(super::archive_violations(&archive)?);
    Ok(violations)
}

fn docker_capture(root: &Path, arguments: &[&str]) -> Result<Vec<u8>> {
    let output = child_command("docker")
        .args(arguments)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| execution_error("执行 docker", error))?;
    if !output.status.success() {
        return Err(PreservedFailure::new(
            format!(
                "部署资产检查执行失败: docker {}: {}",
                arguments.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
            Some(1),
        )
        .into());
    }
    Ok(output.stdout)
}

fn export_image_filesystem(root: &Path, image: &str, target: &Path) -> Result<()> {
    let created = docker_capture(root, &["create", image])?;
    let container_id = String::from_utf8(created)
        .map_err(|_| "docker create 输出不是 UTF-8".to_owned())?
        .trim()
        .to_owned();
    if !regex(r"^[0-9a-f]{12,64}$").is_match(&container_id) {
        return Err("docker create 未返回合法容器 ID".into());
    }
    let result = export_container(root, &container_id, target);
    let _ = child_command("docker")
        .args(["rm", "--force", &container_id])
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .output();
    result
}

fn export_container(root: &Path, container_id: &str, target: &Path) -> Result<()> {
    let output = File::create(target)?;
    let status = child_command("docker")
        .args(["export", container_id])
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::from(output))
        .stderr(Stdio::inherit())
        .status()
        .map_err(|error| execution_error("执行 docker export", error))?;
    if !status.success() {
        return Err(PreservedFailure::new(
            format!("部署资产检查执行失败: docker export 退出状态为 {status}"),
            Some(1),
        )
        .into());
    }
    Ok(())
}
