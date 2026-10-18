#[cfg(windows)]
use std::{collections::BTreeMap, fs, path::PathBuf};

#[cfg(windows)]
use super::{
    FakeRuntime, Fixture, ROLES, lifecycle, lifecycle_binding, powershell_sleep_command,
    read_document,
};

#[cfg(windows)]
#[test]
fn failed_start_publication_reaps_every_tree_and_preserves_concurrent_receipt() {
    let fixture = Fixture::new("publish-failure");
    let binding = lifecycle_binding(&fixture);
    let command = powershell_sleep_command();
    let runtime = FakeRuntime {
        commands: ROLES
            .into_iter()
            .map(|role| (role.to_owned(), command.clone()))
            .collect::<BTreeMap<_, _>>(),
        fail_role: None,
        supervisor_executable: PathBuf::from(env!("CARGO_BIN_EXE_xtask")),
    };
    let target = binding.run.join("start.json");
    fs::write(&target, b"concurrent unknown receipt\n").unwrap();
    assert!(lifecycle::start_runtime(&binding, &runtime).is_err());
    assert_eq!(fs::read(&target).unwrap(), b"concurrent unknown receipt\n");
    let failure = read_document(&binding.run.join("start-failure.json"), "启动发布失败").unwrap();
    assert_eq!(failure.value["stage"], "start-finalize-start");
    assert_eq!(failure.value["completions"].as_object().unwrap().len(), 3);
    assert_eq!(failure.value["ports_closed"], true);
    for role in ROLES {
        let tree = read_document(
            &binding
                .run
                .join("processes")
                .join(format!("{role}-tree.json")),
            "进程树",
        )
        .unwrap();
        assert!(!crate::process::process_is_running(
            tree.value["process"]["pid"].as_u64().unwrap() as u32
        ));
    }
}
