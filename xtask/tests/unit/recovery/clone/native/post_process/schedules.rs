use super::super::schedule_images::tests::{action, before};
use super::*;

struct Port {
    row: Value,
    logins: usize,
    writes: usize,
}

impl ReadPort for Port {
    fn checkpoint(&self) -> Result<()> {
        Ok(())
    }

    fn row(&mut self, _: &Value) -> Result<Value> {
        Ok(self.row.clone())
    }
}

impl SessionPort for Port {
    fn verify_administrator(&mut self) -> Result<()> {
        Ok(())
    }

    fn login(&mut self) -> Result<Value> {
        self.logins += 1;
        Ok(json!({"subject_id":"9","tenant_id":"system","username":"test","permissions":["*"]}))
    }

    fn get(&mut self, action: &Value) -> Result<Value> {
        Ok(
            json!({"id":action["schedule_id"],"handler_key":action["handler_key"],
            "enabled":self.row["row"]["enabled"] == 1,"version":self.row["row"]["version"]}),
        )
    }

    fn disable(&mut self, _: &Value) -> Result<Value> {
        self.writes += 1;
        self.row["row"]["enabled"] = json!(0);
        self.row["row"]["next_run_at"] = Value::Null;
        self.row["row"]["updated_at"] = self.row["database_now"].clone();
        self.row["row"]["version"] = json!(6);
        Err("模拟提交成功且响应丢失".into())
    }
}

#[test]
fn committed_unknown_action_blocks_login_until_explicit_reconciliation() {
    let backend = crate::workspace::root_dir();
    let root = backend.join(".local-tests");
    fs::create_dir_all(&root).unwrap();
    let temporary = tempfile::tempdir_in(&root).unwrap();
    let base = crate::recovery::fixture_control::portable_path(temporary.path());
    let row = before();
    let pending = [action(&backend, &row)];
    let registration = json!({"path":"fixture","bytes":1,"sha256":"a".repeat(64)});
    let administrator = json!({"subject_id":"9","tenant_id":"system","username":"test"});
    let mut port = Port {
        row,
        logins: 0,
        writes: 0,
    };
    let attempt = |number, mode| {
        let output = base.join(format!("attempt-{number:04}"));
        fs::create_dir(&output).unwrap();
        Prepared::load(
            Settings {
                backend: &backend,
                execution: &backend,
                base: &base,
                output: &output,
                registration: &registration,
                pending: &pending,
                administrator: &administrator,
                number,
            },
            mode,
        )?
        .run(&mut port)
    };
    check_unknown(&base, &pending[0], attempt);
    assert_eq!((port.logins, port.writes), (3, 1));
}

fn check_unknown(base: &Path, action: &Value, mut attempt: impl FnMut(u64, Mode) -> Result<Value>) {
    assert!(attempt(1, Mode::Apply).is_err());
    let directory = base.join(format!(
        "schedule-{}/0001",
        action["schedule_id"].as_str().unwrap()
    ));
    let original = common::binding(&directory.join("intent.json")).unwrap();
    assert!(attempt(2, Mode::Apply).is_err());
    assert!(!base.join("attempt-0002/authentication.json").exists());
    assert_eq!(
        attempt(3, Mode::Reconcile).unwrap()["status"],
        "target_schedule_actions_reconciled"
    );
    let proof = document::read(&directory.join("confirmed.json")).unwrap();
    assert_eq!(proof["api_execution_proven"], false);
    assert_eq!(proof["evidence"]["intent.json"], original);
    assert_eq!(
        attempt(4, Mode::Apply).unwrap()["status"],
        "target_schedule_actions_verified"
    );
    assert_eq!(
        common::binding(&directory.join("intent.json")).unwrap(),
        original
    );
    assert!(!directory.with_file_name("0002").exists());
}
