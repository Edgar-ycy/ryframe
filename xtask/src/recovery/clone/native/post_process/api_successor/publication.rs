//! 从已通过的阶段读取对应的 API 发布及其不可变接续证明。

use super::{
    Post, Result, attempt_number, bound_at, common, document, fs, history, original_executor,
    read_at, state, verify_generation, verify_incident,
};
use serde_json::{Value, json};
use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

#[path = "publication/incident.rs"]
mod incident;

#[derive(Debug, PartialEq)]
pub(in crate::recovery::clone::native) struct Selected {
    pub(in crate::recovery::clone::native) publication: Value,
    pub(in crate::recovery::clone::native) binding: Value,
    pub(in crate::recovery::clone::native) passed_attempt: u64,
    pub(in crate::recovery::clone::native) restart_attempt: u64,
}

pub(in crate::recovery::clone::native) fn select(
    backend: &Path,
    run: &Path,
    registration: &Value,
) -> Result<Option<Selected>> {
    select_at(backend, run, registration, None)
}

pub(in crate::recovery::clone::native) fn select_before(
    backend: &Path,
    run: &Path,
    registration: &Value,
    exclusive_attempt: u64,
) -> Result<Option<Selected>> {
    select_at(backend, run, registration, Some(exclusive_attempt))
}

/// 按正式账本核验并返回每个已通过 API 后继的运行证据。
pub(in crate::recovery::clone::native) fn runtime_history(
    backend: &Path,
    run: &Path,
    registration: &Value,
) -> Result<Vec<Value>> {
    Ok(verify_at(backend, run, registration, None)?
        .into_iter()
        .flatten()
        .map(|generation| generation.runtime)
        .collect())
}

struct Verified {
    selected: Selected,
    result: Value,
    intent: Value,
    runtime: Value,
}

fn select_at(
    backend: &Path,
    run: &Path,
    registration: &Value,
    exclusive_attempt: Option<u64>,
) -> Result<Option<Selected>> {
    Ok(verify_at(backend, run, registration, exclusive_attempt)?
        .into_iter()
        .flatten()
        .next_back()
        .map(|generation| generation.selected))
}

fn verify_at(
    backend: &Path,
    run: &Path,
    registration: &Value,
    exclusive_attempt: Option<u64>,
) -> Result<[Option<Verified>; 2]> {
    let saved = state::load(run, false)?;
    let attempts = state::attempts(&saved)?;
    let parent = run.join("post-copy").join("api-successors");
    let [first, second] = completed(attempts, exclusive_attempt)?;
    let verified = if let Some(first) = first {
        let post = history::post(backend, run, Some(registration), None, false)?;
        let first = verify(backend, run, &parent, attempts, &post, first, None)?;
        let second = second
            .map(|row| verify(backend, run, &parent, attempts, &post, row, Some(&first)))
            .transpose()?;
        if exclusive_attempt.is_none() {
            check_paths(backend, &parent, true, second.as_ref())?;
        }
        [Some(first), second]
    } else {
        if exclusive_attempt.is_none() {
            check_paths(backend, &parent, false, None)?;
        }
        [None, None]
    };
    if state::load(run, false)? != saved {
        return Err("API 后继账本在读取期间发生变化".into());
    }
    Ok(verified)
}

fn completed(attempts: &[Value], exclusive_attempt: Option<u64>) -> Result<[Option<&Value>; 2]> {
    let mut passed = attempts.iter().filter(|row| {
        row["stage"] == "runtime-target"
            && matches!(row["mode"].as_str(), Some("restart-api" | "reconcile-api"))
            && row["status"] == "passed"
            && exclusive_attempt
                .is_none_or(|limit| row["number"].as_u64().is_some_and(|number| number < limit))
    });
    let generations = [passed.next(), passed.next()];
    if passed.next().is_some() {
        return Err("API 后继账本含未经登记的额外代次".into());
    }
    Ok(generations)
}

fn verify(
    backend: &Path,
    run: &Path,
    parent: &Path,
    attempts: &[Value],
    post: &Post,
    completed: &Value,
    predecessor: Option<&Verified>,
) -> Result<Verified> {
    let passed_attempt = attempt_number(completed, "number")?;
    let result_binding = &completed["result"];
    let result = read_at(
        backend,
        result_binding,
        &state::result_path(run, passed_attempt),
    )?;
    document::exact(
        &result,
        &["status", "publication", "restore_qualified"],
        "API 后继阶段结果",
    )?;
    if result["status"] != "api_successor_ready" || result["restore_qualified"] != false {
        return Err("API 后继阶段未发布受管启动结果".into());
    }
    let binding = result["publication"].clone();
    let path = publication_path(parent, &binding, predecessor.is_some())?;
    let publication = read_at(backend, &binding, &path)?;
    verify_shape(&publication, post)?;
    let intent_path = path.with_file_name("intent.json");
    let intent = read_at(backend, &publication["intent"], &intent_path)?;
    let (restart_attempt, executor) = original_executor(
        run,
        attempts,
        completed,
        post,
        &intent,
        predecessor.is_some(),
    )?;
    let directory = parent.join(format!("attempt-{restart_attempt:04}"));
    if publication["runtime_directory"] != json!(directory)
        || intent["runtime_directory"] != publication["runtime_directory"]
        || common::local_directory(backend, &directory, "API 后继运行目录")? != directory
        || predecessor.is_some() && path != directory.join("published.json")
    {
        return Err("API 后继运行目录不属于原启动意图".into());
    }
    if let Some(previous) = predecessor {
        incident::verify_second(backend, restart_attempt, &intent, previous)?;
    } else {
        verify_incident(backend, attempts, restart_attempt, post, &intent)?;
    }
    let runtime = verify_generation(backend, post, &directory, &publication, &intent, &executor)?;
    if read_at(backend, &binding, &path)? != publication
        || read_at(backend, &publication["intent"], &intent_path)? != intent
    {
        return Err("API 后继发布在读取期间发生变化".into());
    }
    bound_at(
        backend,
        result_binding,
        &state::result_path(run, passed_attempt),
    )?;
    Ok(Verified {
        selected: Selected {
            publication,
            binding,
            passed_attempt,
            restart_attempt,
        },
        result: result_binding.clone(),
        intent,
        runtime,
    })
}

