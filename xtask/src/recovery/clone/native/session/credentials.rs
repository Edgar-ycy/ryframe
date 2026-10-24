//! 复制后管理员使用独立绑定的当前凭据；不推断初始化密码或修改账号。
use crate::Result;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

const REJECTED: &str = "管理员凭据校验失败；检查后端 .local-tests 内的 TOML 路径、文件绑定及 administrator.tenant_id/username/password；password 必须是首次改密后的当前凭据";

pub(crate) fn load(root: &Path, descriptor: &Value, identity: &Value) -> Result<String> {
    super::model::identity(identity).map_err(|_| REJECTED)?;
    load_account(
        root,
        descriptor,
        identity["tenant_id"].as_str().ok_or(REJECTED)?,
        identity["username"].as_str().ok_or(REJECTED)?,
    )
}

pub(crate) fn load_account(
    root: &Path,
    descriptor: &Value,
    tenant: &str,
    username: &str,
) -> Result<String> {
    read(root, descriptor, tenant, username).map_err(|_| REJECTED.into())
}

fn read(root: &Path, descriptor: &Value, tenant: &str, username: &str) -> Result<String> {
    if tenant != "system" || username.trim().is_empty() {
        return Err(REJECTED.into());
    }
    crate::local_test_path::validate_absolute_path(
        root,
        crate::local_test_path::LocalTestPathKind::ExistingDirectory,
    )?;
    super::super::json::exact(descriptor, &["path", "bytes", "sha256"], "管理员凭据绑定")?;
    if descriptor["bytes"]
        .as_u64()
        .is_none_or(|bytes| bytes == 0 || bytes > 64 * 1024)
    {
        return Err(REJECTED.into());
    }
    let path = super::super::lifecycle::manifest::bound_file(root, descriptor, "管理员凭据", true)?;
    let before = fs::read(&path)?;
    if path.extension().and_then(|value| value.to_str()) != Some("toml")
        || Some(before.len() as u64) != descriptor["bytes"].as_u64()
        || Some(hex::encode(Sha256::digest(&before)).as_str()) != descriptor["sha256"].as_str()
    {
        return Err(REJECTED.into());
    }
    let table: toml::Table = toml::from_str(std::str::from_utf8(&before)?)?;
    if table.len() != 1 {
        return Err(REJECTED.into());
    }
    let administrator = table
        .get("administrator")
        .and_then(toml::Value::as_table)
        .ok_or(REJECTED)?;
    if administrator.len() != 3
        || !["tenant_id", "username", "password"]
            .iter()
            .all(|key| administrator.contains_key(*key))
        || administrator.get("tenant_id").and_then(toml::Value::as_str) != Some(tenant)
        || administrator.get("username").and_then(toml::Value::as_str) != Some(username)
    {
        return Err(REJECTED.into());
    }
    let password = administrator
        .get("password")
        .and_then(toml::Value::as_str)
        .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
        .ok_or(REJECTED)?
        .to_owned();
    if super::super::lifecycle::manifest::bound_file(root, descriptor, "管理员凭据", true)? != path
        || fs::read(&path)? != before
    {
        return Err(REJECTED.into());
    }
    Ok(password)
}

