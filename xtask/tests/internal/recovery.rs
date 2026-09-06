use std::path::Path;

use super::recovery::recovery_command;

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(ToString::to_string).collect()
}

#[test]
fn runtime_stage_uses_private_verifier_and_forwards_the_global_frontend() {
    let frontend = Path::new("D:/前端 worktree");
    let (script, forwarded) = recovery_command(
        &strings(&["runtime", "verify", "--receipt", "runtime.json"]),
        frontend,
    )
    .unwrap();
    assert_eq!(script, "scripts/restore_runtime.py");
    assert_eq!(
        forwarded,
        strings(&[
            "verify",
            "--receipt",
            "runtime.json",
            "--frontend-dir",
            "D:/前端 worktree",
        ])
    );

    let (_, build) =
        recovery_command(&strings(&["runtime", "build", "--write"]), frontend).unwrap();
    assert_eq!(build, strings(&["build", "--write"]));
}

#[test]
fn reference_stages_keep_their_existing_arguments() {
    let arguments = strings(&["plan", "--id", "r1"]);
    let (script, forwarded) = recovery_command(&arguments, Path::new("unused")).unwrap();
    assert_eq!(script, "scripts/restore_reference.py");
    assert_eq!(forwarded, arguments);
}

#[test]
fn fresh_target_uses_the_private_state_machine_with_the_current_backend() {
    let (script, forwarded) = recovery_command(
        &strings(&[
            "fresh-target",
            "--workspace",
            "D:/隔离 target",
            "--operation",
            "status",
        ]),
        Path::new("unused"),
    )
    .unwrap();
    assert_eq!(script, "scripts/devex_clone.py");
    assert_eq!(
        forwarded,
        strings(&[
            "fresh-target",
            "--workspace",
            "D:/隔离 target",
            "--operation",
            "status",
            "--backend-dir",
        ])
        .into_iter()
        .chain([super::workspace::root_dir().display().to_string()])
        .collect::<Vec<_>>(),
    );
    assert!(
        recovery_command(
            &strings(&["fresh-target", "--backend-dir", "other"]),
            Path::new("unused"),
        )
        .is_err()
    );
}

#[test]
fn forwarded_recovery_scripts_exist_in_checkout() {
    let root = super::workspace::root_dir();
    for arguments in [
        strings(&["plan"]),
        strings(&["runtime", "status"]),
        strings(&[
            "fresh-target",
            "--operation",
            "status",
            "--workspace",
            "D:/target",
        ]),
    ] {
        let (script, _) = recovery_command(&arguments, Path::new("unused")).unwrap();
        assert!(
            root.join(script).is_file(),
            "恢复入口转发的脚本不在当前检出中：{script}"
        );
    }
}
