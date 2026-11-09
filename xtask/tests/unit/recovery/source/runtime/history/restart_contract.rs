use super::*;
use crate::recovery::source::evidence::staging;
use std::{fs, process::Command};

struct ProducerFixture {
    _repository: tempfile::TempDir,
    record: Fixture,
}

impl ProducerFixture {
    fn new(mode: &str) -> Self {
        let original = crate::workspace::root_dir();
        let local = original.join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let repository = tempfile::Builder::new()
            .prefix("historical-producer-git-")
            .tempdir_in(local)
            .unwrap();
        let backend = repository.path();
        git(backend, &["init", "--quiet", "--object-format=sha1"]);
        let files = if mode == "old" {
            [
                (
                    "devex_clone_storage_process.py",
                    Some("c695960fd7d165f039aa2046902c3aa2972fb803"),
                ),
                (
                    "devex_clone_target_storage.py",
                    Some("368d95c120c82967620a5f9431472b10a3d51875"),
                ),
                (
                    "devex_clone_target_evidence.py",
                    Some("2a7359740d9f84fb78afd10b861e4d58b8b40b29"),
                ),
            ]
        } else {
            [
                (
                    "devex_clone_storage_process.py",
                    Some("16684182d9f03e3ee6be7bb4b7c10ece435de77d"),
                ),
                ("devex_clone_target_storage.py", None),
                (
                    "devex_clone_target_evidence.py",
                    Some("65055d21c98ce322926190227353c672733779a8"),
                ),
            ]
        };
        fs::create_dir_all(backend.join("tools/python")).unwrap();
        for (name, object) in files {
            if let Some(object) = object {
                let relative = format!("tools/python/{name}");
                fs::write(
                    backend.join(&relative),
                    producer_bytes(&original, object, &relative),
                )
                .unwrap();
            }
        }
        if mode == "mixed" {
            fs::write(
                backend.join("tools/python/devex_clone_target_evidence.py"),
                producer_bytes(
                    &original,
                    "2a7359740d9f84fb78afd10b861e4d58b8b40b29",
                    "tools/python/devex_clone_target_evidence.py",
                ),
            )
            .unwrap();
        }
        if mode == "missing" {
            fs::remove_file(backend.join("tools/python/devex_clone_storage_process.py")).unwrap();
        }
        git(backend, &["add", "tools"]);
        git(
            backend,
            &[
                "commit",
                "--quiet",
                "-m",
                "test(source): 固定真实生产者对象",
            ],
        );
        let head = git(backend, &["rev-parse", "HEAD"]);
        let sources = json!({"snapshot":{"head":head.trim(),"clean":true,"files":[],
            "patch_sha256":crate::contract::sha256_hex(b"")},
            "worktree_fingerprint":format!("sha256:{}","c".repeat(64)),
            "fingerprints":{"product":{"files":0,"sha256":"b".repeat(64)},
                "test_tools":{"files":0,"sha256":"b".repeat(64)},
                "support":{"files":0,"sha256":"b".repeat(64)}}});
        let error = if mode == "old" {
            "CalledProcessError"
        } else {
            "ValueError"
        };
        let record = Fixture::registered(backend.to_path_buf(), sources, error);
        let fixture = Self {
            _repository: repository,
            record,
        };
        fixture.failure(error, mode == "old");
        fixture
    }

    fn failure(&self, error: &str, old_frames: bool) {
        // 固定预期独立于 selector；不读取生产合同常量生成输入。
        let last = if old_frames {
            (
                "tools/python/devex_clone_target_storage.py",
                "actual_windows_argv",
            )
        } else {
            ("tools/python/devex_clone_target_evidence.py", "evidence")
        };
        let calls = [
            ("tools/python/devex_clone_run.py", "execute"),
            ("tools/python/devex_clone_storage.py", "execute_storage"),
            ("tools/python/devex_clone_storage_process.py", "start"),
            (
                "tools/python/devex_clone_storage_process.py",
                "actual_arguments",
            ),
            last,
        ];
        let frames = calls
            .iter()
            .enumerate()
            .map(
                |(index, (file, function))| json!({"file":file,"function":function,"line":index+1}),
            )
            .collect::<Vec<_>>();
        let run = self.record.directory.path();
        document::write_replace(&run.join("failure-0001.json"), &json!({
            "format_version":1,"kind":"devex-stage-failure","attempt":1,
            "stage":"storage-target","mode":"restart","error_type":error,
            "controller":common::binding(&state::controller_path(run,1)).unwrap(),"frames":frames,
        })).unwrap();
    }

