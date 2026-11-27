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

#[test]
fn both_api_successor_generations_reach_lineage_identities_in_runtime_order() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let backend = temporary.path().canonicalize()?;
    let run = backend.join(".local-tests/完整历史运行");
    let create_runtime = |relative: &str, pid: u64, started: &str, role: &str| -> Result<Value> {
        let directory = run.join(relative);
        std::fs::create_dir_all(&directory)?;
        Ok(json!({"runtime_directory":directory,"generations":[
            {"role":role,"sequence":1,"identity":{
                "pid":pid,"started":started,"executable":backend.join(format!("{role}.exe"))
            }}
        ]}))
    };
    let post = create_runtime("post-copy/runtime-0001", 201, "10", "api")?;
    let first = create_runtime("post-copy/api-successors/attempt-0055", 202, "20", "api")?;
    let second = create_runtime("post-copy/api-successors/attempt-0067", 202, "30", "api")?;
    let seed = create_runtime("seed-runtime/runtime", 203, "40", "worker")?;
    let (runtimes, identities) = merge_runtime_history(
        &backend,
        &run,
        vec![post.clone(), second.clone(), seed.clone(), first.clone()],
    )?;
    assert_eq!(runtimes, [first.clone(), second.clone(), post, seed]);
    let identities = identities.into_value();
    assert_eq!(identities.as_array().unwrap().len(), 4);
    for (index, expected) in [first, second].into_iter().enumerate() {
        let item = &identities[index + 1];
        assert_eq!(item["identity"], expected["generations"][0]["identity"]);
        assert_eq!(
            item["occurrences"],
            json!([{"kind":"runtime","runtime_directory":expected["runtime_directory"],
                "role":"api","sequence":1}])
        );
    }
    assert!(merge_runtime_history(&backend, &run, vec![runtimes[0].clone(); 2]).is_err());
    Ok(())
}
