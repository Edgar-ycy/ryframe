//! 按冻结首代和追加代次核对服务归属，不依据当前进程补造历史。
use super::*;
use crate::recovery::clone::native::resources::{common, storage};

pub(super) fn load(
    runtime: &mut dyn Runtime,
    backend: &Path,
    value: &Value,
    settled: bool,
) -> Result<Value> {
    let context = context(backend, value)?;
    let history = context.history(settled)?;
    let Services {
        mut requests,
        mut storage,
        mut cache,
        mut tree_binding,
        mut tree,
        mut observed,
        origin: initial,
    } = initial(runtime, &context)?;
    if history["active_generation"]["kind"] != "initial" {
        let outer = common::read_bound(backend, &history["active_generation"], "当前服务结果")?;
        let generation = common::read_bound(backend, &outer["generation"], "当前服务代次")?;
        for role in ["rustfs", "redis"] {
            requests[role] =
                common::read_bound(backend, &generation[role]["request"], "重启服务请求")?;
        }
        storage = generation["rustfs"]["storage"].clone();
        cache = json!({"runtime":generation["redis"]["runtime"]});
        tree_binding = storage["tree"].clone();
        let path = common::bound_file(backend, &tree_binding, "重启进程树")?;
        let output = path.parent().ok_or("进程树缺少父目录")?;
        tree = self::tree(runtime, output, &requests["rustfs"])?;
        observed = inspect(
            backend,
            output,
            &value["sources"]["manifest"],
            &generation["controller"],
            generation["attempt"].as_u64().ok_or("重启编号无效")?,
            &generation["rustfs"]["request"],
        )?;
        validate_storage(&storage, &observed, &requests["rustfs"], &tree)?;
    }
    let runtime_path = if history["active_generation"]["kind"] == "initial" {
        context.run.join("redis/runtime.json")
    } else {
        Path::new(text(&cache["runtime"], "output")?).join("runtime.json")
    };
    check(
        read(&runtime_path)? == cache["runtime"],
        "Redis 当前重启运行收据变化",
    )?;
    Ok(
        json!({"requests":requests,"tree":tree,"runtime":cache["runtime"],"storage":storage,
        "cache_effective":effective(&context, &requests["redis"])?,"storage_observation":observed,
        "origin":initial,"evidence":{"rustfs_tree":tree_binding,"redis_runtime":binding(&runtime_path)?}}),
    )
}

fn context(backend: &Path, value: &Value) -> Result<Context> {
    check(
        Path::new(text(value, "backend")?) == backend,
        "服务上下文属于其他后端",
    )?;
    let run = crate::local_test_path::validate_local_test_path(
        Path::new(text(value, "run")?),
        backend,
        crate::local_test_path::LocalTestPathKind::ExistingDirectory,
    )?;
    // 执行中的调用方持有旧状态快照；运行代次由外层 guard 单独核对，不能误读新账本。
    Ok(Context {
        backend: backend.to_path_buf(),
        run,
        value: value.clone(),
    })
}

struct Services {
    requests: Value,
    storage: Value,
    cache: Value,
    tree_binding: Value,
    tree: Value,
    observed: Value,
    origin: Value,
}

fn initial(runtime: &mut dyn Runtime, context: &Context) -> Result<Services> {
    let backend = &context.backend;
    let value = &context.value;
    let attempts = state::attempts(&value["state"])?;
    let mut requests = json!({});
    for (role, attempt) in ["rustfs", "redis"].into_iter().zip(attempts) {
        let descriptor = &attempt["sources"]["request"];
        check(
            descriptor.is_object(),
            "历史首代缺少冻结服务请求，不能补造完整生命周期证据",
        )?;
        let path = common::bound_file(backend, descriptor, "冻结服务请求")?;
        check(
            path == context.run.join(role).join("request.json"),
            "冻结服务请求不属于原启动目录",
        )?;
        requests[role] = read(&path)?;
    }
    common::directory_identity(backend, &requests["rustfs"]["data_directory"])?;
    let storage = common::read_bound(backend, &attempts[0]["result"], "RustFS 首代结果")?;
    let tree_binding = storage["tree"].clone();
    check(
        tree_binding.is_object()
            && common::bound_file(backend, &tree_binding, "RustFS 首代进程树")?
                == context.run.join("rustfs/rustfs-tree.json"),
        "历史首代缺少启动前进程树归属，不能宣称完整后代可回收",
    )?;
    let tree = tree(runtime, &context.run.join("rustfs"), &requests["rustfs"])?;
    check(
        tree["process"] == storage["identity"],
        "RustFS 进程树与首代真实创建身份不同",
    )?;
    let observed = inspect(
        backend,
        &context.run.join("rustfs"),
        &value["sources"]["manifest"],
        &binding(&context.run.join("controller-0001.json"))?,
        1,
        &attempts[0]["sources"]["request"],
    )?;
    validate_storage(&storage, &observed, &requests["rustfs"], &tree)?;
    let cache = common::read_bound(backend, &attempts[1]["result"], "Redis 首代结果")?;
    check(
        cache["service"] == "redis"
            && cache["runtime"].is_object()
            && read(&context.run.join("redis/runtime.json"))? == cache["runtime"],
        "Redis 首代缺少冻结运行收据",
    )?;
    let initial = json!({"storage":{"identity":storage["identity"],"sha256":storage["sha256"],
        "process_receipt":storage["process_receipt"],"launch_receipt":storage["launch_receipt"]},
        "redis":cache["runtime"]["redis"]});
    Ok(Services {
        requests,
        storage,
        cache,
        tree_binding,
        tree,
        observed,
        origin: initial,
    })
}

