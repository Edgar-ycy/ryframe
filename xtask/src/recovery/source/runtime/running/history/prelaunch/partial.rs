//! 核验已审计的原启动失败、两个未消费预留及全部落盘文件。
use super::{RECOVER, START, absent_controller, audited, failed, missing, read_controller};
use crate::{
    Result,
    local_test_path::validate_external_existing_file,
    recovery::{
        clone::native::{
            digest, json as document, resources::common, seed::generation::coordinator,
        },
        source::evidence::{
            inventory, running::history::lineage::failure::inputs::exact_directory,
        },
    },
};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

const ATTEMPT: u64 = 84;
const STARTED: &str = "2026-11-27T01:45:36.902600+00:00";
const FINISHED: &str = "2026-11-27T01:51:40.664151+00:00";
const HEAD: &str = "40638110f3fb8c84c25afbb49d5dc3ae3d697c85";
const TREE: &str = "cb39ee7be9500e07728e3e7150bcc3d9943b5462";
const FINGERPRINT: &str = "sha256:d7c6e21ebf3a45c3272427797699fc3e183a24f44dbed6680ee52c99519f30aa";
const CONTROLLER_SHA: &str = "f2eaf50ccf1b4604507d8baaa9b517a81cf56380d5fba0a1db3c6fcc389297cf";
const FAILURE_SHA: &str = "85decaf7d2737fe9ecfcb010f5a71eea9a64c625b6fc42264e4b40401941e7ca";
const EXECUTABLE_SHA: &str = "6241cc3ae3f34be7f93597143cb6ada47bbca725f4c4d8099a5c03f7eb05ca21";
const REQUEST_NAME: &str = "source-generation-request-wkc61111h-20261127-r2.json";
const REQUEST_SHA: &str = "3a843b4a36b27b6e4f07c051c22ff59dd5dc66f402ecf9d9c752e00692f44e07";

const CAPTURE_ATTEMPT: u64 = 85;
const CAPTURE_STARTED: &str = "2026-11-27T02:18:23.282475+00:00";
const CAPTURE_FINISHED: &str = "2026-11-27T02:27:15.457034+00:00";
const CAPTURE_CONTROLLER: &str = "51c5dca0c73565e43fce78f31fcb2c9a9ad48a9f753c94a5a2590a480299f4a9";
const CAPTURE_FAILURE: &str = "b378a5b75fd4b36a081aab99de4a027e623891eb47bcfdfc39bc241c8e87eeae";
const CAPTURE_EXECUTABLE: &str = "2a2f818d03da9de56941fe198c8dee6f4ef5031152d5e02ad99be330eaf28172";
const CAPTURE_LAYOUT: &str = "storage-layout-4f33112b6fa346e58ac8aba2f12b26b4.json";

const FILES: &[(&str, u64, &str)] = &[
    (
        "before/storage-layout-6f817bba7b374f81a8d1e20e5b40f0dd.json",
        3277,
        "3ef749eef47e2a441f4e95aea2bdab269d84b5503a2ffcf0595e671082ad5932",
    ),
    (
        "intent.json",
        699,
        "787e62dabb5dcaea3003a6d15097f6fce93f24a5d933c6467b1bab4327970cab",
    ),
    ("request.json", 5604, REQUEST_SHA),
    (
        "runtime-configuration/config/app.test.toml",
        2532,
        "ee6d7b98162a23b86e30c42913ac1f7ca32e1c11563a910f525347274c577a94",
    ),
    (
        "runtime-configuration/config/app.toml",
        9295,
        "ad9a785bcbb5c7f95fb6117653e75b3ec783a98709cac9d5c8860e09d8d0b604",
    ),
    (
        "runtime-configuration/worker-config/app.test.toml",
        2532,
        "ce850634f534e41eeafd1897a2546651c3f73d0f565bb48368be8d4c2e2e9741",
    ),
    (
        "runtime-configuration/worker-config/app.toml",
        9295,
        "ad9a785bcbb5c7f95fb6117653e75b3ec783a98709cac9d5c8860e09d8d0b604",
    ),
    (
        "runtime/binaries.json",
        989,
        "17941e549b33ae09e28ed5061b535a4d8980077ac8c130b64b23f5f5fdbf01ae",
    ),
    (
        "runtime/runtime.json",
        1765,
        "8eff5a89b07483d383f8915a387d6b06249fa3a812e980403b258d2ebb836189",
    ),
];

