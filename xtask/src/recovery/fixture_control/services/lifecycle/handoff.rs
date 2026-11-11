//! 向原复制账本移交 RustFS，保留同代 Redis 和原标记。
use super::*;
use crate::recovery::clone::native::{
    lifecycle::lock, resources::common, seed::source_registration,
};

pub(super) fn run(
    runtime: &mut dyn Runtime,
    context: &Context,
    owner_file: &Path,
) -> Result<Value> {
    ready(context)?;
    let close_path = context.owner(owner_file)?;
    let close_result = common::binding(&close_path)?;
    let run = copy_for_close(context, &close_path, &close_result)?;
    let clone_guard = ProcessGuard::acquire(&run)?;
    let _fixture_guard = ProcessGuard::acquire(&context.run)?;
    let storage = registered(runtime, context, true)?;
    let (selected, markers) = source_registration::close::handoff(
        &context.backend,
        &close_result,
        &context.run,
        &storage,
    )
    .map_err(|_| model::error("RustFS 移交无法核验原复制账本、种子关闭或服务绑定"))?;
    check(selected == run, "种子关闭收据绑定其他复制目录")?;
    let copy_state = common::binding(&run.join("state.json"))?;
    let guard = || {
        clone_guard.check()?;
        check(
            lock::owner_observation(&run)?.is_none()
                && common::binding(&run.join("state.json"))? == copy_state
                && common::binding(&close_path)? == close_result,
            "RustFS 移交期间原复制账本或种子关闭收据变化",
        )
    };
    owned(context, |owner| {
        context.guard(None)?;
        check_observation(
            runtime,
            &storage,
            &expected_running(),
            "移交前原服务代次未同时运行",
        )?;
        guard()?;
        let mut sources = context.sources();
        sources["seed_close"] = close_result.clone();
        sources["copy_state"] = copy_state.clone();
        stage(
            context,
            "handoff-rustfs",
            &sources,
            owner,
            |_, controller, running, output| {
                let stable = |runtime: &mut dyn Runtime| {
                    context.guard(Some(running))?;
                    guard()?;
                    check(
                        registered(runtime, context, true)? == storage,
                        "RustFS 移交期间原服务请求或收据变化",
                    )
                };
                new_output(context, output)?;
                stable(runtime)?;
                let proof = transfer(runtime, &storage, &markers, stable)?;
                let evidence = output.join("rustfs-stopped.json");
                json_io::write_new(&evidence, &proof)?;
                result(
                    context,
                    "handoff-rustfs",
                    "rustfs_handed_off",
                    json!({
                        "services":{"redis":"running","rustfs":"handed-off"},
                        "controller":controller,"seed_close":close_result,"copy_state":copy_state,
                        "evidence":{"rustfs":binding(&evidence)?,"redis":storage["evidence"]["redis_runtime"]}
                    }),
                )
            },
        )
    })
}

fn ready(context: &Context) -> Result<()> {
    let history = context.history(true)?;
    check(
        history["active_generation"]["kind"] == "initial"
            && history["closed"] == false
            && history["external_recovery"].is_null()
            && history["rustfs_handoff"].is_null(),
        "RustFS 移交只接受尚未关闭或移交的完整首代夹具",
    )?;
    context.no_unknown()
}

fn copy_for_close(context: &Context, close_path: &Path, result: &Value) -> Result<PathBuf> {
    let run = close_path
        .parent()
        .filter(|path| path.ends_with("results"))
        .and_then(Path::parent)
        .ok_or("种子关闭收据缺少复制目录")?;
    let run = common::local_directory(&context.backend, run, "移交复制目录")?;
    check(run != context.run, "RustFS 移交须绑定独立原复制账本")?;
    let state = state::load(&run, false)?;
    let last = state::attempts(&state)?
        .last()
        .ok_or("复制账本缺少种子关闭")?;
    check(
        last["stage"] == "seed-runtime"
            && last["mode"] == "close"
            && last["status"] == "passed"
            && last["result"] == *result,
        "移交 owner 不是原复制账本末尾的成功种子关闭",
    )?;
    Ok(run)
}

fn transfer(
    runtime: &mut dyn Runtime,
    services: &Value,
    markers: &Value,
    stable: impl Fn(&mut dyn Runtime) -> Result<()>,
) -> Result<Value> {
    let redis = || {
        json!({"request":services["requests"]["redis"],
        "runtime":services["runtime"],"markers":markers})
    };
    let before = runtime.call("cache-handoff-markers", redis())?;
    stable(runtime)?;
    let tree = &services["tree"];
    let terminated = runtime.call("tree-stop", json!({"tree":tree,"crash":false}))?;
    check(terminated == true, "RustFS 移交未取得原运行树的停止结果")?;
    observation::closed(&services["requests"]["rustfs"])?;
    let completion = runtime.call("completion", json!({"tree":tree}))?;
    let after = runtime.call("cache-handoff-markers", redis())?;
    check(before == after, "RustFS 移交期间原 Redis 代次或标记变化")?;
    stable(runtime)?;
    Ok(
        json!({"tree":tree,"terminated":terminated,"state":"stopped",
        "completion":completion,"redis":after,"data_directory":services["requests"]["rustfs"]["data_directory"]}),
    )
}

