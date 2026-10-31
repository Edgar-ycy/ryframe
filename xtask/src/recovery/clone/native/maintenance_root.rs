//! 复制工厂与交接共用维护收据登记的执行根；不推断缺失根或创建目录。
use std::{error::Error, fs, io, path::PathBuf};

use serde_json::{Value, json};

use crate::{
    Result,
    local_test_path::{LocalTestPathKind, validate_absolute_path, validate_local_test_path},
    recovery::fixture_control::portable_path,
};

use super::{digest, json as document, resources::common};

const ADVICE: &str = "检查绑定维护收据的 path/bytes/sha256 与 backend_root，执行根必须是当前后端或其 .local-tests 内的现有无链接绝对目录；保留原收据与失败现场，不猜测、创建或重放";

pub(crate) fn request(args: &Value) -> Result<Value> {
    resolve(args).map(|root| json!({"path":portable_path(&root)}))
}

fn resolve(args: &Value) -> Result<PathBuf> {
    document::exact(args, &["backend", "maintenance_build"], "复制维护根参数").map_err(|_| {
        diagnostic(
            "arguments",
            "参数必须仅含 backend 和 maintenance_build",
            None,
        )
    })?;
    let backend = path(&args["backend"], "backend_type")?;
    validate_backend(&backend)?;
    let descriptor = &args["maintenance_build"];
    let before = common::read_bound(&backend, descriptor, "复制维护收据").map_err(|error| {
        diagnostic(
            "maintenance_binding",
            "维护收据路径、JSON 或摘要未通过绑定校验",
            os_code(error.as_ref()),
        )
    })?;
    let root = checked_root(&backend, &before["backend_root"])?;
    let after = common::read_bound(&backend, descriptor, "复制维护收据").map_err(|error| {
        diagnostic(
            "maintenance_recheck",
            "维护收据二次绑定或 JSON 读取失败",
            os_code(error.as_ref()),
        )
    })?;
    if before != after {
        return Err(diagnostic("maintenance_changed", "两次读取的维护收据内容不一致", None).into());
    }
    if checked_root(&backend, &after["backend_root"])? != root {
        return Err(diagnostic("backend_root_changed", "规范执行根在二次核验时变化", None).into());
    }
    common::bound_file(&backend, descriptor, "复制维护收据").map_err(|error| {
        diagnostic(
            "maintenance_recheck",
            "维护收据读取后的字节绑定或路径复核失败",
            os_code(error.as_ref()),
        )
    })?;
    Ok(root)
}

fn path(value: &Value, stage: &'static str) -> Result<PathBuf> {
    let text = value
        .as_str()
        .filter(|text| !text.is_empty() && !text.chars().any(char::is_control))
        .ok_or_else(|| diagnostic(stage, "根目录必须为非空文本且不得含控制字符", None))?;
    Ok(PathBuf::from(text))
}

fn checked_root(backend: &std::path::Path, value: &Value) -> Result<PathBuf> {
    validate_backend(backend)?;
    let declared = path(value, "backend_root_type")?;
    validate_absolute_path(&declared, LocalTestPathKind::ExistingDirectory).map_err(|_| {
        diagnostic(
            "backend_root_path",
            "执行根不满足现有无链接绝对目录约束",
            None,
        )
    })?;
    let root = fs::canonicalize(&declared).map_err(|error| {
        diagnostic(
            "backend_root_canonicalize",
            "执行根规范路径读取失败",
            error.raw_os_error(),
        )
    })?;
    let current = fs::canonicalize(backend).map_err(|error| {
        diagnostic(
            "backend_canonicalize",
            "当前后端规范路径读取失败",
            error.raw_os_error(),
        )
    })?;
    if root != current {
        validate_local_test_path(&declared, backend, LocalTestPathKind::ExistingDirectory)
            .map_err(|_| {
                diagnostic(
                    "backend_root_boundary",
                    "执行根未通过现有受管子目录核验",
                    None,
                )
            })?;
    }
    digest::path_text(&root, "复制维护根").map_err(|_| {
        diagnostic(
            "backend_root_encoding",
            "规范执行根无法表示为路径文本",
            None,
        )
    })?;
    Ok(root)
}