const CAPTURE_FILES: &[(&str, u64, &str)] = &[
    (
        "before/storage-layout-4f33112b6fa346e58ac8aba2f12b26b4.json",
        3277,
        "3ef749eef47e2a441f4e95aea2bdab269d84b5503a2ffcf0595e671082ad5932",
    ),
    FILES[3],
    FILES[4],
    FILES[5],
    FILES[6],
    FILES[7],
    (
        "runtime/runtime.json",
        1807,
        "d1e71818eb35186504e5399fca7374b4041092f5dcbeb82392b2e9164d8c8e53",
    ),
];

pub(in crate::recovery::source::evidence) fn applies(start: &Value) -> bool {
    start["number"] == ATTEMPT
        && start["started_at"] == STARTED
        && start["finished_at"] == FINISHED
        && start["sources"]["snapshot"]["head"] == HEAD
        && start["sources"]["worktree_fingerprint"] == FINGERPRINT
        && failed(start, START, "StageError").is_ok()
}

pub(in crate::recovery::source::evidence) fn applies_recovery_failure(recover: &Value) -> bool {
    recover["number"] == CAPTURE_ATTEMPT
        && recover["started_at"] == CAPTURE_STARTED
        && recover["finished_at"] == CAPTURE_FINISHED
        && recover["sources"]["snapshot"]["head"] == HEAD
        && recover["sources"]["worktree_fingerprint"] == FINGERPRINT
        && failed(recover, RECOVER, "StageError").is_ok()
}

pub(in crate::recovery::source::evidence) fn expected_request_path(run: &Path) -> Result<PathBuf> {
    let parent = run
        .parent()
        .and_then(Path::parent)
        .ok_or("部分输出恢复缺少原请求目录")?;
    Ok(parent.join(REQUEST_NAME))
}

pub(in crate::recovery::source::evidence) fn recovery_history(
    backend: &Path,
    run: &Path,
    prefix: &[Value],
    number: u64,
    request: &Value,
) -> Result<(Value, Value, Value)> {
    let (start, recovery) = history_tail(prefix, number)?;
    let original = verify(backend, run, start, request)?;
    let collection = match recovery {
        Some(recover) => capture_failure(backend, run, recover, start, request)?,
        None => Value::Null,
    };
    Ok((start.clone(), original, collection))
}

fn history_tail(prefix: &[Value], number: u64) -> Result<(&Value, Option<&Value>)> {
    match prefix {
        [.., start] if number == ATTEMPT + 1 && applies(start) => Ok((start, None)),
        [.., start, recover]
            if number == CAPTURE_ATTEMPT + 1
                && applies(start)
                && applies_recovery_failure(recover)
                && recover["sources"] == start["sources"] =>
        {
            Ok((start, Some(recover)))
        }
        _ => Err("部分启动前恢复只能紧接原失败或唯一已审计的只读采集失败，禁止重放".into()),
    }
}

pub(in crate::recovery::source::evidence) fn recovery_failure(
    backend: &Path,
    run: &Path,
    failed_recover: &Value,
    start: &Value,
    request: &Value,
) -> Result<Value> {
    verify(backend, run, start, request)?;
    capture_failure(backend, run, failed_recover, start, request)
}

