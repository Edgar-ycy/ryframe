use super::*;

pub(super) fn load(runtime: &mut dyn Runtime, context: &Context) -> Result<Value> {
    result_files(context)?;
    let attempts = state::attempts(&context.value["state"])?;
    let initial: Vec<_> = attempts
        .iter()
        .take_while(|item| item["stage"] != history::STAGE)
        .collect();
    check(
        initial.len() <= 2
            && initial
                .iter()
                .all(|item| ["passed", "failed"].iter().any(|s| item["status"] == *s)),
        "部分关闭只接受已收尾、尚未执行对象桶写入的首代前缀",
    )?;
    check(
        !initial.windows(2).any(|pair| pair[0]["status"] != "passed"),
        "首代失败后出现后继步骤，不能确认副作用范围",
    )?;
    check(
        initial
            .iter()
            .all(|item| item["status"] != "failed" || item["result"].is_null()),
        "失败首代含未确认发布结果，不能推断副作用范围",
    )?;
    let mut result = json!({"initial":initial,"requests":{},"tree":null,"runtime":null,"bindings":{},"not_started":{}});
    for (index, role) in ["rustfs", "redis", "buckets"].iter().enumerate() {
        load_role(runtime, context, &initial, index, role, &mut result)?;
    }
    Ok(result)
}

fn load_role(
    runtime: &mut dyn Runtime,
    context: &Context,
    initial: &[&Value],
    index: usize,
    role: &&str,
    result: &mut Value,
) -> Result<()> {
    let Some(attempt) = initial.get(index) else {
        result["not_started"][*role] = absent(context, role, None)?;
        return Ok(());
    };
    let descriptor = &attempt["sources"]["request"];
    check(
        attempt["sources"]["review"] == context.value["manifest"]["review"]
            && attempt["sources"]["bootstrap"] == context.value["manifest"]["bootstrap"],
        "初代步骤未绑定原始审阅与环境",
    )?;
    check(
        common::bound_file(&context.backend, descriptor, "首代请求")?
            == context.run.join(role).join("request.json"),
        "部分关闭请求不属于原始启动目录",
    )?;
    result["bindings"][format!("{role}_request")] = descriptor.clone();
    result["bindings"][format!("{role}_controller")] = binding(
        &context
            .run
            .join(format!("controller-{:04}.json", index + 1)),
    )?;
    result["requests"][*role] = common::read_bound(&context.backend, descriptor, "首代请求")?;
    if attempt["status"] != "passed" && !context.run.join(role).join("intent.json").try_exists()? {
        result["not_started"][*role] = absent(context, role, Some(descriptor))?;
    } else if *role == "rustfs" {
        storage(runtime, context, attempt, result)?;
    } else if attempt["status"] == "failed" {
        use crate::recovery::clone::native::cache::failed_cleanup;
        let request = &result["requests"]["redis"];
        let directory = context.run.join("redis");
        let validated = if request["format_version"] == 3 {
            failed_cleanup::observe_windows_failure(request, &directory)
        } else {
            failed_cleanup::validate(request, &directory)
        }
        .map_err(|_| {
            model::error(
                "Redis 原请求、创建、失败、停止或退出收据不完整或不一致；保留现场，禁止重放",
            )
        })?;
        result["runtime"] = validated["runtime"].clone();
        result["failed_cleanup"] = json!(true);
        if let Some(closed) = validated.get("windows_closed") {
            result["windows_failed"] = closed.clone();
        }
        if validated["reconcile_required"] == true {
            result["reconciliation"] = validated.clone();
        }
        result["bindings"]
            .as_object_mut()
            .ok_or("部分关闭绑定集合无效")?
            .extend(
                validated["bindings"]
                    .as_object()
                    .ok_or("失败清理绑定集合无效")?
                    .clone(),
            );
    } else {
        check(
            attempt["status"] == "passed",
            "Redis 启动结果未知；保留现场，不能以缺收据推断未启动",
        )?;
        let cache = common::read_bound(&context.backend, &attempt["result"], "Redis 首代结果")?;
        let file = context.run.join("redis/runtime.json");
        check(
            cache["service"] == "redis"
                && cache["runtime"].is_object()
                && read(&file)? == cache["runtime"],
            "Redis 首代运行收据不完整",
        )?;
        result["runtime"] = cache["runtime"].clone();
        result["bindings"]["redis_result"] = attempt["result"].clone();
        result["bindings"]["redis_runtime"] = binding(&file)?;
    }
    Ok(())
}

fn result_files(context: &Context) -> Result<()> {
    let ledger = state::load(&context.run, true)?;
    let expected = state::attempts(&ledger)?
        .iter()
        .filter_map(|item| item["result"]["path"].as_str())
        .map(PathBuf::from)
        .collect::<std::collections::BTreeSet<_>>();
    for entry in std::fs::read_dir(context.run.join("results"))? {
        let entry = entry?;
        check(
            entry.file_type()?.is_file()
                && expected.contains(&entry.path())
                && !crate::recovery::clone::native::digest::linked_path(&entry.path())?
                    .contains(&true),
            "初代账本存在未登记结果，部分关闭不能接管未知写入",
        )?;
    }
    Ok(())
}