fn verify_shape(publication: &Value, post: &Post) -> Result<()> {
    document::exact(
        publication,
        &[
            "format_version",
            "kind",
            "registration",
            "intent",
            "runtime_directory",
            "runtime",
            "binaries",
            "api",
            "api_tree",
        ],
        "API 后继发布",
    )?;
    if publication["format_version"] != 1
        || publication["kind"] != "devex-clone-api-successor"
        || publication["registration"] != post.descriptor
    {
        return Err("API 后继发布不属于当前登记".into());
    }
    Ok(())
}

fn publication_path(parent: &Path, binding: &Value, second: bool) -> Result<PathBuf> {
    let path = Path::new(document::text(&binding["path"], "API 后继发布路径")?);
    if second {
        let directory = path.parent().ok_or("第二代 API 发布缺少原启动目录")?;
        let number = directory
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix("attempt-"))
            .and_then(|number| number.parse::<u64>().ok())
            .filter(|number| *number > 0)
            .ok_or("第二代 API 发布缺少有效原启动编号")?;
        if path == parent.join(format!("attempt-{number:04}/published.json")) {
            return Ok(path.to_owned());
        }
    } else if path == parent.join("published.json") {
        return Ok(path.to_owned());
    }
    Err("API 发布路径与该代次的正式布局不同".into())
}

fn check_paths(
    backend: &Path,
    parent: &Path,
    first_present: bool,
    second: Option<&Verified>,
) -> Result<()> {
    let second = second
        .map(|value| publication_path(parent, &value.selected.binding, true))
        .transpose()?;
    if present(&parent.join("published.json"))? != first_present {
        return Err("API 后继根发布缺少对应的已通过阶段".into());
    }
    if !present(parent)? {
        return Ok(());
    }
    common::local_directory(backend, parent, "API 后继发布目录")?;
    check_children(parent, second.as_deref())
}

fn check_children(parent: &Path, second: Option<&Path>) -> Result<()> {
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        if !entry
            .file_name()
            .to_string_lossy()
            .to_ascii_lowercase()
            .starts_with("attempt-")
        {
            continue;
        }
        let path = parent.join(entry.file_name()).join("published.json");
        if present(&path)? && second != Some(path.as_path()) {
            return Err("后继 API 运行目录有未提交或重复的发布".into());
        }
    }
    Ok(())
}

fn present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
#[test]
fn api_successor_selects_only_publications_completed_before_the_cutoff() -> Result<()> {
    let rows = [
        json!({"number":55,"stage":"runtime-target","mode":"restart-api","status":"passed"}),
        json!({"number":65,"stage":"seed-runtime","mode":"identities-verify","status":"failed"}),
        json!({"number":66,"stage":"runtime-target","mode":"restart-api","status":"failed"}),
        json!({"number":67,"stage":"runtime-target","mode":"reconcile-api","status":"passed"}),
    ];
    assert_eq!(completed(&rows, Some(55))?, [None, None]);
    assert_eq!(completed(&rows, Some(67))?, [Some(&rows[0]), None]);
    assert_eq!(completed(&rows, None)?, [Some(&rows[0]), Some(&rows[3])]);
    let mut repeated = rows.to_vec();
    repeated.push(rows[3].clone());
    assert!(completed(&repeated, None).is_err());
    Ok(())
}

#[cfg(test)]
#[test]
fn api_successor_refuses_uncommitted_or_misplaced_publications() -> Result<()> {
    let temporary = tempfile::tempdir()?;
    let parent = temporary.path();
    let first = parent.join("published.json");
    let second = parent.join("attempt-0066").join("published.json");
    let unrelated = parent.join("attempt-0067").join("published.json");
    let bound = |path: &Path| json!({"path":path});
    assert_eq!(publication_path(parent, &bound(&first), false)?, first);
    assert!(publication_path(parent, &bound(&first), true).is_err());
    assert!(publication_path(parent, &bound(&second), false).is_err());
    assert_eq!(publication_path(parent, &bound(&second), true)?, second);
    assert!(
        publication_path(
            parent,
            &bound(&parent.join("attempt-66/published.json")),
            true
        )
        .is_err()
    );
    fs::create_dir(second.parent().unwrap())?;
    fs::write(&second, b"{}")?;
    assert!(check_children(parent, None).is_err());
    check_children(parent, Some(&second))?;
    fs::create_dir(unrelated.parent().unwrap())?;
    fs::write(unrelated, b"{}")?;
    assert!(check_children(parent, Some(&second)).is_err());
    Ok(())
}