fn capture_failure(
    backend: &Path,
    run: &Path,
    recover: &Value,
    start: &Value,
    request: &Value,
) -> Result<Value> {
    if !applies_recovery_failure(recover) || recover["sources"] != start["sources"] {
        return Err("部分启动前恢复缺少同源的已审计只读采集失败".into());
    }
    let (controller, owner) = read_controller(run, recover)?;
    if controller["sha256"] != CAPTURE_CONTROLLER {
        return Err("原失败采集的控制器摘要不同".into());
    }
    absent_controller(&owner)?;
    let failure = common::binding(&run.join(format!("failure-{CAPTURE_ATTEMPT:04}.json")))?;
    let actual = common::read_bound(backend, &failure, "原生只读采集失败")?;
    if failure["sha256"] != CAPTURE_FAILURE
        || actual
            != json!({"format_version":2,"kind":"devex-stage-failure","attempt":CAPTURE_ATTEMPT,
                "stage":"seed-runtime","mode":RECOVER,"operation_id":"clone.seed-runtime.source-generation-recover",
                "error_type":"StageError","controller":controller})
    {
        return Err("原只读采集失败不属于已审计的原生阶段收据".into());
    }
    let launch = capture_launch(backend, run, request, &owner)?;
    let output = run.join(format!("seed-runtime/attempt-{CAPTURE_ATTEMPT:04}"));
    let files = output_files(backend, &output, &[], CAPTURE_LAYOUT, CAPTURE_FILES)?;
    let runtime = common::binding(&output.join("runtime/runtime.json"))?;
    missing(
        backend,
        &run.join(format!("results/{CAPTURE_ATTEMPT:04}.json")),
    )?;
    missing(backend, &run.join(format!("g{CAPTURE_ATTEMPT:04}")))?;
    if read_controller(run, recover)? != (controller.clone(), owner.clone())
        || output_files(backend, &output, &[], CAPTURE_LAYOUT, CAPTURE_FILES)? != files
    {
        return Err("原失败采集的控制器或全部输出在核验期间变化".into());
    }
    common::bound_file(backend, &failure, "原生只读采集失败")?;
    common::bound_file(backend, request, "原只读采集显式请求")?;
    absent_controller(&owner)?;
    Ok(
        json!({"attempt":digest::plan_hash(recover)?,"start":digest::plan_hash(start)?,
        "request":request,"controller":controller,"failure":failure,"runtime":runtime,"files":files,
        "executable":launch["executable"],"invocation":launch["invocation"],"exit":launch["exit"],
        "stdout":launch["stdout"],"stderr":launch["stderr"]}),
    )
}

fn capture_launch(backend: &Path, run: &Path, request: &Value, owner: &Value) -> Result<Value> {
    let tool = Path::new(document::text(
        &owner["identity"]["executable"],
        "原只读采集程序",
    )?);
    let root = tool.parent().ok_or("原只读采集程序缺少父目录")?;
    if tool.file_name().and_then(|name| name.to_str()) != Some("frozen-controller.exe")
        || root.file_name().and_then(|name| name.to_str())
            != Some("20261127-h-source-generation-recover-0085-r2")
    {
        return Err("原只读采集程序不属于已登记的 R2 目录".into());
    }
    let executable = external_file(tool, 75_099_136, CAPTURE_EXECUTABLE)?;
    let launch = root.join("intent.json");
    let invocation = external_file(
        &launch,
        1569,
        "e9819c3768a062bd7d4e93337be17a2fa90cabb3ec746e8aeb2a10e4605c1050",
    )?;
    let end = root.join("exit.json");
    let exit = external_file(
        &end,
        451,
        "8399c95024b5a76b6465cd3f0a55f37bf30305a8ee0bced02a3262321b9df750",
    )?;
    let stdout = external_file(
        &root.join("recovery.stdout"),
        0,
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
    )?;
    let stderr = external_file(
        &root.join("recovery.stderr"),
        58,
        "d1ab1ab0728ef0fb201963a937a6c7694de3967d41c2a7a89ed1610f524ba7ca",
    )?;
    let command = document::read(&launch)?;
    let terminal = document::read(&end)?;
    if command["Cwd"] != json!(backend)
        || command["Run"] != json!(run)
        || command["ExpectedAttempt"] != CAPTURE_ATTEMPT
        || command["Tool"]["path"] != executable["path"]
        || !same_digest(&command["Tool"]["sha256"], &executable["sha256"])
        || command["Request"]["path"] != request["path"]
        || !same_digest(&command["Request"]["sha256"], &request["sha256"])
        || command["Arguments"]
            != json!([
                "check",
                "recovery",
                "clone",
                "seed-runtime",
                "--run-dir",
                run,
                "--operation",
                RECOVER,
                "--request",
                request["path"],
                "--write"
            ])
        || terminal["Attempt"] != CAPTURE_ATTEMPT
        || terminal["Pid"] != owner["identity"]["pid"]
        || terminal["Status"] != "failed"
        || terminal["ExitCode"] != 1
        || !terminal["Result"].is_null()
        || !same_digest(&terminal["StdoutSha"], &stdout["sha256"])
        || !same_digest(&terminal["StderrSha"], &stderr["sha256"])
        || common::binding(&launch)? != invocation
        || common::binding(&end)? != exit
    {
        return Err("原只读采集启动、请求或外层退出结果与已登记证据不符".into());
    }
    Ok(
        json!({"executable":executable,"invocation":invocation,"exit":exit,"stdout":stdout,"stderr":stderr}),
    )
}

