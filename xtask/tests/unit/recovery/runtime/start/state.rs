use super::*;

#[cfg(windows)]
#[path = "native_cleanup.rs"]
mod native_cleanup;

struct Fixture {
    _work: tempfile::TempDir,
    backend: PathBuf,
    control: PathBuf,
    root: PathBuf,
    binding: Value,
    request: Value,
    commands: Value,
}

impl Fixture {
    fn new() -> Self {
        let local = crate::workspace::root_dir().join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let work = tempfile::tempdir_in(&local).unwrap();
        let backend =
            crate::recovery::fixture_control::portable_path(&work.path().canonicalize().unwrap());
        let control = backend.join(".local-tests");
        let root = control.join("case/runtime");
        fs::create_dir_all(root.parent().unwrap()).unwrap();
        let binding = json!({
            "registration":{"path":control.join("registration.json"),"bytes":2,"sha256":"1".repeat(64)},
            "target_plan":{"path":control.join("target.json"),"bytes":2,"sha256":"2".repeat(64)},
        });
        let request = json!({"runtime_directory":root,"registration":binding,
            "authority":{"scope_id":"state-test"},"tools":{"coordinator_root":backend}});
        let executable = crate::recovery::fixture_control::portable_path(
            &std::env::current_exe().unwrap().canonicalize().unwrap(),
        );
        let commands = json!({"api":[executable],"worker":[executable],"frontend":[executable]});
        Self {
            _work: work,
            backend,
            control,
            root,
            binding,
            request,
            commands,
        }
    }

    fn runtime(&self) -> Runtime {
        Runtime {
            backend: self.backend.clone(),
            root: self.root.clone(),
            binding: self.binding.clone(),
        }
    }

    fn start(&self, owner: &Owner) -> Result<State> {
        let before = runtime_observation(&self.backend, &self.root)?;
        self.runtime().first(
            &self.request,
            &self.commands,
            before,
            owner,
            &mut Inputs::new(&self.backend),
        )
    }

    fn seed_role(&self, state: &State, role: &str, ordinal: usize) -> Result<Value> {
        let identity = |offset| {
            json!({
                "pid":80 + ordinal * 3 + offset,"started":"100","executable":self.commands[role][0],
            })
        };
        let tree = json!({
            "format_version":2,"kind":"full-stack-process-tree","runtime_directory":state.directory(),
            "role":role,"scope_id":"state-test","operation_id":state.generation()["roles"][role]["operation_id"],
            "supervisor":identity(0),"monitor":identity(1),"process":identity(2),"group_id":80 + ordinal * 3,
        });
        let directory = state.directory();
        strict::write_new(&directory.join(format!("{role}-tree.json")), &tree)?;
        strict::write_new(
            &directory.join(format!("{role}.json")),
            &json!({
                "format_version":1,"role":role,"scope_id":"state-test","identity":tree["process"],
            }),
        )?;
        fs::write(
            directory.join(format!("{role}.log")),
            b"registered test log",
        )?;
        Ok(tree)
    }
}

#[test]
fn first_generation_and_append_keep_original_intent_and_historical_commands() -> Result<()> {
    for existing_empty in [false, true] {
        let fixture = Fixture::new();
        if existing_empty {
            fs::create_dir(&fixture.root)?;
        }
        Owner::with(&fixture.control, "runtime-start:test", |owner| {
            let mut first = fixture.start(owner)?;
            let intent = generation::creation_path(&fixture.root);
            let intent_bytes = fs::read(&intent)?;
            assert_eq!(first.number(), 1);
            assert_eq!(first.directory(), fixture.root.join("generation-0001"));
            assert_eq!(first.generation()["status"], "starting");
            first.update(owner, |generation| {
                generation["status"] = json!("start_failed");
                generation["error_type"] = json!("FixtureStartFailure");
                Ok(())
            })?;
            let old = first.generation().clone();
            let old_directory = first.directory.clone();
            let mut commands = fixture.commands.clone();
            commands["api"]
                .as_array_mut()
                .unwrap()
                .push(json!("--next"));
            let previous = Previous {
                saved: first.saved,
                commands: first.commands,
                root_identity: first.root_identity,
                directory: old_directory.clone(),
                directory_identity: first.directory_identity,
            };
            let next = fixture
                .runtime()
                .append(&fixture.request, &commands, previous, owner)?;
            assert_eq!(next.number(), 2);
            assert_eq!(next.directory(), fixture.root.join("generation-0002"));
            assert_eq!(next.saved.document.value["generations"][0], old);
            assert_eq!(
                next.generation()["roles"]["api"]["command"],
                commands["api"]
            );
            assert_eq!(fs::read(&intent)?, intent_bytes);
            assert!(old_directory.is_dir() && next.directory().is_dir());
            Ok(())
        })?;
    }
    Ok(())
}

#[test]
fn first_intent_and_state_updates_fail_closed_on_unknown_or_replaced_files() -> Result<()> {
    let fixture = Fixture::new();
    Owner::with(&fixture.control, "runtime-start:test", |owner| {
        let intent = generation::creation_path(&fixture.root);
        fs::write(&intent, b"{}")?;
        assert!(
            fixture
                .start(owner)
                .err()
                .unwrap()
                .to_string()
                .contains("显式恢复")
        );
        assert_eq!(fs::read(&intent)?, b"{}");
        assert!(!fixture.root.exists());
        fs::remove_file(&intent)?;

        let mut state = fixture.start(owner)?;
        let path = fixture.root.join(STATE);
        let bytes = fs::read(&path)?;
        let unknown = state.directory().join("foreign.json");
        fs::write(&unknown, b"original")?;
        let err = state.update(owner, |_| Ok(())).unwrap_err();
        assert!(err.to_string().contains("未登记文件"));
        assert_eq!(fs::read(&unknown)?, b"original");
        assert_eq!(fs::read(&path)?, bytes);
        fs::remove_file(&unknown)?;

        let old = fixture.control.join("original-lifecycle.json");
        fs::rename(&path, &old)?;
        fs::write(&path, &bytes)?;
        let err = state.update(owner, |_| Ok(())).unwrap_err();
        assert!(err.to_string().contains("文件身份变化"));
        assert_eq!(fs::read(&path)?, bytes);
        assert_eq!(fs::read(&old)?, bytes);
        Ok(())
    })
}

#[test]
fn launch_requires_all_roles_then_publishes_original_receipt_once_before_running() -> Result<()> {
    let fixture = Fixture::new();
    Owner::with(&fixture.control, "runtime-start:test", |owner| {
        let mut state = fixture.start(owner)?;
        let launch = state.directory().join(LAUNCH);
        assert!(state.publish_launch(owner).is_err());
        assert!(!launch.exists());
        for (ordinal, role) in ROLES.into_iter().enumerate() {
            let tree = fixture.seed_role(&state, role, ordinal)?;
            state.update(owner, |generation| {
                let item = &mut generation["roles"][role];
                item["tree"] = tree;
                item["state"] = json!("running");
                Ok(())
            })?;
        }
        let binding = state.publish_launch(owner)?;
        let raw = fs::read(&launch)?;
        let published = strict::read(&launch)?;
        assert_eq!(published["generation"], state.number());
        assert_eq!(published["request"], fixture.request);
        assert_eq!(state.generation()["launch"], binding);
        assert_eq!(state.generation()["status"], "running");
        assert!(state.publish_launch(owner).is_err());
        assert_eq!(fs::read(&launch)?, raw);
        Ok(())
    })
}
