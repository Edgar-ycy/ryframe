use super::process;
use std::{
    fs,
    io::{Cursor, Read},
    sync::mpsc,
    thread,
};

#[test]
fn frontend_logs_redact_bound_secrets_and_keep_bounded_failure_evidence() {
    let directory = super::tests::temporary();
    for overflow in [false, true] {
        let (tx, rx) = mpsc::channel();
        let reader = thread::spawn(move || {
            let prefix = Cursor::new(b"reason SyntheticPassword".to_vec());
            let suffix = Cursor::new(if overflow { vec![b'x'; 64] } else { vec![] });
            tx.send(process::capture(prefix.chain(suffix), 30)).unwrap();
        });
        let log = directory.path().join(format!("{overflow}.log"));
        let result = process::finish_log(rx, reader, &log, &["SyntheticPassword".into()]);
        assert_eq!(result.is_err(), overflow);
        let saved = fs::read_to_string(log).unwrap();
        assert!(saved.starts_with("reason [REDACTED]"));
        assert!(!saved.contains("SyntheticPassword"));
    }
    let (captured, overflow) = process::capture(Cursor::new(b"complete"), 8).unwrap();
    assert_eq!(captured, b"complete");
    assert!(!overflow);
}

#[test]
fn frontend_handoff_rejects_rewrite_or_replacement_before_supervisor_claim() {
    use super::dependency::FrontendCommand;
    use serde_json::json;
    let temporary = super::tests::temporary();
    let backend = crate::workspace::root_dir();
    for replace in [false, true] {
        let directory = temporary.path().join(format!("command-{replace}"));
        let log = temporary.path().join(format!("command-{replace}.log"));
        let (guard, _held) = super::frontend::publish(
            &backend,
            &FrontendCommand {
                binding: &json!({}),
                arguments: &json!([]),
                environment: &json!({}),
                log: &log,
                directory: &directory,
                timeout: 10,
                secrets: &[],
            },
        )
        .unwrap();
        super::frontend::handoff(&directory, &guard).unwrap();
        let path = directory.join("runtime.json");
        let bytes = fs::read(&path).unwrap();
        if replace {
            fs::rename(&path, directory.join("original.json")).unwrap();
            fs::write(&path, &bytes).unwrap();
        } else {
            let mut changed = bytes;
            changed.push(b' ');
            fs::write(&path, changed).unwrap();
        }
        let request = json!({"format_version":1,"kind":"ryframe-full-stack-process",
            "operation":"run-frontend","arguments":{"backend":backend,"directory":directory,"request_binding":guard}});
        assert!(
            crate::recovery::full_stack::worker::execute(&serde_json::to_vec(&request).unwrap())
                .is_err()
        );
        for name in [
            "frontend-control.guard",
            "frontend-control.lock",
            "frontend-tree.json",
            "frontend.json",
        ] {
            assert!(!directory.join(name).exists());
        }
    }
}