fn external_file(path: &Path, bytes: u64, sha256: &str) -> Result<Value> {
    validate_external_existing_file(path)?;
    let descriptor = common::binding(path)?;
    if descriptor["bytes"] != bytes || descriptor["sha256"] != sha256 {
        return Err("原只读采集程序或外层收据摘要不同".into());
    }
    Ok(descriptor)
}

fn same_digest(a: &Value, b: &Value) -> bool {
    a.as_str()
        .zip(b.as_str())
        .is_some_and(|(a, b)| a.eq_ignore_ascii_case(b))
}

pub(in crate::recovery::source::evidence) fn verify(
    backend: &Path,
    run: &Path,
    start: &Value,
    request: &Value,
) -> Result<Value> {
    if !applies(start) {
        return Err("部分输出恢复只接受已审计的原启动失败".into());
    }
    audited(backend, &start["sources"], TREE)?;
    let (controller, owner) = read_controller(run, start)?;
    if controller["sha256"] != CONTROLLER_SHA {
        return Err("部分输出恢复的原控制器摘要不同".into());
    }
    absent_controller(&owner)?;
    let failure = native_failure(backend, run, &controller)?;
    let executable = frozen_executable(&owner)?;
    let output = run.join(format!("g{ATTEMPT:04}"));
    let files = verified_files(backend, &output)?;
    let internal = verify_request(backend, run, request, &output)?;
    let intent = common::binding(&output.join("intent.json"))?;
    verify_intent(
        &common::read_bound(backend, &intent, "部分输出的原启动意图")?,
        &internal,
        &output.join("runtime"),
    )?;
    let runtime = common::binding(&output.join("runtime/runtime.json"))?;
    for path in [
        run.join(format!("results/{ATTEMPT:04}.json")),
        run.join(format!("seed-runtime/attempt-{ATTEMPT:04}")),
    ] {
        missing(backend, &path)?;
    }
    if read_controller(run, start)? != (controller.clone(), owner.clone())
        || verified_files(backend, &output)? != files
        || verify_request(backend, run, request, &output)? != internal
    {
        return Err("原控制器、请求或部分输出在核验期间变化".into());
    }
    common::bound_file(backend, &failure, "部分输出的原失败收据")?;
    absent_controller(&owner)?;
    Ok(
        json!({"start":digest::plan_hash(start)?,"failure":failure,"controller":controller,
        "source":start["sources"],"tree":TREE,"request":request,"internal_request":internal,
        "intent":intent,"runtime":runtime,"files":files,"executable":executable}),
    )
}

