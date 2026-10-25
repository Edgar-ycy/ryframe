//! 工具共用的版本阶段判断。只读 Workspace 版本与受信 Git 历史，不连接网络。

use std::{
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

use serde::Serialize;

/// Workspace 当前版本及其是否已进入稳定版保护阶段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReleaseStage {
    pub version: String,
    pub stable: bool,
}

/// 读取并校验 Workspace 的 SemVer 版本。
pub fn workspace_version(content: &str) -> Result<String, String> {
    let document = toml::from_str::<toml::Value>(content)
        .map_err(|error| format!("Cargo Workspace 清单无法解析：{error}"))?;
    let version = document
        .get("workspace")
        .and_then(|value| value.get("package"))
        .and_then(|value| value.get("version"))
        .and_then(toml::Value::as_str)
        .ok_or_else(|| "Cargo Workspace 缺少有效的 package.version".to_owned())?;
    stable_version(version)?;
    Ok(version.to_owned())
}

/// 判断合法 SemVer 版本是否已经进入 `1.x` 及以上的稳定阶段。
pub fn stable_version(version: &str) -> Result<bool, String> {
    let split = version
        .char_indices()
        .find_map(|(index, character)| matches!(character, '-' | '+').then_some(index));
    let (core, suffix) = split.map_or((version, None), |index| {
        (&version[..index], Some(&version[index + 1..]))
    });
    let components = core.split('.').collect::<Vec<_>>();
    if components.len() != 3
        || components
            .iter()
            .any(|component| !valid_numeric_component(component))
        || suffix.is_some_and(|value| {
            value.is_empty()
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
        })
    {
        return Err("版本必须使用 SemVer".to_owned());
    }
    Ok(components[0] != "0")
}

fn valid_numeric_component(value: &str) -> bool {
    value.bytes().all(|byte| byte.is_ascii_digit())
        && (value == "0" || !value.starts_with('0'))
        && !value.is_empty()
}

/// 读取当前版本阶段，并阻止降低版本绕过已经生效的稳定版保护。
pub fn release_stage(root: &Path, trusted_ref: &str) -> Result<ReleaseStage, String> {
    let manifest =
        std::fs::read_to_string(root.join("Cargo.toml")).map_err(|error| error.to_string())?;
    evaluate_release_stage(
        &manifest,
        trusted_ref,
        || git(root, &["rev-parse", "--is-shallow-repository"]),
        || git(root, &["tag", "--list"]),
        || historical_manifests(root, trusted_ref),
    )
}

fn evaluate_release_stage(
    manifest: &str,
    trusted_ref: &str,
    shallow_repository: impl FnOnce() -> Result<String, String>,
    tags: impl FnOnce() -> Result<String, String>,
    historical_manifests: impl FnOnce() -> Result<Vec<String>, String>,
) -> Result<ReleaseStage, String> {
    let version = workspace_version(manifest)?;
    let stable = stable_version(&version)?;
    if !valid_git_ref(trusted_ref) {
        return Err("受信 Git 引用无效".to_owned());
    }
    if !stable {
        let shallow = shallow_repository()?;
        if shallow.trim() != "false" {
            return Err("稳定版保护需要完整 Git 历史，浅克隆不能证明仍处于开发阶段".to_owned());
        }
        let mut protected = tags()?.lines().any(stable_release_tag);
        if !protected {
            for historical in historical_manifests()? {
                let Ok(historical_version) = workspace_version(&historical) else {
                    continue;
                };
                if stable_version(&historical_version)? {
                    protected = true;
                    break;
                }
            }
        }
        if protected {
            return Err("稳定版保护已生效，禁止降低 Workspace 版本绕过契约或迁移保护".to_owned());
        }
    }
    Ok(ReleaseStage { version, stable })
}

fn valid_git_ref(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && bytes
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'/' | b'-'))
}