pub(super) fn lock_recipient(context: &Context, history: &Value) -> Result<Option<ProcessGuard>> {
    if history["rustfs_handoff"].is_null() {
        return Ok(None);
    }
    let result = history::evidence::bound(&context.run, &history["rustfs_handoff"])?;
    let run = Path::new(text(&result["copy_state"], "path")?)
        .parent()
        .ok_or("RustFS 移交缺少复制目录")?;
    let run = common::local_directory(&context.backend, run, "移交复制目录")?;
    let guard = ProcessGuard::acquire(&run)?;
    check(
        lock::owner_observation(&run)?.is_none(),
        "移交后的复制运行仍有控制器，不能关闭原 Redis",
    )?;
    Ok(Some(guard))
}

pub(super) fn observe_retired(
    runtime: &mut dyn Runtime,
    services: &Value,
    closed: bool,
) -> Result<Value> {
    let tree = &services["tree"];
    check(
        observation::identities(runtime, tree)?
            == json!({"supervisor":"missing","monitor":"missing","process":"missing"}),
        "已移交的 RustFS 原进程树仍存在",
    )?;
    runtime.call("members-wait", json!({"tree":tree}))?;
    let cache = runtime.call(
        "cache-status",
        json!({"request":services["requests"]["redis"],
        "runtime":services["runtime"],"effective":services["cache_effective"]}),
    )?;
    check(
        cache["state"] == if closed { "stopped" } else { "running" },
        "RustFS 移交后的原 Redis 状态与账本不同",
    )?;
    Ok(json!({"redis":cache["state"],"rustfs":"handed-off","termination":null}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    struct Fixture {
        sockets: Option<[TcpListener; 2]>,
        calls: Vec<String>,
        fail_before: bool,
        drift: bool,
    }

    impl Runtime for Fixture {
        fn call(&mut self, operation: &str, arguments: Value) -> Result<Value> {
            self.calls.push(operation.to_owned());
            match operation {
                "cache-handoff-markers" => {
                    check(
                        arguments["markers"] == json!({"fixed":true}),
                        "标记合同变化",
                    )?;
                    check(!self.fail_before, "测试：标记读取失败")?;
                    Ok(json!({"run_id": if self.drift && self.sockets.is_none() {
                        "other"
                    } else {
                        "original"
                    }}))
                }
                "tree-stop" => {
                    check(
                        arguments == json!({"tree":{"original":true},"crash":false}),
                        "停止树变化",
                    )?;
                    self.sockets.take();
                    Ok(json!(true))
                }
                "completion" => Ok(json!({"status":"stopped"})),
                _ => panic!("移交调用了额外原语 {operation}"),
            }
        }

        fn now(&self) -> Duration {
            Duration::ZERO
        }

        fn pause(&mut self, _: Duration) {}
    }

    #[test]
    fn handoff_stops_only_rustfs_and_requires_the_same_redis_generation() {
        for (fail_before, drift) in [(false, false), (true, false), (false, true)] {
            let sockets = std::array::from_fn(|_| TcpListener::bind("127.0.0.1:0").unwrap());
            let [api, console] = sockets
                .each_ref()
                .map(|socket| format!("http://{}", socket.local_addr().unwrap()));
            let services = json!({"tree":{"original":true},
                "requests":{"rustfs":{"api_url":api,"console_url":console,"data_directory":"same"}}});
            let mut fixture = Fixture {
                sockets: Some(sockets),
                calls: Vec::new(),
                fail_before,
                drift,
            };
            let result = transfer(&mut fixture, &services, &json!({"fixed":true}), |_| Ok(()));
            if fail_before {
                assert!(result.is_err());
                assert_eq!(fixture.calls, ["cache-handoff-markers"]);
                assert!(fixture.sockets.is_some());
                continue;
            }
            assert_eq!(
                fixture.calls,
                [
                    "cache-handoff-markers",
                    "tree-stop",
                    "completion",
                    "cache-handoff-markers"
                ]
            );
            if drift {
                assert!(result.is_err());
            } else {
                let proof = result.unwrap();
                assert_eq!(proof["data_directory"], "same");
                assert_eq!(proof["redis"]["run_id"], "original");
            }
        }
    }
}
