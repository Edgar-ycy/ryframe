//! 固定 MySQL 查询和进程用例共用的本地客户端夹具。
use std::{
    cell::{Cell, RefCell},
    fs::{self, File},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use serde_json::Value;

use crate::recovery::clone::native::{
    export::transport::{
        CommandOutput, CommandSpec, StderrDigest, StdoutTarget, SystemTransport, Transport,
    },
    json as json_io,
    resources::common,
    s3,
};

pub(crate) const MYSQL_VERIFICATION_STDERR: &[u8] = b"\xffmysql-verification-secret\n";

pub(crate) fn assert_mysql_ownership_retry(work: &Path, target_key: &str, second_stdout: &[u8]) {
    let receipts = fs::read_dir(work)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("mysql-ownership-") && name.ends_with(".json"))
        })
        .collect::<Vec<_>>();
    assert_eq!(receipts.len(), 3);
    let relation = receipts
        .iter()
        .map(|path| json_io::read(path).unwrap())
        .find(|value| value["kind"] == "mysql-verification-retry")
        .unwrap();
    assert_eq!(relation["check"], "ownership");
    assert_eq!(relation["target_key"], target_key);
    assert_eq!(
        relation["reason"],
        "first_returncode_zero_stdout_stderr_exactly_empty"
    );
    assert_ne!(relation["first"]["path"], relation["second"]["path"]);
    let mut stdout_paths = Vec::new();
    for (slot, raw) in [("first", b"".as_slice()), ("second", second_stdout)] {
        let receipt = &relation[slot];
        let path = Path::new(receipt["path"].as_str().unwrap());
        assert!(receipts.iter().any(|record| record.as_path() == path));
        let actual = common::binding(path).unwrap();
        assert_eq!(receipt["bytes"], actual["bytes"]);
        assert_eq!(receipt["sha256"], actual["sha256"]);
        let output = json_io::read(path).unwrap();
        assert_eq!(output["kind"], "mysql-verification-output");
        assert_eq!(output["check"], "ownership");
        assert_eq!(output["target_key"], target_key);
        assert_eq!(output["returncode"], 0);
        assert!(output["error_type"].is_null());
        assert_eq!(output["stderr_bytes"], 0);
        let stdout = PathBuf::from(output["stdout_file"]["path"].as_str().unwrap());
        assert_eq!(stdout.parent(), Some(work));
        assert_eq!(fs::read(&stdout).unwrap(), raw);
        let actual = common::binding(&stdout).unwrap();
        assert_eq!(output["stdout_file"]["bytes"], actual["bytes"]);
        assert_eq!(output["stdout_file"]["sha256"], actual["sha256"]);
        stdout_paths.push(stdout);
    }
    assert_ne!(stdout_paths[0], stdout_paths[1]);
}

pub(crate) fn compile_mysql_verification_client(directory: &Path) -> PathBuf {
    let executable = directory.join(binary_name("mysql-verification-stderr"));
    compile_client(&executable);
    executable
}

pub(crate) const MYSQL_TIMEOUT_PARTIAL_STDOUT: &[u8] = b"partial-private-mysql\n";

pub(crate) fn compile_mysql_timeout_client(directory: &Path) -> PathBuf {
    let executable = directory.join(binary_name("mysql-verification-timeout"));
    compile_client(&executable);
    executable
}

#[derive(Default)]
pub(crate) struct MysqlTimeoutTransport {
    pub(crate) calls: Cell<usize>,
    pub(crate) writer: RefCell<Option<File>>,
}

impl Transport for MysqlTimeoutTransport {
    fn run(&self, _: CommandSpec) -> crate::Result<CommandOutput> {
        panic!("超时夹具只允许固定 MySQL 查询");
    }

    fn run_mysql_verification(
        &self,
        mut spec: CommandSpec,
    ) -> crate::Result<(CommandOutput, StderrDigest)> {
        self.calls.set(self.calls.get() + 1);
        assert_eq!(spec.timeout, Duration::from_secs(1800));
        let StdoutTarget::HeldFile { file, .. } = &spec.stdout else {
            panic!("必须使用已持有的 MySQL stdout 句柄");
        };
        *self.writer.borrow_mut() = Some(file.try_clone()?);
        spec.timeout = Duration::from_secs(3);
        SystemTransport.run_mysql_verification(spec)
    }

    fn s3(&self, _: s3::Request) -> crate::Result<Value> {
        panic!("超时夹具不允许访问对象存储");
    }
}

pub(crate) fn compile_client(executable: &Path) {
    let source = crate::workspace::root_dir().join("xtask/tests/fixtures/job_mysql_client.rs");
    let mut command = Command::new("rustc");
    command
        .args([
            "--edition",
            "2024",
            "--crate-name",
            "job_mysql_fixture",
            "-C",
            "debuginfo=0",
        ])
        .arg(&source)
        .arg("-o")
        .arg(executable);
    if cfg!(windows) {
        command.args(["-C", "linker=rust-lld.exe"]);
    }
    let output = command.output().expect("应编译独立原生客户端夹具");
    assert!(
        output.status.success(),
        "原生客户端夹具编译失败：{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

pub(crate) fn binary_name(stem: &str) -> String {
    format!("{stem}{}", std::env::consts::EXE_SUFFIX)
}