fn storage(
    runtime: &mut dyn Runtime,
    context: &Context,
    attempt: &Value,
    result: &mut Value,
) -> Result<()> {
    let request = result["requests"]["rustfs"].clone();
    check(
        request["scope_id"] == context.value["manifest"]["scope_id"],
        "部分关闭 RustFS scope 不同",
    )?;
    common::directory_identity(&context.backend, &request["data_directory"])?;
    let output = context.run.join("rustfs");
    if attempt["status"] == "failed" && !output.join("rustfs-tree.json").try_exists()? {
        let closed =
            super::super::super::storage::closed::read(&context.backend, &context.run, attempt)?;
        result["bindings"]
            .as_object_mut()
            .ok_or("部分关闭输入集合无效")?
            .extend(
                closed["bindings"]
                    .as_object()
                    .ok_or("早期关闭输入集合无效")?
                    .clone(),
            );
        result["early_storage"] = closed;
        return Ok(());
    }
    let tree = runtime.call(
        "tree-read",
        json!({"output":output,"scope_id":request["scope_id"]}),
    )?;
    check(
        tree["scope_id"] == request["scope_id"] && tree["runtime_directory"] == json!(output),
        "部分关闭监督树属于其他 scope 或目录",
    )?;
    storage_files(&output, &tree)?;
    if output.join("actor.json").try_exists()? || output.join("actor-binding.json").try_exists()? {
        let records =
            super::super::super::storage::records::load(&context.backend, &context.run, attempt)?;
        result["bindings"]
            .as_object_mut()
            .ok_or("部分关闭输入集合无效")?
            .extend(
                records["bindings"]
                    .as_object()
                    .ok_or("原 actor 输入集合无效")?
                    .clone(),
            );
    }
    let observed = registration::inspect(
        &context.backend,
        &output,
        &context.value["sources"]["manifest"],
        &binding(&context.run.join("controller-0001.json"))?,
        1,
        &attempt["sources"]["request"],
    )?;
    check(
        observed["state"] == "recorded"
            && observed["identity"] == tree["process"]
            && !observed["launch_receipt"].is_null(),
        "RustFS 部分启动缺少精确创建身份、启动或监督树归属",
    )?;
    if attempt["status"] == "passed" {
        let saved = common::read_bound(&context.backend, &attempt["result"], "RustFS 首代结果")?;
        check(
            ["identity", "process_receipt", "launch_receipt"]
                .iter()
                .all(|key| saved[*key] == observed[*key])
                && saved["sha256"] == request["executable"]["sha256"]
                && saved["tree"] == binding(&output.join("rustfs-tree.json"))?,
            "RustFS 首代结果与实际启动归属不同",
        )?;
        result["bindings"]["rustfs_result"] = attempt["result"].clone();
    }
    for name in [
        "intent.json",
        "process.json",
        "launch.json",
        "rustfs-tree.json",
    ] {
        result["bindings"][format!("rustfs_{name}")] = binding(&output.join(name))?;
    }
    result["bindings"]["rustfs_controller"] = binding(&context.run.join("controller-0001.json"))?;
    result["tree"] = tree;
    Ok(())
}

fn storage_files(output: &Path, tree: &Value) -> Result<()> {
    // 原生 actor 的额外文件必须先由完整授权和内容核验；不能只扩文件名白名单。
    if output.join("actor.json").try_exists()? || output.join("actor-binding.json").try_exists()? {
        let run = output.parent().ok_or("原 actor 无账本目录")?;
        let saved = state::load(run, true)?;
        let attempt = state::attempts(&saved)?
            .first()
            .ok_or("原 actor 无首代阶段")?;
        super::super::super::storage::records::load(&crate::workspace::root_dir(), run, attempt)?;
        return Ok(());
    }
    let operation = text(tree, "operation_id")?;
    let mut names = [
        "request.json",
        "intent.json",
        "launch.json",
        "process.json",
        "ready.json",
        "spawned.json",
        "rustfs-tree.json",
        "rustfs.json",
        "stdout.log",
        "stderr.log",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<std::collections::BTreeSet<_>>();
    for suffix in ["ready", "stopped"] {
        names.insert(format!("rustfs-members-{operation}-{suffix}.json"));
    }
    for suffix in ["control", "result"] {
        names.insert(format!("rustfs-tree-{operation}-{suffix}.json"));
    }
    for entry in std::fs::read_dir(output)? {
        let entry = entry?;
        check(
            entry.file_type()?.is_file()
                && names.contains(&entry.file_name().to_string_lossy().into_owned())
                && !crate::recovery::clone::native::digest::linked_path(&entry.path())?
                    .contains(&true),
            "RustFS 部分启动目录含未知写入、链接或非普通文件",
        )?;
    }
    Ok(())
}

fn absent(context: &Context, role: &str, request: Option<&Value>) -> Result<Value> {
    let directory = context.run.join(role);
    verify_absent(&directory, request)?;
    Ok(json!({"request":request}))
}

pub(super) fn verify_absent(directory: &Path, request: Option<&Value>) -> Result<()> {
    check(
        !crate::recovery::clone::native::digest::linked_path(directory)?.contains(&true),
        "未启动目录经过链接或重解析点",
    )?;
    match std::fs::symlink_metadata(directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            check(request.is_none(), "冻结请求目录缺失")
        }
        Err(error) => Err(error.into()),
        Ok(metadata) => {
            check(metadata.is_dir(), "未启动步骤目录不是普通目录")?;
            let files = std::fs::read_dir(directory)?
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<std::io::Result<Vec<_>>>()?;
            check(
                match request {
                    Some(expected) => {
                        files == [directory.join("request.json")]
                            && binding(&files[0])? == *expected
                    }
                    None => files.is_empty(),
                },
                "未启动步骤包含 intent、launch、process、运行收据或未知写入，不能推断零启动",
            )
        }
    }
}
