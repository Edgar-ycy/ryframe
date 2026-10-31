//! 复制后 pacing 只消费显式请求的文件绑定，不推断前端拓扑或创建预算状态。
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::{
    Result,
    local_test_path::{
        LocalTestPathKind, strip_local_test_prefix, validate_absolute_path,
        validate_local_test_path,
    },
    recovery::clone::native::{json as document, target_verify::input::request::external_file},
};

pub(crate) fn request(args: &Value) -> Result<Value> {
    document::exact(
        args,
        &["backend", "directory", "frontend_root", "pacing"],
        "复制后 pacing 绑定参数",
    )?;
    let backend = path(&args["backend"])?;
    let directory = path(&args["directory"])?;
    let frontend = path(&args["frontend_root"])?;
    let pacing = &args["pacing"];
    document::exact(
        pacing,
        &[
            "contract",
            "bindings",
            "python_sha256",
            "login_budget_module_sha256",
        ],
        "复制后 pacing",
    )?;
    // 这里只核验容器类型；业务间隔、authority 和 catalog 仍由原 session 阶段校验。
    document::object(&pacing["contract"], "复制后 pacing contract")?;
    document::exact(
        &pacing["bindings"],
        &["python", "login_budget_state"],
        "复制后 pacing 文件绑定",
    )?;
    let state = path(&pacing["bindings"]["login_budget_state"])?;
    let python = path(&pacing["bindings"]["python"])?;
    let module = frontend.join("scripts/browser-login-budget.mjs");
    for _ in 0..2 {
        verify_paths(&backend, &directory, &frontend, &state)?;
        external_file(&json!({"path":python,"sha256":pacing["python_sha256"]})).map_err(
            |_| "复制后 pacing Python 文件绑定无效或已漂移；核对显式路径与 SHA-256，不执行或重放",
        )?;
        external_file(&json!({"path":module,"sha256":pacing["login_budget_module_sha256"]}))
            .map_err(|_| "复制后 pacing 前端模块绑定无效或已漂移；核对显式 frontend_root 与 SHA-256，不执行或重放")?;
    }
    verify_paths(&backend, &directory, &frontend, &state)?;
    Ok(Value::Null)
}

fn path(value: &Value) -> Result<PathBuf> {
    let text = value
        .as_str()
        .filter(|text| !text.is_empty() && !text.chars().any(char::is_control))
        .ok_or("复制后 pacing 路径必须是非空文本且不能包含控制字符")?;
    Ok(PathBuf::from(text))
}

