use std::{fs, path::PathBuf};

use serde_json::{Value, json};
use tempfile::TempDir;

use super::{FILES, output, write_or_match};
use crate::recovery::clone::native::{json as document, resources::common};

fn fixture() -> (TempDir, PathBuf, PathBuf) {
    let backend = crate::workspace::root_dir();
    let local = backend.join(".local-tests");
    fs::create_dir_all(&local).unwrap();
    let directory = tempfile::Builder::new()
        .prefix("source-publication-")
        .tempdir_in(local)
        .unwrap();
    let run = directory.path().join("run");
    fs::create_dir_all(run.join("seed-runtime")).unwrap();
    (directory, backend, run)
}

fn history(numbers: &[u64]) -> Value {
    json!({"attempts":numbers.iter().map(|number| json!({
        "number":number,"stage":"seed-runtime","mode":"source-register"
    })).collect::<Vec<_>>()})
}

#[test]
fn all_seven_prefixes_resume_the_unique_old_attempt_without_rewriting() {
    for length in 0..=FILES.len() {
        let (_directory, backend, run) = fixture();
        let old = run.join("seed-runtime/attempt-0001");
        fs::create_dir(&old).unwrap();
        for (index, name) in FILES.iter().take(length).enumerate() {
            document::write_new(&old.join(name), &json!({"index":index})).unwrap();
        }
        let before = FILES[..length]
            .iter()
            .map(|name| common::binding(&old.join(name)).unwrap())
            .collect::<Vec<_>>();
        let mut state = history(&[1, 2, 4, 5]);
        state["attempts"][1]["mode"] = json!("close");
        for number in [2, 4, 5] {
            let ignored = run.join(format!("seed-runtime/attempt-{number:04}"));
            fs::create_dir(&ignored).unwrap();
            fs::write(ignored.join("not-registration"), b"ignored").unwrap();
        }
        assert_eq!(output(&backend, &run, &state, 4).unwrap(), old);
        for (index, name) in FILES.iter().enumerate() {
            let descriptor = write_or_match(&backend, &old, name, &json!({"index":index})).unwrap();
            assert_eq!(descriptor, common::binding(&old.join(name)).unwrap());
            if index < length {
                assert_eq!(descriptor, before[index]);
            }
        }
    }
}

#[test]
fn invalid_prefix_unknown_entry_nonfile_and_multiple_candidates_preserve_evidence() {
    for invalid in ["gap", "unknown", "nonfile", "multiple"] {
        let (_directory, backend, run) = fixture();
        let old = run.join("seed-runtime/attempt-0001");
        fs::create_dir(&old).unwrap();
        match invalid {
            "gap" => document::write_new(&old.join(FILES[1]), &json!({"saved":true})).unwrap(),
            "unknown" => fs::write(old.join("unknown.json"), b"original").unwrap(),
            "nonfile" => fs::create_dir(old.join(FILES[0])).unwrap(),
            "multiple" => fs::create_dir(run.join("seed-runtime/attempt-0002")).unwrap(),
            _ => unreachable!(),
        }
        assert!(
            output(&backend, &run, &history(&[1, 2]), 3).is_err(),
            "{invalid}"
        );
        assert!(old.is_dir());
        assert!(!run.join("seed-runtime/attempt-0003").exists());
        if invalid == "unknown" {
            assert_eq!(fs::read(old.join("unknown.json")).unwrap(), b"original");
        }
    }
}

#[test]
fn new_output_requires_existing_parent_and_never_overwrites_current_attempt() {
    let (_directory, backend, run) = fixture();
    let path = output(&backend, &run, &history(&[1]), 2).unwrap();
    assert_eq!(path, run.join("seed-runtime/attempt-0002"));
    let binding = write_or_match(&backend, &path, FILES[0], &json!({"saved":true})).unwrap();
    assert!(output(&backend, &run, &history(&[]), 2).is_err());
    assert_eq!(common::binding(&path.join(FILES[0])).unwrap(), binding);
    let missing = run.join("other-run");
    fs::create_dir(&missing).unwrap();
    assert!(output(&backend, &missing, &history(&[]), 1).is_err());
    assert!(!missing.join("seed-runtime").exists());
}

#[test]
fn matching_json_preserves_raw_bytes_while_conflicting_or_duplicate_json_is_rejected() {
    let (_directory, backend, run) = fixture();
    let path = output(&backend, &run, &history(&[]), 1).unwrap();
    let file = path.join(FILES[0]);
    let raw = b" { \"second\": true, \"first\": 1 } \n";
    fs::write(&file, raw).unwrap();
    let before = common::binding(&file).unwrap();
    assert_eq!(
        write_or_match(&backend, &path, FILES[0], &json!({"first":1,"second":true})).unwrap(),
        before
    );
    assert!(write_or_match(&backend, &path, FILES[0], &json!({"first":2})).is_err());
    assert_eq!(fs::read(&file).unwrap(), raw);
    let duplicate = b"{\"first\":1,\"first\":1}";
    fs::write(&file, duplicate).unwrap();
    assert!(write_or_match(&backend, &path, FILES[0], &json!({"first":1})).is_err());
    assert_eq!(fs::read(&file).unwrap(), duplicate);
    assert!(!path.join(FILES[1]).exists());
}

#[test]
fn ordered_publication_keeps_two_mib_body_and_checks_reader_budget_before_creation() {
    let (_directory, backend, run) = fixture();
    let path = output(&backend, &run, &history(&[]), 1).unwrap();
    assert!(write_or_match(&backend, &path, FILES[1], &json!({})).is_err());
    assert!(write_or_match(&backend, &path, "../unknown", &json!({})).is_err());
    assert!(write_or_match(&backend, &path, FILES[0], &json!([])).is_err());
    assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
    let value = json!({"body":"x".repeat(2 * 1024 * 1024)});
    let descriptor = write_or_match(&backend, &path, FILES[0], &value).unwrap();
    assert_eq!(document::read(&path.join(FILES[0])).unwrap(), value);
    assert!(descriptor["bytes"].as_u64().unwrap() > 2 * 1024 * 1024);
    assert_eq!(descriptor, common::binding(&path.join(FILES[0])).unwrap());
    assert_eq!(descriptor.as_object().unwrap().len(), 3);
    assert!(descriptor.get("path").is_some());
    assert!(descriptor.get("sha256").is_some());
    assert!(serde_json::to_vec(&descriptor).unwrap().len() < 1024);
    let oversized = json!({"body":"x".repeat(document::MAX_JSON_BYTES as usize)});
    assert!(write_or_match(&backend, &path, FILES[1], &oversized).is_err());
    let mut unreadable = json!({});
    for _ in 0..140 {
        unreadable = json!({"nested":unreadable});
    }
    assert!(write_or_match(&backend, &path, FILES[1], &unreadable).is_err());
    assert!(!path.join(FILES[1]).exists());
    assert_eq!(common::binding(&path.join(FILES[0])).unwrap(), descriptor);
    fs::write(path.join("untracked"), b"retain").unwrap();
    assert!(write_or_match(&backend, &path, FILES[1], &json!({})).is_err());
    assert_eq!(fs::read(path.join("untracked")).unwrap(), b"retain");
    assert!(!path.join(FILES[1]).exists());
}
