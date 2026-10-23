use super::{
    dependency::{Dependencies, FrontendCommand, Python},
    model::*,
};
use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

#[test]
fn supervised_commands_bind_completion_preserve_failure_and_reclaim_timeout_tree() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut api = Python::new(root).unwrap();
    let output = Command::new(&api.executable)
        .args(["-c", "import sys;print(sys.executable)"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let executable = String::from_utf8(output.stdout).unwrap().trim().to_owned();
    let temp = super::tests::temporary();
    let frontend = temp.path().join("frontend");
    fs::create_dir(&frontend).unwrap();
    let script = frontend.join("pnpm");
    fs::write(&script,b"import sys,subprocess,time\nfrom pathlib import Path\nprint('Private'+'Secret',flush=True)\ntime.sleep(0.3)\nif 'corrupt' in sys.argv: (Path(sys.argv[-1])/'frontend.json').unlink()\nif 'timeout' in sys.argv:\n p=subprocess.Popen([sys.executable,'-c','import time;time.sleep(60)'])\n print(p.pid,flush=True)\n time.sleep(60)\nif 'fail' in sys.argv: sys.exit(17)\n").unwrap();
    let binding = json!({"scope_id":"fixture-browser-process","frontend":{"path":frontend},"tools":{"corepack":{"path":executable},"node":{"path":executable},"corepack_entry":{"path":script}}});
    for (mode, timeout) in [
        ("success", 20),
        ("fail", 20),
        ("timeout", 3),
        ("corrupt", 20),
    ] {
        let directory = temp.path().join(mode);
        let log = temp.path().join(format!("{mode}.log"));
        let result = api.frontend(FrontendCommand {
            binding: &binding,
            arguments: &json!([mode, directory]),
            environment: &environment(),
            log: &log,
            directory: &directory,
            timeout,
            secrets: &["PrivateSecret".into()],
        });
        let bytes = fs::read(&log).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("PrivateSecret"));
        assert!(String::from_utf8_lossy(&bytes).contains("[REDACTED]"));
        if mode == "success" {
            let result = result.unwrap();
            descriptor(
                &result["completion"],
                &path(&result["completion"]["path"]).unwrap(),
            )
            .unwrap();
        } else {
            let error = result.unwrap_err();
            if mode == "fail" {
                assert_eq!(crate::process::failure_exit_code(error.as_ref()), Some(17));
            }
            if mode == "corrupt" {
                assert_ne!(crate::process::failure_exit_code(error.as_ref()), Some(0));
            }
            let (tree, _) = document(&directory.join("frontend-tree.json")).unwrap();
            let proof = directory.join(format!(
                "frontend-members-{}-stopped.json",
                tree["operation_id"].as_str().unwrap()
            ));
            assert_eq!(document(&proof).unwrap().0["status"], "stopped");
        }
    }
}
fn environment() -> Value {
    let mut values = serde_json::Map::new();
    for (key, value) in std::env::vars() {
        if [
            "PATH",
            "PATHEXT",
            "SYSTEMROOT",
            "WINDIR",
            "COMSPEC",
            "HOME",
            "USERPROFILE",
            "HOMEDRIVE",
            "HOMEPATH",
            "TEMP",
            "TMP",
            "APPDATA",
            "LOCALAPPDATA",
        ]
        .contains(&key.to_uppercase().as_str())
        {
            values.insert(key, json!(value));
        }
    }
    Value::Object(values)
}