fn verify_paths(backend: &Path, directory: &Path, frontend: &Path, state: &Path) -> Result<()> {
    for path in [backend, directory, frontend] {
        validate_absolute_path(path, LocalTestPathKind::ExistingDirectory)
            .map_err(|_| "复制后 pacing 根必须是现有无链接绝对目录；核对显式请求，不创建目录")?;
    }
    validate_local_test_path(directory, backend, LocalTestPathKind::ExistingDirectory)
        .map_err(|_| "复制后 pacing run 必须位于当前后端 .local-tests 内且不能经过链接")?;
    let expected = directory.join("post-copy/login-budget.json");
    if strip_local_test_prefix(state, &expected).is_none_or(|suffix| !suffix.as_os_str().is_empty())
    {
        return Err("复制后 pacing 登录预算必须是当前 run 的 post-copy/login-budget.json".into());
    }
    let kind = if state
        .try_exists()
        .map_err(|_| "复制后 pacing 登录预算路径无法核验")?
    {
        LocalTestPathKind::ExistingFile
    } else {
        LocalTestPathKind::OutputFile
    };
    validate_local_test_path(state, backend, kind)
        .map_err(|_| "复制后 pacing 登录预算必须是无链接的受管文件位置；不创建或覆盖状态")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::clone::native::digest;
    use std::fs;
    use tempfile::TempDir;

    struct Fixture {
        _temporary: TempDir,
        root: PathBuf,
        backend: PathBuf,
        directory: PathBuf,
        frontend: PathBuf,
        python: PathBuf,
        module: PathBuf,
        args: Value,
    }

    impl Fixture {
        fn new() -> Self {
            let parent = crate::workspace::root_dir().join(".local-tests");
            fs::create_dir_all(&parent).unwrap();
            let temporary = tempfile::Builder::new()
                .prefix("copy-pacing-binding-")
                .tempdir_in(parent)
                .unwrap();
            let root = fs::canonicalize(temporary.path()).unwrap();
            let backend = root.join("backend");
            let directory = backend.join(".local-tests/run one");
            let frontend = root.join("frontends/独立 前端");
            let python = root.join("python.exe");
            let module = frontend.join("scripts/browser-login-budget.mjs");
            fs::create_dir_all(&directory).unwrap();
            fs::create_dir_all(module.parent().unwrap()).unwrap();
            fs::write(&python, b"python-fixture").unwrap();
            fs::write(&module, b"module-original").unwrap();
            let args = json!({"backend":backend,"directory":directory,"frontend_root":frontend,
                "pacing":{"contract":{},"bindings":{"python":python,
                    "login_budget_state":directory.join("post-copy/login-budget.json")},
                    "python_sha256":digest::file(&python).unwrap().sha256,
                    "login_budget_module_sha256":digest::file(&module).unwrap().sha256}});
            Self {
                _temporary: temporary,
                root,
                backend,
                directory,
                frontend,
                python,
                module,
                args,
            }
        }

        fn rejected(&self, args: &Value) {
            let error = request(args).unwrap_err().to_string();
            assert!(!error.contains("private-password-token"), "{error}");
            assert!(!self.directory.join("post-copy").exists());
        }
    }

    #[test]
    fn explicit_nonadjacent_frontend_is_read_only_with_absent_or_existing_budget() {
        let fixture = Fixture::new();
        assert_ne!(
            fixture.frontend,
            fixture.backend.parent().unwrap().join("ryframe-vue3")
        );
        assert_eq!(request(&fixture.args).unwrap(), Value::Null);
        assert!(!fixture.directory.join("post-copy").exists());
        let state = fixture.directory.join("post-copy/login-budget.json");
        fs::create_dir(state.parent().unwrap()).unwrap();
        fs::write(&state, b"existing-budget").unwrap();
        assert_eq!(request(&fixture.args).unwrap(), Value::Null);
        assert_eq!(fs::read(&state).unwrap(), b"existing-budget");
        assert_eq!(fs::read_dir(state.parent().unwrap()).unwrap().count(), 1);
        assert_eq!(fs::read(&fixture.module).unwrap(), b"module-original");
        assert_eq!(fs::read(&fixture.python).unwrap(), b"python-fixture");
    }

    #[test]
    fn same_length_module_or_python_drift_rejects_without_creating_state() {
        for module in [true, false] {
            let fixture = Fixture::new();
            let path = if module {
                &fixture.module
            } else {
                &fixture.python
            };
            let changed = vec![b'x'; fs::metadata(path).unwrap().len() as usize];
            fs::write(path, &changed).unwrap();
            fixture.rejected(&fixture.args);
            assert_eq!(fs::read(path).unwrap(), changed);
        }
    }

    #[test]
    fn external_wrong_run_and_directory_budget_positions_reject_without_writes() {
        let fixture = Fixture::new();
        for state in [
            fixture.root.join("login-budget.json"),
            fixture
                .backend
                .join(".local-tests/other/post-copy/login-budget.json"),
            fixture.directory.join("post-copy/../login-budget.json"),
            fixture.directory.join("post-copy/login-budget.json/extra"),
        ] {
            let mut args = fixture.args.clone();
            args["pacing"]["bindings"]["login_budget_state"] = json!(state);
            fixture.rejected(&args);
        }
        let state = fixture.directory.join("post-copy/login-budget.json");
        fs::create_dir_all(&state).unwrap();
        assert!(request(&fixture.args).is_err());
        assert_eq!(fs::read_dir(&state).unwrap().count(), 0);
    }

    #[test]
    fn exact_types_hashes_and_paths_fail_closed_without_private_diagnostics() {
        let fixture = Fixture::new();
        for (pointer, value) in [
            ("/backend", json!(null)),
            ("/backend", json!("relative-backend")),
            ("/directory", json!([])),
            ("/directory", json!(fixture.frontend)),
            ("/frontend_root", json!(true)),
            ("/frontend_root", json!("")),
            ("/frontend_root", json!("private-password-token\n")),
            ("/frontend_root", json!("private-password-token")),
            ("/frontend_root", json!(fixture.root.join("missing"))),
            ("/frontend_root", json!(fixture.python)),
            ("/pacing", json!([])),
            ("/pacing/contract", json!(1)),
            ("/pacing/bindings", json!(null)),
            ("/pacing/bindings/python", json!(1)),
            ("/pacing/bindings/python", json!("relative-python")),
            ("/pacing/bindings/login_budget_state", json!(false)),
            ("/pacing/python_sha256", json!(1)),
            ("/pacing/python_sha256", json!("A".repeat(64))),
            (
                "/pacing/login_budget_module_sha256",
                json!("private-password-token"),
            ),
        ] {
            let mut args = fixture.args.clone();
            *args.pointer_mut(pointer).unwrap() = value;
            fixture.rejected(&args);
        }
        for pointer in ["", "/pacing", "/pacing/bindings"] {
            let mut args = fixture.args.clone();
            args.pointer_mut(pointer).unwrap()["private-password-token"] = json!(true);
            fixture.rejected(&args);
        }
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn linked_roots_tools_module_and_budget_parent_are_rejected() {
        let fixture = Fixture::new();
        let frontend = fixture.root.join("linked frontend");
        let python = fixture.root.join("linked-python.exe");
        let backend = fixture.root.join("linked backend");
        let directory = fixture.backend.join(".local-tests/linked run");
        #[cfg(windows)]
        {
            std::os::windows::fs::symlink_dir(&fixture.frontend, &frontend).unwrap();
            std::os::windows::fs::symlink_file(&fixture.python, &python).unwrap();
            std::os::windows::fs::symlink_dir(&fixture.backend, &backend).unwrap();
            std::os::windows::fs::symlink_dir(&fixture.directory, &directory).unwrap();
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&fixture.frontend, &frontend).unwrap();
            std::os::unix::fs::symlink(&fixture.python, &python).unwrap();
            std::os::unix::fs::symlink(&fixture.backend, &backend).unwrap();
            std::os::unix::fs::symlink(&fixture.directory, &directory).unwrap();
        }
        for (pointer, value) in [
            ("/frontend_root", json!(frontend)),
            ("/pacing/bindings/python", json!(python)),
            ("/backend", json!(backend)),
            ("/directory", json!(directory)),
        ] {
            let mut args = fixture.args.clone();
            *args.pointer_mut(pointer).unwrap() = value;
            if pointer == "/backend" {
                args["directory"] = json!(backend.join(".local-tests/run one"));
            }
            if matches!(pointer, "/backend" | "/directory") {
                let state = Path::new(args["directory"].as_str().unwrap())
                    .join("post-copy/login-budget.json");
                args["pacing"]["bindings"]["login_budget_state"] = json!(state);
            }
            fixture.rejected(&args);
        }
        fs::remove_file(&fixture.module).unwrap();
        let original = fixture.root.join("module-original.mjs");
        fs::write(&original, b"module-original").unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&original, &fixture.module).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&original, &fixture.module).unwrap();
        fixture.rejected(&fixture.args);
        // 恢复原模块后再隔离检查预算父目录，避免由前一个拒绝原因掩盖。
        fs::remove_file(&fixture.module).unwrap();
        fs::write(&fixture.module, b"module-original").unwrap();
        let parent = fixture.directory.join("post-copy");
        let target = fixture.backend.join(".local-tests/budget-target");
        fs::create_dir(&target).unwrap();
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&target, &parent).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &parent).unwrap();
        assert!(request(&fixture.args).is_err());
        assert!(!target.join("login-budget.json").exists());
        assert_eq!(fs::read(&fixture.module).unwrap(), b"module-original");
    }
}
