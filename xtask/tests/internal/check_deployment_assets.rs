use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use super::{
    check::{TaskExecutor, deployment_assets},
    process::failure_exit_code,
};

#[test]
fn current_deployment_assets_pass_without_writing_controlled_files() {
    let root = super::workspace::root_dir();
    let before = deployment_snapshot(&root);
    let violations = deployment_assets::source_violations(&root).unwrap();
    assert!(violations.is_empty(), "{violations:#?}");
    assert_eq!(deployment_snapshot(&root), before);
    assert_eq!(
        TaskExecutor::DeploymentAssets.definition().id,
        "policy.deployment-assets"
    );
    assert!(
        TaskExecutor::DeploymentAssets
            .definition()
            .allowed_writes
            .is_empty()
    );
}

#[test]
fn rust_toolchain_versions_stay_aligned() {
    for (channel, minimum, image, expected) in [
        ("1.98.0", "1.98", "1.98.0", None),
        ("stable", "1.98", "1.98.0", Some("完整版本号")),
        ("1.98.0-beta.1", "1.98", "1.98.0", Some("完整版本号")),
        ("1.98.0", "1.97", "1.98.0", Some("主次版本一致")),
        ("1.98.0", "1.98", "1.97.1", Some("完全一致")),
        ("1.98.0", "1.98", "1.98.1", Some("完全一致")),
    ] {
        let source = format!("ARG RUST_IMAGE=rust:{image}-bookworm@sha256:abc\n");
        let violations = deployment_assets::rules::toolchain_violations(channel, minimum, &source);
        if let Some(expected) = expected {
            assert!(
                violations.iter().any(|item| item.contains(expected)),
                "{channel}/{minimum}/{image}: {violations:?}"
            );
        } else {
            assert!(violations.is_empty(), "{channel}/{minimum}/{image}");
        }
    }
}

#[test]
fn markdown_anchors_follow_headings_fences_and_duplicates() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("target/deployment-assets-tests");
    fs::create_dir_all(&root).unwrap();
    let temporary = tempfile::tempdir_in(root).unwrap();
    fs::create_dir(temporary.path().join("docs")).unwrap();
    let document = temporary.path().join("docs/runbook.md");
    fs::write(
        &document,
        "# Runbook\n\n```text\n# Hidden\n```\n\n## Runbook\n\n### 中文 标题\n",
    )
    .unwrap();
    let anchors = deployment_assets::rules::markdown_anchors(&document).unwrap();
    assert!(anchors.contains("runbook"));
    assert!(anchors.contains("runbook-1"));
    assert!(anchors.contains("中文-标题"));
    assert!(!anchors.contains("hidden"));
    let valid = "https://github.com/Edgar-ycy/ryframe/blob/main/docs/runbook.md#runbook-1";
    assert_eq!(
        deployment_assets::rules::validate_runbook_url(temporary.path(), valid).unwrap(),
        None
    );
    let missing = "https://github.com/Edgar-ycy/ryframe/blob/main/docs/runbook.md#missing-heading";
    assert!(
        deployment_assets::rules::validate_runbook_url(temporary.path(), missing)
            .unwrap()
            .unwrap()
            .contains("标题锚点不存在")
    );
}

#[test]
fn deployment_violations_preserve_the_original_failure_exit_code() {
    let error = deployment_assets::finish(vec!["模拟资产违规".to_owned()], "静态").unwrap_err();
    assert_eq!(failure_exit_code(error.as_ref()), Some(1));
}

#[test]
fn distroless_archive_passes_with_expected_executables() {
    let fixture = ArchiveFixture::new();
    write_archive(&fixture.path, &expected_entries());
    let violations = deployment_assets::archive_violations(&fixture.path).unwrap();
    assert!(violations.is_empty(), "{violations:#?}");
}

#[test]
fn archive_rejects_runtime_tools_product_tooling_and_generator_endpoints() {
    let fixture = ArchiveFixture::new();
    let mut entries = expected_entries();
    entries.push(("bin/sh".into(), b'0', 0o755, b"shell".to_vec()));
    entries.push((
        "opt/ryframe/ryframe-reset".into(),
        b'0',
        0o755,
        b"reset".to_vec(),
    ));
    entries.push((
        "usr/local/bin/ryframe".into(),
        b'0',
        0o755,
        b"/api/v1/tools/gen".to_vec(),
    ));
    write_archive(&fixture.path, &entries);
    let violations = deployment_assets::archive_violations(&fixture.path).unwrap();
    let text = violations.join("\n");
    assert!(text.contains("不得包含 shell"), "{text}");
    assert!(text.contains("generator/reset 可疑路径"), "{text}");
    assert!(text.contains("在线生成器端点"), "{text}");
}

