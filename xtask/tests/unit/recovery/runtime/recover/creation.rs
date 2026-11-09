use super::*;
use crate::recovery::fixture_control::portable_path;
use std::cell::Cell;

struct Fixture {
    _temp: tempfile::TempDir,
    backend: PathBuf,
    root: PathBuf,
    binding: Value,
    request: Value,
    commands: Value,
}

impl Fixture {
    fn new() -> Self {
        let local = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let temp = tempfile::Builder::new()
            .prefix("runtime-recover-cut-")
            .tempdir_in(local)
            .unwrap();
        let backend = portable_path(&temp.path().canonicalize().unwrap());
        let local = backend.join(".local-tests");
        let case = local.join("case");
        fs::create_dir_all(&case).unwrap();
        let root = case.join("runtime");
        let descriptor =
            |name: &str| json!({"path":local.join(name),"bytes":2,"sha256":"1".repeat(64)});
        let binding = json!({"registration":descriptor("registration.json"),"target_plan":descriptor("target.json")});
        let request = json!({"registration":binding,"runtime_directory":root,
            "tools":{"coordinator_root":backend},"authority":{"scope_id":"runtime-cut"}});
        let executable = std::env::current_exe().unwrap();
        let commands = json!({"api":[executable],"worker":[executable],"frontend":[executable]});
        Self {
            _temp: temp,
            backend,
            root,
            binding,
            request,
            commands,
        }
    }

    fn cut(&self) -> Cut<'_> {
        Cut {
            backend: &self.backend,
            root: &self.root,
            binding: &self.binding,
        }
    }

    fn create(&self) -> Value {
        self.cut()
            .model("create-intent", &self.request, &Value::Null, &Value::Null)
            .unwrap()
    }

    fn inspect(&self, inputs: &mut Inputs) -> Result<Creation> {
        self.cut().read(
            inputs,
            || panic!("存在原意图时不用首次登记的未开始分支"),
            || Ok(()),
            |request, _| {
                assert_eq!(request, &self.request);
                Ok(self.commands.clone())
            },
        )
    }
}

#[test]
fn absent_intent_rechecks_original_registration_and_cannot_create_state() {
    let fixture = Fixture::new();
    let mut inputs = Inputs::new(&fixture.backend);
    let registered = Cell::new(0);
    let result = fixture
        .cut()
        .read(
            &mut inputs,
            || {
                registered.set(registered.get() + 1);
                Ok(())
            },
            || panic!("无 intent 不直接进入创建核验"),
            |_, _| panic!("无 intent 不推导启动命令"),
        )
        .unwrap();
    assert!(result.0.is_none());
    assert_eq!(registered.get(), 1);
    assert!(!fixture.root.exists());
    assert!(!generation::creation_path(&fixture.root).exists());
    assert!(
        fixture
            .cut()
            .read(
                &mut inputs,
                || {
                    fixture.create();
                    Ok(())
                },
                || panic!("同一次观察不能采用期间出现的 intent"),
                |_, _| panic!("同一次观察不能采用期间出现的命令"),
            )
            .is_err()
    );
    assert!(!fixture.root.join(STATE).exists());
}

