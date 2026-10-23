use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

use super::PrivateRuntimeInvocation;
use crate::{
    Result,
    process::{PreservedFailure, resolved_executable},
};

pub(super) fn run(root: &Path, invocation: &PrivateRuntimeInvocation) -> Result<()> {
    let local = root.join(".local-tests");
    fs::create_dir_all(&local)?;
    let mut request = tempfile::Builder::new()
        .prefix("runtime-request-")
        .suffix(".json")
        .tempfile_in(&local)?;
    let written = request
        .write_all(invocation.protocol.as_bytes())
        .and_then(|()| request.as_file().sync_all());
    let path = request.into_temp_path();
    let result = written
        .map_err(Into::into)
        .and_then(|()| execute(root, invocation, &path));
    path.close().map_err(|_| "恢复运行受管请求文件清理失败")?;
    result
}

fn execute(root: &Path, invocation: &PrivateRuntimeInvocation, path: &Path) -> Result<()> {
    let guard =
        crate::recovery::monitoring::execution::ProtectedInputs::open([path.to_path_buf()])?;
    let environment =
        crate::recovery::monitoring::config::safe_environment(&root.join(".local-tests"))?;
    let mut command = Command::new(resolved_executable(root, "python")?);
    command
        .args([
            "-X",
            "utf8",
            "-E",
            "-s",
            "-B",
            invocation.script,
            "--request-file",
        ])
        .arg(path)
        .current_dir(root)
        .env_clear()
        .envs(environment)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    // verify 的标准输入继续保留给权威上下文；请求路径与内容不写入公开日志。
    println!("→ python -X utf8 -E -s -B {}", invocation.script);
    let status = command.status()?;
    guard.assert_unchanged()?;
    if status.success() {
        Ok(())
    } else {
        Err(PreservedFailure::new("恢复运行阶段执行失败".into(), status.code()).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_request_lives_through_dispatch_and_cleans_success_failure_and_spawn_error() {
        let local = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let fixture = tempfile::Builder::new()
            .prefix("runtime-protocol-test-")
            .tempdir_in(local)
            .unwrap();
        let root = fixture.path();
        let script = root.join("probe.py");
        fs::write(&script, r#"import json, os, sys
from pathlib import Path
assert sys.argv[1] == '--request-file' and len(sys.argv) == 3
path = Path(sys.argv[2])
assert path.is_absolute() and path.parent == Path.cwd() / '.local-tests'
assert json.loads(path.read_text(encoding='utf-8')) == {'private': 'file-only'}
assert not any(key.upper().startswith(('APP_', 'RYFRAME_')) or key.upper() == 'MYSQL_PWD' for key in os.environ)
assert sys.flags.utf8_mode == 1 and sys.flags.dont_write_bytecode == 1
"#).unwrap();
        let invocation = PrivateRuntimeInvocation {
            script: "probe.py",
            protocol: r#"{"private":"file-only"}"#.into(),
        };
        run(root, &invocation).unwrap();
        assert_eq!(fs::read_dir(root.join(".local-tests")).unwrap().count(), 0);
        fs::write(&script, "raise SystemExit(7)\n").unwrap();
        let error = run(root, &invocation).unwrap_err();
        assert_eq!(crate::process::failure_exit_code(error.as_ref()), Some(7));
        assert_eq!(fs::read_dir(root.join(".local-tests")).unwrap().count(), 0);
        fs::create_dir(root.join("config")).unwrap();
        fs::write(
            root.join("config/tools.toml"),
            "[verification]\npython_program = 'missing-client.exe'\n",
        )
        .unwrap();
        assert!(run(root, &invocation).is_err());
        assert_eq!(fs::read_dir(root.join(".local-tests")).unwrap().count(), 0);
    }
}