fn validate_backend(backend: &std::path::Path) -> Result<()> {
    validate_absolute_path(backend, LocalTestPathKind::ExistingDirectory)
        .map(|_| ())
        .map_err(|_| {
            diagnostic(
                "backend_directory",
                "当前后端不满足现有无链接绝对目录约束",
                None,
            )
            .into()
        })
}

fn diagnostic(stage: &'static str, reason: &'static str, os_code: Option<i32>) -> String {
    format!("复制维护执行根核验失败：stage={stage}；原因={reason}；os_code={os_code:?}。{ADVICE}")
}

fn os_code(error: &(dyn Error + 'static)) -> Option<i32> {
    let mut cause = Some(error);
    while let Some(error) = cause {
        if let Some(code) = error
            .downcast_ref::<io::Error>()
            .and_then(io::Error::raw_os_error)
        {
            return Some(code);
        }
        cause = error.source();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::BTreeMap, path::Path};
    use tempfile::TempDir;

    struct Fixture {
        _temporary: TempDir,
        root: PathBuf,
        backend: PathBuf,
        nested: PathBuf,
        receipt: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let parent = crate::workspace::root_dir().join(".local-tests");
            fs::create_dir_all(&parent).unwrap();
            let temporary = tempfile::Builder::new()
                .prefix("copy-maintenance-root-")
                .tempdir_in(parent)
                .unwrap();
            let root = fs::canonicalize(temporary.path()).unwrap();
            let backend = root.join("backend");
            let nested = backend.join(".local-tests/执行 树");
            let receipt = backend.join(".local-tests/maintenance/build.json");
            fs::create_dir_all(&nested).unwrap();
            fs::create_dir_all(receipt.parent().unwrap()).unwrap();
            fs::write(nested.join("marker"), b"preserved").unwrap();
            Self {
                _temporary: temporary,
                root,
                backend,
                nested,
                receipt,
            }
        }

        fn bind(&self, value: Value) -> Value {
            fs::write(&self.receipt, serde_json::to_vec(&value).unwrap()).unwrap();
            json!({"backend":portable_path(&self.backend),
                "maintenance_build":common::binding(&self.receipt).unwrap()})
        }

        fn rejected(&self, args: &Value, stage: &str) {
            let before = snapshot(&self.root);
            let error = request(args).unwrap_err().to_string();
            assert_eq!(snapshot(&self.root), before);
            assert!(error.contains(&format!("stage={stage}；")), "{error}");
            assert!(error.ends_with(ADVICE));
            assert!(!error.contains("private-password-token"));
        }
    }

    fn snapshot(root: &Path) -> BTreeMap<PathBuf, (String, Vec<u8>)> {
        let mut values = BTreeMap::new();
        let mut pending = vec![root.to_path_buf()];
        while let Some(path) = pending.pop() {
            let metadata = fs::symlink_metadata(&path).unwrap();
            let (kind, bytes) = if digest::linked(&metadata) {
                ("link", Vec::new())
            } else if metadata.is_dir() {
                pending.extend(
                    fs::read_dir(&path)
                        .unwrap()
                        .map(|entry| entry.unwrap().path()),
                );
                ("directory", Vec::new())
            } else {
                ("file", fs::read(&path).unwrap())
            };
            values.insert(path, (kind.into(), bytes));
        }
        values
    }

    #[test]
    fn current_and_managed_nested_roots_return_portable_paths_without_writes() {
        let fixture = Fixture::new();
        for root in [&fixture.backend, &fixture.nested] {
            let args = fixture.bind(json!({"backend_root":portable_path(root)}));
            let before = snapshot(&fixture.root);
            assert_eq!(
                request(&args).unwrap(),
                json!({"path":portable_path(&fs::canonicalize(root).unwrap())})
            );
            assert_eq!(snapshot(&fixture.root), before);
        }
        let args =
            fixture.bind(json!({"backend_root":fs::canonicalize(&fixture.backend).unwrap()}));
        assert_eq!(
            request(&args).unwrap(),
            json!({"path":portable_path(&fixture.backend)})
        );
    }

    #[test]
    fn external_relative_missing_or_invalid_roots_cannot_fall_back_to_backend() {
        let fixture = Fixture::new();
        let external = fixture.root.join("external");
        let ordinary = fixture.backend.join("ordinary");
        fs::create_dir(&external).unwrap();
        fs::create_dir(&ordinary).unwrap();
        for (value, stage) in [
            (json!({}), "backend_root_type"),
            (json!({"backend_root":null}), "backend_root_type"),
            (json!({"backend_root":17}), "backend_root_type"),
            (json!({"backend_root":""}), "backend_root_type"),
            (
                json!({"backend_root":"relative/private-password-token"}),
                "backend_root_path",
            ),
            (
                json!({"backend_root":fixture.root.join("missing")}),
                "backend_root_path",
            ),
            (json!({"backend_root":external}), "backend_root_boundary"),
            (json!({"backend_root":ordinary}), "backend_root_boundary"),
            (
                json!({"backend_root":fixture.backend.join(".local-tests")}),
                "backend_root_boundary",
            ),
            (
                json!({"backend_root":format!("{}/../执行 树", fixture.nested.display())}),
                "backend_root_path",
            ),
        ] {
            let args = fixture.bind(value);
            fixture.rejected(&args, stage);
        }
        let args = fixture.bind(json!({"backend_root":fixture.receipt}));
        fixture.rejected(&args, "backend_root_path");
    }

    #[test]
    fn bound_receipt_drift_types_and_unknown_arguments_reject_without_echoing_values() {
        let fixture = Fixture::new();
        let args = fixture.bind(json!({"backend_root":fixture.backend,"label":"original"}));
        let original = fs::read_to_string(&fixture.receipt).unwrap();
        fs::write(&fixture.receipt, original.replace("original", "changed!")).unwrap();
        fixture.rejected(&args, "maintenance_binding");
        let args = fixture.bind(json!({"backend_root":fixture.backend}));
        for (pointer, value) in [
            ("/backend", json!(null)),
            ("/backend", json!("private-password-token")),
            ("/maintenance_build", json!([])),
            ("/maintenance_build/path", json!(false)),
            (
                "/maintenance_build/path",
                json!(fixture.root.join("external.json")),
            ),
            ("/maintenance_build/bytes", json!(0)),
            ("/maintenance_build/bytes", json!(true)),
            ("/maintenance_build/sha256", json!("A".repeat(64))),
            ("/maintenance_build/sha256", json!("0".repeat(64))),
        ] {
            let mut wrong = args.clone();
            let stage = match pointer {
                "/backend" if value.is_null() => "backend_type",
                "/backend" => "backend_directory",
                _ => "maintenance_binding",
            };
            *wrong.pointer_mut(pointer).unwrap() = value;
            fixture.rejected(&wrong, stage);
        }
        let mut wrong = args;
        wrong["private-password-token"] = json!(true);
        fixture.rejected(&wrong, "arguments");
        fs::write(&fixture.receipt, b"private-password-token").unwrap();
        let args = json!({"backend":fixture.backend,"maintenance_build":common::binding(&fixture.receipt).unwrap()});
        fixture.rejected(&args, "maintenance_binding");
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn linked_roots_and_same_bytes_receipts_are_rejected_without_cleanup() {
        let fixture = Fixture::new();
        let linked = fixture.backend.join(".local-tests/linked-root");
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&fixture.backend, &linked).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&fixture.backend, &linked).unwrap();
        let args = fixture.bind(json!({"backend_root":linked}));
        fixture.rejected(&args, "backend_root_path");
        let mut args = fixture.bind(json!({"backend_root":fixture.backend}));
        let linked_receipt = fixture.receipt.with_file_name("linked.json");
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&fixture.receipt, &linked_receipt).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&fixture.receipt, &linked_receipt).unwrap();
        args["maintenance_build"]["path"] = json!(linked_receipt);
        fixture.rejected(&args, "maintenance_binding");
    }
}
