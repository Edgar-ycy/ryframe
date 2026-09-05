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
