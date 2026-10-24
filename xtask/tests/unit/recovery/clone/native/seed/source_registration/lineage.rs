use super::*;

fn attempt(stage: &str, mode: &str, status: &str) -> Value {
    json!({"stage":stage,"mode":mode,"status":status})
}

#[test]
fn complete_runtime_chain_keeps_every_published_amendment_in_order() {
    let run = std::env::current_dir().unwrap().join("完整历史运行");
    let mut attempts = vec![attempt("seed-runtime", "prepare", "passed")];
    attempts.extend((0..12).map(|_| attempt("post-copy", "amend", "passed")));
    attempts.push(attempt("post-copy", "amend", "failed"));
    let saved = json!({"attempts":attempts});
    let base = run.join("post-copy");
    let actual = runtime_paths(&run, &saved, &base.join("runtime-0012")).unwrap();
    let mut expected = vec![base.join("runtime")];
    expected.extend((1..=12).map(|number| base.join(format!("runtime-{number:04}"))));
    expected.push(run.join("seed-runtime/runtime"));
    assert_eq!(actual, expected);
}

#[test]
fn active_runtime_must_cover_all_successful_amendments() {
    let run = std::env::current_dir().unwrap().join("runtime-chain");
    let saved = json!({"attempts":[attempt("post-copy", "amend", "passed")]});
    assert!(runtime_paths(&run, &saved, &run.join("post-copy/runtime")).is_err());
    let duplicate = json!({"attempts":[
        attempt("seed-runtime", "prepare", "passed"),
        attempt("seed-runtime", "prepare", "passed")
    ]});
    assert!(runtime_paths(&run, &duplicate, &run.join("post-copy/runtime")).is_err());
}