#[test]
fn existing_intent_materializes_one_exact_lifecycle_from_missing_or_empty_root() {
    for exists in [false, true] {
        let fixture = Fixture::new();
        let original = fixture.create();
        if exists {
            fs::create_dir(&fixture.root).unwrap();
        }
        let mut inputs = Inputs::new(&fixture.backend);
        let verified = fixture.inspect(&mut inputs).unwrap();
        assert_eq!(verified, fixture.inspect(&mut inputs).unwrap());
        let intent = verified.0.unwrap();
        let closed = Cell::new(0);
        fixture
            .cut()
            .materialize(
                &intent,
                &inputs,
                || Ok(()),
                || {
                    closed.set(closed.get() + 1);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(closed.get(), 2);
        let state_path = fixture.root.join(STATE);
        let state = document::read(&state_path, &fixture.backend, "首代测试状态").unwrap();
        assert_eq!(state.value["creation_intent"], original);
        assert_eq!(state.value["generations"][0]["request"], fixture.request);
        assert_eq!(state.value["generations"][0]["status"], "starting");
        assert_eq!(state.value["generations"].as_array().unwrap().len(), 1);
        for role in generation::ROLES {
            assert_eq!(
                state.value["generations"][0]["roles"][role]["command"],
                fixture.commands[role]
            );
            assert_eq!(
                state.value["generations"][0]["roles"][role]["state"],
                "pending"
            );
        }
        assert!(!fixture.root.join("generation-0001").exists());
        assert!(
            fixture
                .cut()
                .materialize(&intent, &inputs, || Ok(()), || Ok(()))
                .is_err()
        );
        state.assert_unchanged().unwrap();
        assert_eq!(
            common::binding(&generation::creation_path(&fixture.root)).unwrap(),
            original
        );
    }
}

#[test]
fn unknown_entries_or_lost_closed_observation_prevent_publication() {
    let fixture = Fixture::new();
    fixture.create();
    fs::create_dir(&fixture.root).unwrap();
    let mut inputs = Inputs::new(&fixture.backend);
    let original = fixture.inspect(&mut inputs).unwrap().0.unwrap();
    let foreign = fixture.root.join("foreign.json");
    fs::write(&foreign, b"unregistered").unwrap();
    assert!(
        fixture
            .cut()
            .materialize(&original, &inputs, || Ok(()), || Ok(()))
            .is_err()
    );
    assert_eq!(fs::read(&foreign).unwrap(), b"unregistered");
    assert!(!fixture.root.join(STATE).exists());

    let fixture = Fixture::new();
    fixture.create();
    fs::create_dir(&fixture.root).unwrap();
    let mut inputs = Inputs::new(&fixture.backend);
    let original = fixture.inspect(&mut inputs).unwrap().0.unwrap();
    assert!(
        fixture
            .cut()
            .materialize(&original, &inputs, || Ok(()), || Err("闭口失败".into()))
            .is_err()
    );
    assert!(!fixture.root.join(STATE).exists());
    let closed = Cell::new(0);
    assert!(
        fixture
            .cut()
            .materialize(
                &original,
                &inputs,
                || Ok(()),
                || {
                    if closed.replace(closed.get() + 1) == 0 {
                        Ok(())
                    } else {
                        Err("发布后闭口失败".into())
                    }
                }
            )
            .is_err()
    );
    let state =
        document::read(&fixture.root.join(STATE), &fixture.backend, "保留失败现场").unwrap();
    assert_eq!(state.value["creation_intent"], original.binding);
}

#[cfg(target_os = "linux")]
#[test]
fn owner_transition_tolerates_only_ctime_on_exact_absent_control_anchor() {
    let control = Path::new("/workspace/.local-tests");
    let before = (
        control.join("runtime"),
        json!({"exists":false,"entries":[],
            "anchor":{"path":control,"device":2,"inode":3,"created_ns":4}}),
    );
    let mut after = before.clone();
    after.1["anchor"]["created_ns"] = json!(5);
    assert!(same_control_anchor(&before, &after, control));
    assert!(!same_control_anchor(
        &before,
        &after,
        Path::new("/unrelated")
    ));
    for (field, value) in [
        ("device", json!(4)),
        ("inode", json!(4)),
        ("path", json!("/other")),
    ] {
        let mut changed = after.clone();
        changed.1["anchor"][field] = value;
        assert!(!same_control_anchor(&before, &changed, control));
    }
    let mut changed = after.clone();
    changed.1["entries"] = json!(["unknown"]);
    assert!(!same_control_anchor(&before, &changed, control));
    let mut changed = after.clone();
    changed.1["exists"] = json!(true);
    assert!(!same_control_anchor(&before, &changed, control));
    let mut changed = after;
    changed.0 = control.join("other-runtime");
    assert!(!same_control_anchor(&before, &changed, control));
}
