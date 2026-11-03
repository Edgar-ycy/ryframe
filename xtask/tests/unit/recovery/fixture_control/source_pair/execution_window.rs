use super::super::{Window, window};
use super::{Fixture, fs, json};
use std::{io::ErrorKind, path::Path};

fn protected_error(error: &std::io::Error) {
    assert!(
        error.kind() == ErrorKind::PermissionDenied
            || (cfg!(windows) && matches!(error.raw_os_error(), Some(32 | 33))),
        "受保护收据替换出现非预期错误：{error}; kind={:?}, raw_os_error={:?}",
        error.kind(),
        error.raw_os_error()
    );
}

#[test]
fn execution_window_preserves_publication_and_rejects_each_source_root_drift() {
    let fixture = Fixture::new();
    let publication = fixture.publication();
    let path = fixture.output.join("execution-source.json");
    let original = fs::read(&path).unwrap();
    let opened = Window::open(&fixture.backend, &path).unwrap();
    assert_eq!(opened.publication(), &publication);
    opened.verify().unwrap();
    let descriptor = opened.binding().clone();
    drop(opened);
    assert_eq!(
        super::super::verify(&fixture.backend, &descriptor).unwrap(),
        publication
    );
    let root = Path::new(
        publication["published"]["backend"]["path"]
            .as_str()
            .unwrap(),
    );
    let pair = super::super::pair(&fixture.backend, root, &descriptor).unwrap();
    assert_eq!(
        pair,
        json!({"format_version":1,"kind":super::super::PAIR_KIND,
        "execution_source":descriptor,"sources":publication["original"]["source_pair"]["sources"]})
    );
    assert_eq!(super::super::verify_pair(root, &pair).unwrap(), publication);
    for (changed, label) in [
        (fixture.backend.join("src/lib.rs"), "生成来源 backend"),
        (
            fixture.frontend.join("src/business.txt"),
            "生成来源 frontend",
        ),
        (
            fixture.output.join("backend/src/lib.rs"),
            "发布来源 backend",
        ),
        (
            fixture.output.join("frontend/src/business.txt"),
            "发布来源 frontend",
        ),
    ] {
        let before = fs::read(&changed).unwrap();
        let window = Window::open(&fixture.backend, &path).unwrap();
        fs::write(&changed, "// 窗口内源码漂移\n").unwrap();
        let message = window.verify().unwrap_err().to_string();
        assert!(message.contains(label), "{message}");
        drop(window);
        fs::write(changed, before).unwrap();
    }
    assert_eq!(fs::read(path).unwrap(), original);
    assert_eq!(fixture.verify().unwrap(), publication);
}

#[test]
fn execution_window_rejects_same_bytes_publication_and_fixture_replacement() {
    let fixture = Fixture::new();
    let publication = fixture.publication();
    let path = fixture.output.join("execution-source.json");
    for replaced in [&path, &fixture.receipt] {
        let before = fs::read(replaced).unwrap();
        let retained = replaced.with_extension("window-retained");
        let window = Window::open(&fixture.backend, &path).unwrap();
        let renamed = match fs::rename(replaced, &retained) {
            Ok(()) => {
                fs::write(replaced, &before).unwrap();
                assert!(window.verify().is_err());
                true
            }
            Err(error) => {
                protected_error(&error);
                assert_eq!(fs::read(replaced).unwrap(), before);
                window.verify().unwrap();
                false
            }
        };
        drop(window);
        if renamed {
            fs::remove_file(replaced).unwrap();
            fs::rename(retained, replaced).unwrap();
        }
        assert_eq!(fs::read(replaced).unwrap(), before);
    }
    assert_eq!(fixture.verify().unwrap(), publication);
}

#[test]
fn execution_window_keeps_business_failure_and_independent_root_postcheck_errors() {
    let fixture = Fixture::new();
    let publication = fixture.publication();
    let path = fixture.output.join("execution-source.json");
    let generated = fixture.backend.join("src/lib.rs");
    let published = fixture.output.join("frontend/src/business.txt");
    let generated_before = fs::read(&generated).unwrap();
    let published_before = fs::read(&published).unwrap();
    let opened = Window::open(&fixture.backend, &path).unwrap();
    fs::write(&generated, "// 生成根漂移\n").unwrap();
    fs::write(&published, "// 发布根漂移\n").unwrap();
    let body: crate::Result<()> = Err("合成业务读取失败".into());
    let after = opened.verify();
    let message = window::finish([body, after]).unwrap_err().to_string();
    assert!(message.contains("合成业务读取失败"), "{message}");
    assert!(message.contains("生成来源 backend"), "{message}");
    assert!(message.contains("发布来源 frontend"), "{message}");
    assert!(message.contains("执行来源后验同时失败"), "{message}");
    drop(opened);
    fs::write(&generated, generated_before).unwrap();
    fs::write(&published, published_before).unwrap();
    assert_eq!(fixture.verify().unwrap(), publication);
}