fn tree(runtime: &mut dyn Runtime, output: &Path, request: &Value) -> Result<Value> {
    runtime.call(
        "tree-read",
        json!({"output":output,"scope_id":request["scope_id"]}),
    )
}

fn validate_storage(
    storage: &Value,
    observed: &Value,
    request: &Value,
    tree: &Value,
) -> Result<()> {
    check(
        observed["state"] == "recorded"
            && observed["identity"] == tree["process"]
            && ["identity", "process_receipt", "launch_receipt"]
                .iter()
                .all(|key| storage[*key] == observed[*key])
            && storage["sha256"] == request["executable"]["sha256"],
        "RustFS 意图、进程、成功结果与冻结启动来源不同",
    )
}

fn inspect(
    backend: &Path,
    output: &Path,
    registration: &Value,
    controller: &Value,
    number: u64,
    descriptor: &Value,
) -> Result<Value> {
    check(
        common::bound_file(backend, descriptor, "嵌入服务请求")? == output.join("request.json"),
        "服务账本嵌入请求必须位于原始启动目录",
    )?;
    let request = common::read_bound(backend, descriptor, "嵌入服务请求")?;
    common::bound_file(backend, controller, "服务控制器")?;
    let intent_path = output.join("intent.json");
    let intent = read(&intent_path)?;
    check(
        intent
            == json!({"format_version":1,"kind":"devex-clone-storage-intent",
        "request":registration,"controller":controller,"attempt":number,
        "arguments":storage::model::arguments(&request)?,"environment":storage::model::configuration(&request)?}),
        "存储意图不属于原注册与对应控制阶段",
    )?;
    let receipt_path = output.join("process.json");
    let receipt = read(&receipt_path)?;
    let identity = common::validate_process_identity(&receipt["identity"])?;
    check(
        receipt
            == json!({"format_version":1,"role":"rustfs","scope_id":request["scope_id"],
        "lifecycle":"running","identity":identity,"api_url":request["api_url"],"console_url":request["console_url"],
        "data_dir":request["data_directory"]["path"],"intent":binding(&intent_path)?})
            && identity["executable"] == request["executable"]["path"],
        "存储进程收据不属于该启动意图或实际工具",
    )?;
    let launch_path = output.join("launch.json");
    let launch = if launch_path.exists() {
        check(
            read(&launch_path)?
                == json!({"format_version":1,"scope_id":request["scope_id"],
            "identity":identity,"arguments":storage::model::arguments(&request)?,
            "environment":storage::model::configuration(&request)?,"credential_files":request["credential_files"]}),
            "存储启动参数收据变化",
        )?;
        binding(&launch_path)?
    } else {
        Value::Null
    };
    Ok(
        json!({"state":"recorded","identity":identity,"process_receipt":binding(&receipt_path)?,
        "launch_receipt":launch,"intent":binding(&intent_path)?}),
    )
}

fn effective(context: &Context, original: &Value) -> Result<Value> {
    if context.value["manifest"]["review"] == context.value["sources"]["review"] {
        return Ok(Value::Null);
    }
    let tools = &context.value["documents"]["review"]["tools"];
    let redis = &tools["redis_server"];
    let python = &tools["redis_python"];
    check(
        redis["distribution"] == original["distribution"]
            && redis["resolved_path"] == original["executable"]
            && redis["sha256"] == original["sha256"]
            && python["distribution"] == original["distribution"]
            && python["path"] == original["python"]["path"]
            && python["resolved_path"] == original["python"]["executable"],
        "夹具工具续签只能替换 WSL 或同一 Python 入口的字节代次",
    )?;
    let mut effective = original.clone();
    effective["wsl"] = tools["wsl"].clone();
    effective["python"] = json!({"path":python["path"],"executable":python["resolved_path"],"sha256":python["sha256"]});
    Ok(if effective == *original {
        Value::Null
    } else {
        effective
    })
}
