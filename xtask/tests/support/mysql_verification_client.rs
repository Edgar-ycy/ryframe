//! 固定 MySQL 查询和进程用例共用的本地客户端夹具。
use std::{
    cell::{Cell, RefCell},
    fs::File,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

use serde_json::Value;

use crate::recovery::clone::native::{
    export::transport::{
        CommandOutput, CommandSpec, StderrDigest, StdoutTarget, SystemTransport, Transport,
    },
    s3,
};

pub(crate) const MYSQL_VERIFICATION_STDERR: &[u8] = b"\xffmysql-verification-secret\n";

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