fn stable_release_tag(value: &str) -> bool {
    let Some(version) = value.strip_prefix('v') else {
        return false;
    };
    let components = version.split('.').collect::<Vec<_>>();
    components.len() == 3
        && components[0]
            .bytes()
            .next()
            .is_some_and(|byte| matches!(byte, b'1'..=b'9'))
        && components.iter().all(|component| {
            !component.is_empty() && component.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn git(root: &Path, arguments: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .map_err(|error| format!("无法读取稳定版保护历史：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "无法读取稳定版保护历史：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|error| format!("稳定版保护历史不是 UTF-8：{error}"))
}

fn historical_manifests(root: &Path, trusted_ref: &str) -> Result<Vec<String>, String> {
    let revisions = git(
        root,
        &["log", trusted_ref, "--format=%H", "--", "Cargo.toml"],
    )?
    .lines()
    .filter(|line| !line.is_empty())
    .map(str::to_owned)
    .collect::<Vec<_>>();
    if revisions.is_empty() {
        return Ok(Vec::new());
    }
    let mut child = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("无法读取稳定版保护历史：{error}"))?;
    let input = revisions
        .iter()
        .map(|revision| format!("{revision}:Cargo.toml\n"))
        .collect::<String>();
    child
        .stdin
        .take()
        .ok_or("无法写入稳定版保护历史查询")?
        .write_all(input.as_bytes())
        .map_err(|error| format!("无法写入稳定版保护历史查询：{error}"))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("无法读取稳定版保护历史：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "无法读取稳定版保护历史：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    parse_cat_file_batch(&output.stdout, revisions.len())
}

fn parse_cat_file_batch(stream: &[u8], count: usize) -> Result<Vec<String>, String> {
    let mut cursor = 0;
    let mut manifests = Vec::with_capacity(count);
    for _ in 0..count {
        let header_end = stream[cursor..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map(|offset| cursor + offset)
            .ok_or_else(|| "稳定版保护历史不完整".to_owned())?;
        let header = std::str::from_utf8(&stream[cursor..header_end])
            .map_err(|_| "稳定版保护历史无效".to_owned())?;
        let fields = header.split_ascii_whitespace().collect::<Vec<_>>();
        if fields.len() != 3 || fields[1] != "blob" {
            return Err("稳定版保护历史中的 Cargo.toml 无法读取".to_owned());
        }
        let size = fields[2]
            .parse::<usize>()
            .map_err(|_| "稳定版保护历史中的 Cargo.toml 大小无效".to_owned())?;
        let content_start = header_end + 1;
        let content_end = content_start
            .checked_add(size)
            .filter(|end| *end < stream.len() && stream[*end] == b'\n')
            .ok_or_else(|| "稳定版保护历史不完整".to_owned())?;
        manifests.push(
            std::str::from_utf8(&stream[content_start..content_end])
                .map_err(|_| "稳定版保护历史中的 Cargo.toml 不是 UTF-8".to_owned())?
                .to_owned(),
        );
        cursor = content_end + 1;
    }
    if cursor != stream.len() {
        return Err("稳定版保护历史包含多余输出".to_owned());
    }
    Ok(manifests)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path, process::Command};

    use super::{evaluate_release_stage, release_stage, stable_version, workspace_version};

    struct TestRoot(tempfile::TempDir);

    impl TestRoot {
        fn new() -> Self {
            Self(crate::test_directory("release-stage-"))
        }

        fn path(&self) -> &Path {
            self.0.path()
        }

        fn version(&self, value: &str) {
            fs::write(
                self.path().join("Cargo.toml"),
                format!("[workspace.package]\nversion = \"{value}\"\n"),
            )
            .unwrap();
        }

        fn git(&self, arguments: &[&str]) {
            let result = Command::new("git")
                .arg("-C")
                .arg(self.path())
                .args(arguments)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }

        fn commit(&self, message: &str) -> String {
            self.git(&["add", "Cargo.toml"]);
            self.git(&[
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-m",
                message,
            ]);
            String::from_utf8(
                Command::new("git")
                    .arg("-C")
                    .arg(self.path())
                    .args(["rev-parse", "HEAD"])
                    .output()
                    .unwrap()
                    .stdout,
            )
            .unwrap()
            .trim()
            .to_owned()
        }
    }

    #[test]
    fn shallow_history_cannot_disable_stable_protection() {
        let manifest = "[workspace.package]\nversion = \"0.12.1\"\n";
        let error = evaluate_release_stage(
            manifest,
            "HEAD",
            || Ok("true\n".to_owned()),
            || panic!("浅克隆必须在读取 tags 前失败"),
            || panic!("浅克隆必须在读取历史清单前失败"),
        )
        .unwrap_err();
        assert!(error.contains("浅克隆"));

        let stage = evaluate_release_stage(
            "[workspace.package]\nversion = \"1.0.0\"\n",
            "HEAD",
            || panic!("稳定阶段不需要降级豁免"),
            || panic!("稳定阶段不需要读取 tags"),
            || panic!("稳定阶段不需要读取历史清单"),
        )
        .unwrap();
        assert!(stage.stable);
    }

    #[test]
    fn version_requires_workspace_semver() {
        assert!(!stable_version("0.99.0").unwrap());
        assert!(stable_version("1.0.0").unwrap());
        assert!(stable_version("2.0.0-rc.1").unwrap());
        for value in ["[workspace]", "[workspace.package]\nversion = \"latest\""] {
            assert!(workspace_version(value).is_err());
        }
        for value in ["01.0.0", "1.0", " 1.0.0", "1.0.0-", "1.0.0-rc_1"] {
            assert!(stable_version(value).is_err(), "{value}");
        }
    }

    #[test]
    fn stable_release_tags_keep_the_original_major_boundary() {
        let error = evaluate_release_stage(
            "[workspace.package]\nversion = \"0.12.1\"\n",
            "HEAD",
            || Ok("false\n".to_owned()),
            || Ok("v1.01.0\n".to_owned()),
            || Ok(Vec::new()),
        )
        .unwrap_err();
        assert!(error.contains("禁止降低"));

        let stage = evaluate_release_stage(
            "[workspace.package]\nversion = \"0.12.1\"\n",
            "HEAD",
            || Ok("false\n".to_owned()),
            || Ok("v0.99.0\n".to_owned()),
            || Ok(Vec::new()),
        )
        .unwrap();
        assert!(!stage.stable);
    }

    #[test]
    fn downgrade_is_rejected_after_commit_or_tag() {
        let root = TestRoot::new();
        root.git(&["init"]);
        root.version("0.12.1");
        let baseline = root.commit("development baseline");
        assert!(!release_stage(root.path(), "HEAD").unwrap().stable);

        root.version("1.0.0");
        assert!(release_stage(root.path(), "HEAD").unwrap().stable);
        root.commit("stable baseline");

        root.version("0.12.2");
        assert!(
            release_stage(root.path(), "HEAD")
                .unwrap_err()
                .contains("禁止降低")
        );
        root.commit("downgrade attempt");
        assert!(
            release_stage(root.path(), "HEAD")
                .unwrap_err()
                .contains("禁止降低")
        );

        root.git(&["tag", "v1.0.0"]);
        assert!(
            release_stage(root.path(), &baseline)
                .unwrap_err()
                .contains("禁止降低")
        );
    }
}