fn native_failure(backend: &Path, run: &Path, controller: &Value) -> Result<Value> {
    let path = run.join(format!("failure-{ATTEMPT:04}.json"));
    let binding = common::binding(&path)?;
    let failure = common::read_bound(backend, &binding, "部分输出的原失败收据")?;
    if binding["sha256"] != FAILURE_SHA
        || failure
            != json!({"format_version":2,"kind":"devex-stage-failure","attempt":ATTEMPT,
                "stage":"seed-runtime","mode":START,"operation_id":"clone.seed-runtime.source-generation-start",
                "error_type":"StageError","controller":controller})
    {
        return Err("部分输出恢复未绑定原生 StageError 收据".into());
    }
    Ok(binding)
}

fn frozen_executable(owner: &Value) -> Result<Value> {
    let path = Path::new(document::text(
        &owner["identity"]["executable"],
        "原启动工具",
    )?);
    validate_external_existing_file(path)?;
    let binding = common::binding(path)?;
    if binding["sha256"] != EXECUTABLE_SHA || binding["bytes"] != 75_022_336 {
        return Err("部分输出恢复未绑定原已审计的冻结启动工具".into());
    }
    Ok(binding)
}

fn verify_request(backend: &Path, run: &Path, request: &Value, output: &Path) -> Result<Value> {
    let external = common::bound_file(backend, request, "部分输出的原显式请求")?;
    let internal = common::binding(&output.join("request.json"))?;
    if external != expected_request_path(run)?
        || request["sha256"] != REQUEST_SHA
        || request["sha256"] != internal["sha256"]
        || request["bytes"] != internal["bytes"]
    {
        return Err("部分输出恢复必须使用原显式请求及同字节内部副本".into());
    }
    let value = common::read_bound(backend, &internal, "部分输出的原请求副本")?;
    coordinator::current_request(&value)?;
    Ok(internal)
}

fn verify_intent(value: &Value, request: &Value, runtime: &Path) -> Result<()> {
    document::exact(&value["operations"], &["api", "worker"], "原角色启动预留")?;
    let operations = &value["operations"];
    let operation = |role| {
        operations[role].as_str().is_some_and(|id| {
            id.len() == 32
                && id
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        })
    };
    if value
        != &json!({"format_version":1,"kind":"seed-source-runtime-intent","request":request,
            "runtime_directory":runtime,"operations":operations})
        || operations["api"] == operations["worker"]
        || !operation("api")
        || !operation("worker")
    {
        return Err("部分输出的启动意图或两个原角色预留不匹配".into());
    }
    Ok(())
}

fn verified_files(backend: &Path, output: &Path) -> Result<Vec<Value>> {
    output_files(
        backend,
        output,
        &["intent.json", "request.json"],
        "storage-layout-6f817bba7b374f81a8d1e20e5b40f0dd.json",
        FILES,
    )
}

fn output_files(
    backend: &Path,
    output: &Path,
    names: &[&str],
    layout: &str,
    expected: &[(&str, u64, &str)],
) -> Result<Vec<Value>> {
    exact_directory(
        backend,
        output,
        names,
        &["before", "runtime", "runtime-configuration"],
    )?;
    exact_directory(backend, &output.join("before"), &[layout], &[])?;
    exact_directory(
        backend,
        &output.join("runtime"),
        &["binaries.json", "runtime.json"],
        &[],
    )?;
    let config = output.join("runtime-configuration");
    exact_directory(backend, &config, &[], &["config", "worker-config"])?;
    for role in ["config", "worker-config"] {
        exact_directory(
            backend,
            &config.join(role),
            &["app.toml", "app.test.toml"],
            &[],
        )?;
    }
    let actual = inventory::manifest(output)?;
    require_files(&actual, expected)?;
    for (name, _, _) in expected {
        let path = output.join(name);
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file()
            || digest::linked(&metadata)
            || common::link_count_for(&path, &metadata)? != 1
        {
            return Err("部分输出证据必须为无链接普通文件".into());
        }
    }
    Ok(actual)
}