#[test]
fn archive_rejects_missing_non_executable_or_linked_binaries() {
    let fixture = ArchiveFixture::new();
    let mut entries = expected_entries();
    entries.retain(|(name, _, _, _)| name != "usr/local/bin/ryframe-worker");
    entries.push((
        "usr/local/bin/ryframe-migrate".into(),
        b'2',
        0o777,
        Vec::new(),
    ));
    entries.push((
        "usr/local/bin/unexpected".into(),
        b'0',
        0o755,
        b"unexpected".to_vec(),
    ));
    write_archive(&fixture.path, &entries);
    let violations = deployment_assets::archive_violations(&fixture.path).unwrap();
    let text = violations.join("\n");
    assert!(text.contains("条目必须精确"), "{text}");
    assert!(text.contains("ryframe-migrate 必须"), "{text}");
}

fn expected_entries() -> Vec<(String, u8, u32, Vec<u8>)> {
    deployment_assets::EXPECTED_BINARIES
        .iter()
        .map(|name| {
            (
                format!("usr/local/bin/{name}"),
                b'0',
                0o755,
                b"product binary".to_vec(),
            )
        })
        .collect()
}

fn write_archive(path: &Path, entries: &[(String, u8, u32, Vec<u8>)]) {
    let mut archive = Vec::new();
    for (name, kind, mode, content) in entries {
        write_header(&mut archive, name, *kind, *mode, content.len());
        archive.extend_from_slice(content);
        let padding = (512 - content.len() % 512) % 512;
        archive.resize(archive.len() + padding, 0);
    }
    archive.resize(archive.len() + 1024, 0);
    fs::write(path, archive).unwrap();
}

fn write_header(archive: &mut Vec<u8>, name: &str, kind: u8, mode: u32, size: usize) {
    assert!(name.len() < 100);
    let mut header = [0_u8; 512];
    header[..name.len()].copy_from_slice(name.as_bytes());
    write_octal(&mut header[100..108], u64::from(mode));
    write_octal(&mut header[108..116], 0);
    write_octal(&mut header[116..124], 0);
    write_octal(&mut header[124..136], size as u64);
    write_octal(&mut header[136..148], 0);
    header[148..156].fill(b' ');
    header[156] = kind;
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let checksum = header.iter().map(|byte| u64::from(*byte)).sum::<u64>();
    let checksum = format!("{checksum:06o}");
    header[148..154].copy_from_slice(checksum.as_bytes());
    header[154] = b' ';
    header[155] = 0;
    archive.extend_from_slice(&header);
}

fn write_octal(field: &mut [u8], value: u64) {
    let width = field.len() - 1;
    let text = format!("{value:0width$o}", width = width);
    assert_eq!(text.len(), width);
    field[..width].copy_from_slice(text.as_bytes());
    field[width] = 0;
}

struct ArchiveFixture {
    _temporary: tempfile::TempDir,
    path: PathBuf,
}

impl ArchiveFixture {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("target/deployment-assets-tests");
        fs::create_dir_all(&root).unwrap();
        let temporary = tempfile::tempdir_in(root).unwrap();
        let path = temporary.path().join("filesystem.tar");
        Self {
            _temporary: temporary,
            path,
        }
    }
}

fn deployment_snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    for relative in [
        "Cargo.toml",
        "rust-toolchain.toml",
        "deploy",
        ".github/workflows",
        "tools/fixtures/deployment/deploy.env",
        "crates/ryframe-api/src",
    ] {
        collect_files(root, &root.join(relative), &mut result);
    }
    result
}

fn collect_files(root: &Path, path: &Path, result: &mut BTreeMap<PathBuf, Vec<u8>>) {
    if path.is_file() {
        result.insert(
            path.strip_prefix(root).unwrap().to_path_buf(),
            fs::read(path).unwrap(),
        );
        return;
    }
    for entry in fs::read_dir(path).unwrap() {
        collect_files(root, &entry.unwrap().path(), result);
    }
}