/// 现存 Python 注册调用方只请求校验；协议结果不返回明文凭据。
pub(crate) fn validate_request(args: &Value) -> Result<Value> {
    let result = (|| -> Result<Value> {
        super::super::json::exact(
            args,
            &["backend", "identity", "administrator_credentials"],
            "管理员凭据请求",
        )?;
        let root = Path::new(args["backend"].as_str().ok_or(REJECTED)?);
        let identity = &args["identity"];
        let subject = identity["subject_id"].as_str().ok_or(REJECTED)?;
        let username = identity["username"].as_str().ok_or(REJECTED)?;
        if subject.is_empty()
            || subject.len() > 19
            || subject.starts_with('0')
            || !subject.bytes().all(|byte| byte.is_ascii_digit())
            || username.is_empty()
            || username.len() > 64
            || !username
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
        {
            return Err(REJECTED.into());
        }
        load(root, &args["administrator_credentials"], &args["identity"])?;
        Ok(Value::Null)
    })();
    result.map_err(|_| REJECTED.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::clone::native::{digest, lifecycle::manifest};
    use serde_json::json;
    use tempfile::TempDir;

    struct Fixture {
        _temporary: TempDir,
        root: std::path::PathBuf,
        file: std::path::PathBuf,
        identity: Value,
    }

    impl Fixture {
        fn new() -> Self {
            let temporary = tempfile::tempdir_in(crate::workspace::root_dir()).unwrap();
            let root = temporary.path().to_path_buf();
            let local = root.join(".local-tests/session");
            fs::create_dir_all(&local).unwrap();
            let file = local.join("administrator.toml");
            Self {
                _temporary: temporary,
                root,
                file,
                identity: json!({"subject_id":"9","tenant_id":"system","username":"admin",
                    "client_address":"198.19.20.1"}),
            }
        }

        fn write(&self, text: &str) -> Value {
            fs::write(&self.file, text).unwrap();
            manifest::binding(&self.file, &digest::file(&self.file).unwrap()).unwrap()
        }

        fn valid(&self) -> Value {
            self.write("[administrator]\ntenant_id='system'\nusername='admin'\npassword='Current!Changed123'\n")
        }

        fn rejected(&self, descriptor: &Value, identity: &Value) {
            let error = load(&self.root, descriptor, identity)
                .unwrap_err()
                .to_string();
            assert_eq!(error, REJECTED);
            for secret in [
                "Current!Changed123",
                "private-parser-sentinel",
                "Initial!Only123",
            ] {
                assert!(!error.contains(secret));
            }
        }
    }

    #[test]
    fn current_credentials_are_independent_of_bootstrap_and_never_returned_by_validation() {
        let f = Fixture::new();
        let descriptor = f.valid();
        fs::create_dir(f.root.join("config")).unwrap();
        fs::write(
            f.root.join("config/app.test.toml"),
            "[bootstrap]\nadmin_password='Initial!Only123'\n",
        )
        .unwrap();
        assert_eq!(
            load(&f.root, &descriptor, &f.identity).unwrap(),
            "Current!Changed123"
        );
        assert_eq!(
            load_account(&f.root, &descriptor, "system", "admin").unwrap(),
            "Current!Changed123"
        );
        assert!(load_account(&f.root, &descriptor, "other", "admin").is_err());
        assert!(load_account(&f.root, &descriptor, "system", "other").is_err());
        let args = json!({"backend":f.root,"identity":f.identity,
            "administrator_credentials":descriptor});
        assert_eq!(validate_request(&args).unwrap(), Value::Null);
        for subject in ["1", "9999999999999999999"] {
            for username in ["A_z.09-".to_owned(), "a".repeat(64)] {
                let mut valid = args.clone();
                valid["identity"]["subject_id"] = json!(subject);
                valid["identity"]["username"] = json!(username);
                valid["administrator_credentials"] = f.write(&format!(
                    "[administrator]\ntenant_id='system'\nusername='{username}'\npassword='Current!Changed123'\n"
                ));
                assert_eq!(validate_request(&valid).unwrap(), Value::Null);
            }
        }
        let descriptor = f.valid();
        let mut rejected = args.clone();
        rejected["administrator_credentials"] = descriptor;
        for subject in [
            json!(""),
            json!("0"),
            json!("01"),
            json!("+1"),
            json!("1\n"),
            json!("１"),
            json!("١"),
            json!("10000000000000000000"),
            json!(9),
            Value::Null,
            json!(true),
        ] {
            rejected["identity"]["subject_id"] = subject;
            assert_eq!(
                validate_request(&rejected).unwrap_err().to_string(),
                REJECTED
            );
        }
        // 此长度仍是既有 load 接受的 u64；仅 post 元数据入口限制为 19 位。
        assert!(
            load(
                &f.root,
                &rejected["administrator_credentials"],
                &json!({
                    "subject_id":"10000000000000000000", "tenant_id":"system",
                    "username":"admin", "client_address":"198.19.20.1"
                })
            )
            .is_ok()
        );
        for username in [
            "a".repeat(65),
            "管理员".into(),
            "admin user".into(),
            "admin/".into(),
        ] {
            let mut invalid = args.clone();
            invalid["identity"]["username"] = json!(username);
            invalid["administrator_credentials"] = f.write(&format!(
                "[administrator]\ntenant_id='system'\nusername='{username}'\npassword='Current!Changed123'\n"
            ));
            assert!(
                load(
                    &f.root,
                    &invalid["administrator_credentials"],
                    &invalid["identity"]
                )
                .is_ok()
            );
            assert_eq!(
                validate_request(&invalid).unwrap_err().to_string(),
                REJECTED
            );
        }
        let descriptor = f.valid();
        for username in [
            json!(""),
            json!("admin\n"),
            json!(9),
            Value::Null,
            json!(true),
        ] {
            let mut invalid = args.clone();
            invalid["administrator_credentials"] = descriptor.clone();
            invalid["identity"]["username"] = username;
            assert_eq!(
                validate_request(&invalid).unwrap_err().to_string(),
                REJECTED
            );
        }
        let mut overridden = args;
        overridden["password_env"] = json!("OLD_PASSWORD");
        assert_eq!(
            validate_request(&overridden).unwrap_err().to_string(),
            REJECTED
        );
    }

    #[test]
    fn exact_identity_and_toml_schema_reject_legacy_fields_and_redact_parser_errors() {
        let f = Fixture::new();
        for body in [
            "[administrator]\ntenant_id='other'\nusername='admin'\npassword='private-parser-sentinel'\n",
            "[administrator]\ntenant_id='system'\nusername='other'\npassword='private-parser-sentinel'\n",
            "[administrator]\ntenant_id='system'\nusername='admin'\npassword=''\n",
            "[administrator]\ntenant_id='system'\nusername='admin'\npassword_env='OLD_PASSWORD'\n",
            "[administrator]\ntenant_id='system'\nusername='admin'\npassword='ok'\npassword_env='OLD_PASSWORD'\n",
            "[administrator]\ntenant_id='system'\nusername='admin'\npassword='private-parser-sentinel\n",
            "[administrator]\ntenant_id='system'\nusername='admin'\npassword='ok'\n[bootstrap]\nadmin_password='Initial!Only123'\n",
        ] {
            f.rejected(&f.write(body), &f.identity);
        }
        let descriptor = f.valid();
        for field in ["password", "password_env"] {
            let mut identity = f.identity.clone();
            identity[field] = json!("private-parser-sentinel");
            f.rejected(&descriptor, &identity);
            let args = json!({"backend":f.root,"identity":identity,
                "administrator_credentials":descriptor});
            assert_eq!(validate_request(&args).unwrap_err().to_string(), REJECTED);
        }
        let mut wrong = f.identity.clone();
        wrong["username"] = json!("other");
        f.rejected(&descriptor, &wrong);
        for address in ["198.19.20.1", "198.19.20.254"] {
            let mut identity = f.identity.clone();
            identity["client_address"] = json!(address);
            assert_eq!(
                validate_request(&json!({"backend":f.root,"identity":identity,
                "administrator_credentials":descriptor}))
                .unwrap(),
                Value::Null
            );
        }
        for (field, value) in [
            ("tenant_id", json!("other")),
            ("client_address", json!("198.19.20.0")),
            ("client_address", json!("198.19.20.255")),
            ("client_address", json!("198.19.20.01")),
            ("client_address", json!("198.19.20.１")),
            ("client_address", json!("198.19.20.1\n")),
            ("client_address", json!(9)),
        ] {
            let mut identity = f.identity.clone();
            identity[field] = value;
            f.rejected(&descriptor, &identity);
            assert_eq!(
                validate_request(&json!({"backend":f.root,"identity":identity,
                "administrator_credentials":descriptor}))
                .unwrap_err()
                .to_string(),
                REJECTED
            );
        }
    }

    #[test]
    fn binding_changes_and_paths_outside_local_tests_fail_closed() {
        let f = Fixture::new();
        let descriptor = f.valid();
        fs::write(
            &f.file,
            "[administrator]\npassword='private-parser-sentinel'\n",
        )
        .unwrap();
        f.rejected(&descriptor, &f.identity);
        let descriptor = f.valid();
        for (field, value) in [
            ("bytes", json!(1)),
            ("sha256", json!("0".repeat(64))),
            ("path", json!("administrator.toml")),
            ("password", json!("private-parser-sentinel")),
        ] {
            let mut wrong = descriptor.clone();
            wrong[field] = value;
            f.rejected(&wrong, &f.identity);
        }
        let outside = f.root.join("administrator.toml");
        fs::copy(&f.file, &outside).unwrap();
        let outside = manifest::binding(&outside, &digest::file(&outside).unwrap()).unwrap();
        f.rejected(&outside, &f.identity);
        let mut traversal = descriptor;
        traversal["path"] = json!(
            f.root
                .join(".local-tests/session/../session/administrator.toml")
        );
        f.rejected(&traversal, &f.identity);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_credentials_are_rejected() {
        let f = Fixture::new();
        let descriptor = f.valid();
        let linked = f.file.with_file_name("linked.toml");
        std::os::unix::fs::symlink(&f.file, &linked).unwrap();
        let mut descriptor = descriptor;
        descriptor["path"] = json!(linked);
        f.rejected(&descriptor, &f.identity);
    }
}
