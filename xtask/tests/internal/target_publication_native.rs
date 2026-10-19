use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::PathBuf,
};

use serde_json::json;

use crate::{recovery::clone::native::target_verify::publication, workspace};

struct Fixture {
    directory: tempfile::TempDir,
    target: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let parent = workspace::root_dir().join(".local-tests/target-publication");
        fs::create_dir_all(&parent).unwrap();
        let directory = tempfile::tempdir_in(parent).unwrap();
        fs::write(
            directory.path().join("unknown.bin"),
            "保留外部证据".as_bytes(),
        )
        .unwrap();
        let target = directory.path().join("verify.json");
        Self { directory, target }
    }

    fn files(&self) -> BTreeMap<PathBuf, Vec<u8>> {
        fs::read_dir(self.directory.path())
            .unwrap()
            .map(|entry| {
                let path = entry.unwrap().path();
                (path.file_name().unwrap().into(), fs::read(path).unwrap())
            })
            .collect()
    }
}

fn io_cause<'a>(error: &'a (dyn std::error::Error + 'static)) -> &'a io::Error {
    error.source().unwrap().downcast_ref::<io::Error>().unwrap()
}

#[test]
fn publication_success_contains_complete_json_and_one_lf() {
    let fixture = Fixture::new();
    let value = json!({"结果": "完整"});
    publication::publish(&fixture.target, &value).unwrap();
    let bytes = fs::read(&fixture.target).unwrap();
    assert_eq!(bytes, "{\n  \"结果\": \"完整\"\n}\n".as_bytes());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
        value
    );
    assert_eq!(fixture.files().len(), 2);
    assert_eq!(
        fs::read(fixture.directory.path().join("unknown.bin")).unwrap(),
        "保留外部证据".as_bytes()
    );
}

#[test]
fn publication_finishes_writing_and_sync_before_final_path_exists() {
    let fixture = Fixture::new();
    let events = RefCell::new(Vec::new());
    publication::publish_with(
        &fixture.target,
        &json!({"status": "complete"}),
        |file, bytes| {
            assert!(!fixture.target.exists());
            assert_eq!(fixture.files().len(), 2);
            assert!(bytes.ends_with(b"}\n"));
            serde_json::from_slice::<serde_json::Value>(bytes).unwrap();
            events.borrow_mut().push("write");
            file.write_all(bytes)
        },
        |file| {
            assert!(!fixture.target.exists());
            assert!(file.metadata()?.len() > 0);
            events.borrow_mut().push("sync");
            file.sync_all()
        },
    )
    .unwrap();
    assert_eq!(*events.borrow(), ["write", "sync"]);
    assert!(fixture.target.is_file());
    assert_eq!(fixture.files().len(), 2);
}

#[test]
fn publication_interrupted_write_only_removes_its_temporary_file() {
    let fixture = Fixture::new();
    let before = fixture.files();
    let synced = Cell::new(false);
    let error = publication::publish_with(
        &fixture.target,
        &json!({"status": "complete"}),
        |file, bytes| {
            file.write_all(&bytes[..7])?;
            assert_eq!(file.metadata()?.len(), 7);
            Err(io::Error::new(io::ErrorKind::Interrupted, "注入写入中断"))
        },
        |_| {
            synced.set(true);
            Ok(())
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("写入临时文件失败"));
    assert!(error.to_string().contains("注入写入中断"));
    assert_eq!(io_cause(error.as_ref()).kind(), io::ErrorKind::Interrupted);
    assert!(!synced.get());
    assert!(!fixture.target.exists());
    assert_eq!(fixture.files(), before);
}

#[test]
fn publication_failed_sync_only_removes_its_temporary_file() {
    let fixture = Fixture::new();
    let before = fixture.files();
    let written = Cell::new(0);
    let error = publication::publish_with(
        &fixture.target,
        &json!({"status": "complete"}),
        |file, bytes| {
            file.write_all(bytes)?;
            written.set(bytes.len());
            Ok(())
        },
        |file| {
            assert_eq!(file.metadata()?.len(), written.get() as u64);
            assert!(!fixture.target.exists());
            Err(io::Error::other("注入同步失败"))
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("同步临时文件失败"));
    assert!(error.to_string().contains("注入同步失败"));
    assert_eq!(io_cause(error.as_ref()).kind(), io::ErrorKind::Other);
    assert!(!fixture.target.exists());
    assert_eq!(fixture.files(), before);
}

#[test]
fn publication_existing_target_keeps_original_bytes_and_unknown_files() {
    let fixture = Fixture::new();
    fs::write(&fixture.target, b"existing\0receipt\n").unwrap();
    let before = fixture.files();
    let error = publication::publish(&fixture.target, &json!({"replacement": true})).unwrap_err();
    assert!(error.to_string().contains("发布最终文件失败"));
    assert_eq!(
        io_cause(error.as_ref()).kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(fixture.files(), before);
}

#[test]
fn publication_concurrent_target_creation_keeps_competing_bytes() {
    let fixture = Fixture::new();
    let competing = b"concurrent\0receipt\n";
    let mut expected = fixture.files();
    expected.insert(PathBuf::from("verify.json"), competing.to_vec());
    let error = publication::publish_with(
        &fixture.target,
        &json!({"replacement": true}),
        |file, bytes| file.write_all(bytes),
        |file| {
            file.sync_all()?;
            assert!(!fixture.target.exists());
            std::thread::scope(|scope| {
                scope
                    .spawn(|| {
                        OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .open(&fixture.target)?
                            .write_all(competing)
                    })
                    .join()
                    .unwrap()
            })
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("发布最终文件失败"));
    assert_eq!(
        io_cause(error.as_ref()).kind(),
        io::ErrorKind::AlreadyExists
    );
    assert_eq!(fixture.files(), expected);
}

#[test]
fn publication_missing_parent_reports_creation_stage_without_creating_directories() {
    let fixture = Fixture::new();
    let before = fixture.files();
    let path = fixture.directory.path().join("missing/verify.json");
    let error = publication::publish(&path, &json!({"status": "complete"})).unwrap_err();
    assert!(error.to_string().contains("创建同目录临时文件失败"));
    assert_eq!(io_cause(error.as_ref()).kind(), io::ErrorKind::NotFound);
    assert!(!path.exists());
    assert_eq!(fixture.files(), before);
}
