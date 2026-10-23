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
    let args = json!({"backend":f.root,"identity":f.identity,
        "administrator_credentials":descriptor});
    assert_eq!(validate_request(&args).unwrap(), Value::Null);
    let mut overridden = args;
    overridden["password_env"] = json!("OLD_PASSWORD");
    assert!(validate_request(&overridden).is_err());
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
    }
    let mut wrong = f.identity.clone();
    wrong["username"] = json!("other");
    f.rejected(&descriptor, &wrong);
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
