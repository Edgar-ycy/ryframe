use super::*;

fn fixture() -> tempfile::TempDir {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join(".local-tests/consumer-file-tests");
    fs::create_dir_all(&root).unwrap();
    tempfile::tempdir_in(root).unwrap()
}

#[test]
fn identical_bytes_cannot_authorize_a_replaced_file_identity() {
    let root = fixture();
    let path = root.path().join("bound.json");
    let archived = root.path().join("owned.json");
    fs::write(&path, b"identical bytes").unwrap();
    let held = HeldFile::open(&path).unwrap();
    match fs::rename(&path, &archived) {
        Ok(()) => {
            fs::write(&path, b"identical bytes").unwrap();
            assert!(held.verify().is_err());
            assert_eq!(fs::read(&path).unwrap(), b"identical bytes");
        }
        Err(error) => {
            #[cfg(not(windows))]
            panic!("当前平台应允许重命名已打开文件：{error}");
            #[cfg(windows)]
            {
                assert!(matches!(error.raw_os_error(), Some(5 | 32)));
                held.verify().unwrap();
            }
        }
    }
}

#[test]
fn absent_parent_is_observed_without_creating_it_and_parent_traversal_is_rejected() {
    let root = fixture();
    assert!(!exists(&root.path().join("absent/also-absent/file.json")).unwrap());
    assert!(!root.path().join("absent").exists());
    assert!(exists(&root.path().join("absent/../file.json")).is_err());
    assert!(absolute_directory(&root.path().join("absent/..")).is_err());
    assert!(write_new(&root.path().join("absent/../file.json"), b"x").is_err());
}

#[test]
fn dangling_parent_is_never_treated_as_optional_absence() {
    let root = fixture();
    let target = root.path().join("missing-target");
    let link = root.path().join("dangling");
    fs::create_dir(&target).unwrap();
    link_directory(&link, &target);
    assert_eq!(target.parent(), Some(root.path()));
    assert!(fs::read_dir(&target).unwrap().next().is_none());
    fs::remove_dir(&target).unwrap();
    assert!(fs::symlink_metadata(&link).is_ok());
    assert!(exists(&link.join("config.json")).is_err());
    assert!(directory(&link.join("child")).is_err());
    assert!(!target.exists());
}

pub(super) fn link_directory(link: &Path, target: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).unwrap();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let link = link.to_str().unwrap().replace('/', "\\");
        let target = target.to_str().unwrap().replace('/', "\\");
        let result = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .creation_flags(0x0800_0000)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "创建测试 junction 失败：status={:?}, link={}, target={}\nstdout={}\nstderr={}\nstdout_bytes={:?}\nstderr_bytes={:?}",
            result.status,
            link,
            target,
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr),
            result.stdout,
            result.stderr
        );
    }
}