fn require_files(actual: &[Value], expected: &[(&str, u64, &str)]) -> Result<()> {
    if actual.len() != expected.len()
        || actual
            .iter()
            .zip(expected)
            .any(|(row, (path, bytes, sha))| {
                row != &json!({"path":path,"bytes":bytes,"sha256":sha})
            })
    {
        return Err("部分输出不属于已审计的完整文件清单".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_output_rejects_added_launch_artifacts_and_changed_originals() {
        let files: Vec<_> = FILES
            .iter()
            .map(|(path, bytes, sha)| json!({"path":path,"bytes":bytes,"sha256":sha}))
            .collect();
        require_files(&files, FILES).unwrap();
        for path in [
            "before/image.json",
            "dataset-lineage.json",
            "runtime/api-tree.json",
        ] {
            let mut changed = files.clone();
            changed.push(json!({"path":path,"bytes":1,"sha256":REQUEST_SHA}));
            assert!(require_files(&changed, FILES).is_err());
        }
        assert!(require_files(&files[1..], FILES).is_err());
        let mut changed = files;
        changed[0]["sha256"] = json!(REQUEST_SHA);
        assert!(require_files(&changed, FILES).is_err());
    }

    #[test]
    fn retry_accepts_only_the_audited_capture_failure_as_an_adjacent_predecessor() {
        let source = json!({"snapshot":{"head":HEAD},"worktree_fingerprint":FINGERPRINT});
        let start = json!({"number":ATTEMPT,"started_at":STARTED,"finished_at":FINISHED,"sources":source,
            "stage":"seed-runtime","mode":START,"status":"failed","result":null,"error_type":"StageError"});
        let recover = json!({"number":CAPTURE_ATTEMPT,"started_at":CAPTURE_STARTED,"finished_at":CAPTURE_FINISHED,
            "sources":source,"stage":"seed-runtime","mode":RECOVER,"status":"failed",
            "result":null,"error_type":"StageError"});
        assert!(history_tail(std::slice::from_ref(&start), CAPTURE_ATTEMPT).is_ok());
        assert!(history_tail(std::slice::from_ref(&start), CAPTURE_ATTEMPT + 1).is_err());
        let prefix = [start.clone(), recover.clone()];
        let (selected, collection) = history_tail(&prefix, CAPTURE_ATTEMPT + 1).unwrap();
        assert_eq!((selected, collection), (&start, Some(&recover)));
        assert!(history_tail(&prefix, CAPTURE_ATTEMPT + 2).is_err());
        assert!(history_tail(&[start, recover, json!({"number":86})], 87).is_err());
    }

    #[test]
    fn partial_output_requires_exact_attempt_and_distinct_bound_reservations() {
        let mut start = json!({"number":ATTEMPT,"started_at":STARTED,"finished_at":FINISHED,"sources":{"snapshot":{"head":HEAD},
            "worktree_fingerprint":FINGERPRINT},"stage":"seed-runtime","mode":START,"status":"failed",
            "result":null,"error_type":"StageError"});
        assert!(applies(&start));
        start["error_type"] = json!("ValueError");
        assert!(!applies(&start));
        let runtime = Path::new("runtime");
        let request = json!({"path":"request.json","bytes":5604,"sha256":REQUEST_SHA});
        let mut intent = json!({"format_version":1,"kind":"seed-source-runtime-intent","request":request,
            "runtime_directory":runtime,"operations":{"api":"a".repeat(32),"worker":"b".repeat(32)}});
        verify_intent(&intent, &request, runtime).unwrap();
        assert!(verify_intent(&intent, &request, Path::new("foreign/runtime")).is_err());
        intent["operations"]["worker"] = intent["operations"]["api"].clone();
        assert!(verify_intent(&intent, &request, runtime).is_err());
    }
}