    fn verify(&self) -> Result<(Value, Value)> {
        restart_incident(
            &self.record.backend,
            self.record.directory.path(),
            &self.record.row,
        )
    }
}

fn git(root: &Path, args: &[&str]) -> String {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "core.autocrlf=false",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=disabled-hooks",
            "-c",
            "user.name=RyFrame测试",
            "-c",
            "user.email=fixture@invalid",
        ])
        .args(args);
    for (name, _) in std::env::vars_os() {
        if name
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
        {
            command.env_remove(name);
        }
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "隔离 producer Git 操作失败：{args:?}"
    );
    String::from_utf8(output.stdout).unwrap()
}

fn producer_bytes(root: &Path, object: &str, relative: &str) -> Vec<u8> {
    if let Ok(bytes) = staging::git(root, &["cat-file", "blob", object]) {
        return bytes;
    }
    // 新 producer 尚未提交时，只允许组合候选中与冻结 blob 精确相同的真实文件。
    let observed = staging::git(root, &["hash-object", "--no-filters", "--", relative]).unwrap();
    assert_eq!(
        String::from_utf8(observed).unwrap().trim(),
        object,
        "缺少历史对象且当前文件不是冻结 producer；须先装配对应组合候选"
    );
    fs::read(root.join(relative)).unwrap()
}

#[test]
fn historical_restart_selects_real_old_and_native_git_producers_without_checkout_guessing() {
    for mode in ["old", "native"] {
        let fixture = ProducerFixture::new(mode);
        // 当前 checkout 故意漂移；选择只能读取已绑定 commit 内的对象。
        fs::write(
            fixture
                .record
                .backend
                .join("tools/python/devex_clone_storage_process.py"),
            b"checkout drift\n",
        )
        .unwrap();
        let observed = fixture.verify().unwrap();
        assert_eq!(
            observed.0,
            common::binding(&fixture.record.directory.path().join("failure-0001.json")).unwrap()
        );
    }
}

#[test]
fn historical_restart_rejects_crossed_error_frames_mixed_objects_and_missing_sources() {
    for mode in ["old", "native"] {
        let mut fixture = ProducerFixture::new(mode);
        let original = fixture.record.row["error_type"].clone();
        fixture.failure(original.as_str().unwrap(), mode != "old");
        assert!(fixture.verify().is_err(), "未拒绝交叉调用栈：{mode}");
        let crossed = if mode == "old" {
            "ValueError"
        } else {
            "CalledProcessError"
        };
        fixture.record.row["error_type"] = json!(crossed);
        fixture.failure(crossed, mode == "old");
        assert!(fixture.verify().is_err(), "未拒绝交叉错误类型：{mode}");
        fixture.record.row["error_type"] = original;
        fixture.failure(
            fixture.record.row["error_type"].as_str().unwrap(),
            mode == "old",
        );
        let head = fixture.record.row["sources"]["snapshot"]["head"]
            .as_str()
            .unwrap();
        let object = fixture
            .record
            .backend
            .join(".git/objects")
            .join(&head[..2])
            .join(&head[2..]);
        // Git 对象在 Windows 上可为只读；移动隔离夹具中的对象即可模拟缺失。
        fs::rename(&object, object.with_extension("unavailable")).unwrap();
        assert!(fixture.verify().is_err(), "未拒绝不可读提交：{mode}");
    }
    for mode in ["mixed", "missing"] {
        assert!(
            ProducerFixture::new(mode).verify().is_err(),
            "未拒绝未知组合：{mode}"
        );
    }
}
