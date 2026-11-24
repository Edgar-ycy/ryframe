//! 保留原 RustFS 运行代次，仅从已确认关闭的 Windows Redis Job 接续。
use super::*;
use crate::recovery::{
    clone::native::{digest, resources::common},
    fixture_control::services::history::evidence::INCOMPLETE_REDIS_CLEANUP,
};

const OPERATION: &str = "restart-redis";

pub(super) fn run(
    runtime: &mut dyn Runtime,
    context: &Context,
    owner_file: &Path,
) -> Result<Value> {
    let history = context.history(true)?;
    context.require_current_owner(owner_file)?;
    context.no_unknown()?;
    check(
        controller(runtime, context)?.is_null(),
        "Redis 单独重启前仍存在控制器",
    )?;
    let previous = registered(runtime, context, true)?;
    let incident = observe_incident(runtime, context, &history, &previous)?;
    let result_file = &history["active_generation"];
    let generation = history::evidence::restart_generation(&context.run, result_file)?;
    let mut sources = context.sources();
    sources["previous_generation"] = result_file.clone();
    let expected = state::attempts(&context.value["state"])?.len() as u64 + 1;
    let _guard = ProcessGuard::acquire(&context.run)?;
    owned(context, |owner| {
        context.guard(None)?;
        context.no_unknown()?;
        stage(
            context,
            OPERATION,
            &sources,
            owner,
            |number, controller, running, output| {
                check(number == expected, "Redis 单独重启编号与账本不一致")?;
                let attempt = Attempt {
                    history: &history,
                    previous: &previous,
                    rustfs: &generation["rustfs"],
                    incident: &incident,
                    controller,
                    running,
                    output,
                    number,
                };
                attempt.restart(runtime, context)
            },
        )
    })
}

pub(super) fn observe_incident(
    runtime: &mut dyn Runtime,
    context: &Context,
    history: &Value,
    services: &Value,
) -> Result<Value> {
    check(
        history["initial_complete"] == true
            && history["closed"] == false
            && history["external_recovery"].is_null()
            && history["restart_predecessor"].is_null()
            && history["rustfs_handoff"].is_null()
            && history["unsettled"].as_array().is_some_and(Vec::is_empty)
            && history["active_generation"].is_object()
            && history["active_generation"].get("kind").is_none(),
        "Redis 单独重启必须接续开放的已验证重启代次",
    )?;
    context.no_unknown()?;
    let generation =
        history::evidence::restart_generation(&context.run, &history["active_generation"])?;
    let request = &services["requests"]["redis"];
    check(
        services["cache_effective"].is_null()
            && request["format_version"] == 3
            && services["runtime"]["format_version"] == 3
            && services["runtime"]["redis"]["platform"] == "windows"
            && services["runtime"] == generation["redis"]["runtime"]
            && services["storage"] == generation["rustfs"]["storage"]
            && services["tree"]["process"] == services["storage"]["identity"]
            && services["evidence"]["rustfs_tree"] == services["storage"]["tree"]
            && history::evidence::bound(&context.run, &generation["redis"]["request"])? == *request
            && history::evidence::bound(&context.run, &generation["rustfs"]["request"])?
                == services["requests"]["rustfs"],
        "Redis 单独重启未绑定原 Windows Redis 运行收据或原 RustFS 登记",
    )?;
    let tree = &services["tree"];
    common::local_directory(
        &context.backend,
        Path::new(text(tree, "runtime_directory")?),
        "Redis 单独重启所保留的原 RustFS 目录",
    )?;
    let running = json!({"supervisor":"running","monitor":"running","process":"running"});
    check(
        observation::identities(runtime, tree)? == running
            && runtime.call(
                "storage-running",
                json!({"identity":tree["process"],"request":services["requests"]["rustfs"]}),
            )? == true
            && observation::identities(runtime, tree)? == running,
        "Redis 单独重启前无法确认原 RustFS 的完整进程树与监听归属",
    )?;
    let incident = runtime.call(
        "cache-accepted-failure",
        json!({"request":request,"runtime":services["runtime"],"live":true}),
    )?;
    require_incident(
        &context.run,
        request,
        &services["runtime"],
        &incident,
        generation["kind"] == "reference-fixture-redis-generation",
    )?;
    Ok(incident)
}

pub(super) fn require_incident(
    run: &Path,
    request: &Value,
    runtime: &Value,
    incident: &Value,
    redis_only_predecessor: bool,
) -> Result<()> {
    match incident["kind"].as_str() {
        Some("windows-redis-accepted-failure") if !redis_only_predecessor => return Ok(()),
        Some(INCOMPLETE_REDIS_CLEANUP) => {}
        _ => return Err("原 Windows Redis 缺少当前前驱可采用的已确认事故证据".into()),
    }
    history::evidence::exact(
        incident,
        &[
            "kind",
            "failure",
            "runtime",
            "supervisor",
            "process",
            "port",
            "normal_job_exit_proof",
        ],
    )?;
    let output = Path::new(text(runtime, "output")?);
    check(
        incident["supervisor"] == runtime["supervisor"]
            && incident["process"] == runtime["process"]
            && incident["port"] == request["port"]
            && incident["port"] == runtime["redis"]["port"]
            && incident["normal_job_exit_proof"].is_null()
            && history::evidence::path_is(
                &incident["runtime"]["path"],
                &output.join("runtime.json"),
            )
            && history::evidence::path_is(
                &incident["failure"]["path"],
                &output.join("failed.json"),
            )
            && history::evidence::bound(run, &incident["runtime"])? == *runtime
            && history::evidence::bound(run, &incident["failure"])?["cleanup_completed"] == false,
        "Windows Redis 事故未绑定原运行、失败、进程身份与端口",
    )
}

struct Attempt<'a> {
    history: &'a Value,
    previous: &'a Value,
    rustfs: &'a Value,
    incident: &'a Value,
    controller: &'a Value,
    running: &'a Value,
    output: &'a Path,
    number: u64,
}

impl Attempt<'_> {
    fn restart(&self, runtime: &mut dyn Runtime, context: &Context) -> Result<Value> {
        new_output(context, self.output)?;
        let cache_output = self.output.join("redis");
        let request_file = cache_output.join("request.json");
        let incident_file = self.output.join("redis-incident.json");
        let request = restart::redis_request(self.previous)?;
        new_output(context, &cache_output)?;
        json_io::write_new(&request_file, &request)?;
        json_io::write_new(&incident_file, self.incident)?;
        let incident_binding = binding(&incident_file)?;
        context.guard(Some(self.running))?;
        let previous = registered(runtime, context, true)?;
        check(
            previous == *self.previous
                && observe_incident(runtime, context, self.history, &previous)? == *self.incident,
            "Redis 单独重启前原服务登记或 Job 退出证据变化",
        )?;
        context.guard(Some(self.running))?;
        let request_binding = binding(&request_file)?;
        check(
            history::evidence::bound(&context.run, &request_binding)? == request
                && history::evidence::bound(&context.run, &incident_binding)? == *self.incident,
            "Redis 单独重启的持久请求或事故证据变化",
        )?;
        let started = runtime.call(
            "cache-start",
            json!({"request":request,"private":context.value["private"],"output":cache_output,
                "context":context.value,"running":self.running}),
        )?;
        let runtime_file = binding(&cache_output.join("runtime.json"))?;
        check_started(context, &cache_output, &request, &started, &runtime_file)?;

        let mut current = self.previous.clone();
        current["requests"]["redis"] = request.clone();
        current["runtime"] = started.clone();
        current["evidence"]["redis_runtime"] = runtime_file;
        let readback = Readback {
            current: &current,
            request: &request_binding,
            incident: &incident_binding,
        };
        match self.publish(runtime, context, readback) {
            Ok(result) => Ok(result),
            Err(error) => {
                self.cleanup(runtime, context, &request, &started)
                    .map_err(|_| {
                        Box::new(model::LifecycleError {
                            category: "RestartCleanupUnknown",
                            message: "新 Redis 代次的精确回收未确认；保留原证据和现场，禁止自动重放",
                        }) as Box<dyn std::error::Error>
                    })?;
                Err(error)
            }
        }
    }

    fn publish(
        &self,
        runtime: &mut dyn Runtime,
        context: &Context,
        readback: Readback<'_>,
    ) -> Result<Value> {
        self.check_current(runtime, context, &readback)?;
        let file = self.output.join("restart-redis.json");
        let previous_generation = &self.history["active_generation"];
        let generation = json!({
            "format_version":1,"kind":"reference-fixture-redis-generation","run":context.run,
            "attempt":self.number,"owner":context.value["sources"]["state_before"],
            "previous_generation":previous_generation,"controller":self.controller,
            "services":{"redis":"running","rustfs":"running"},
            "rustfs":self.rustfs,
            "redis":{"request":readback.request,"runtime":readback.current["runtime"]},
            "incident":readback.incident,"remote_writes":0,"resources_deleted":false
        });
        json_io::write_new(&file, &generation)?;
        let descriptor = binding(&file)?;
        self.check_current(runtime, context, &readback)?;
        check(
            history::evidence::bound(&context.run, &descriptor)? == generation,
            "Redis 单独重启代次在发布前发生变化",
        )?;
        result(
            context,
            OPERATION,
            "redis_restarted",
            json!({"controller":self.controller,"services":generation["services"],
                "generation":descriptor,"previous_generation":previous_generation,"next_operation":"close"}),
        )
    }

    fn check_current(
        &self,
        runtime: &mut dyn Runtime,
        context: &Context,
        readback: &Readback<'_>,
    ) -> Result<()> {
        context.guard(Some(self.running))?;
        context.no_unknown()?;
        check(
            registered(runtime, context, true)? == *self.previous
                && history::evidence::bound(&context.run, readback.request)?
                    == readback.current["requests"]["redis"]
                && history::evidence::bound(&context.run, readback.incident)? == *self.incident
                && history::evidence::bound(
                    &context.run,
                    &readback.current["evidence"]["redis_runtime"],
                )? == readback.current["runtime"],
            "发布 Redis 单独重启期间，原服务登记或本次证据变化",
        )?;
        check_observation(
            runtime,
            readback.current,
            &expected_running(),
            "原 RustFS 或新 Redis 代次未保持就绪",
        )?;
        let old = runtime.call(
            "cache-accepted-failure",
            json!({"request":self.previous["requests"]["redis"],
                "runtime":self.previous["runtime"],"live":false}),
        )?;
        check(
            old == *self.incident,
            "发布 Redis 后继前原退出与失败收据变化",
        )?;
        context.guard(Some(self.running))
    }

    fn cleanup(
        &self,
        runtime: &mut dyn Runtime,
        context: &Context,
        request: &Value,
        started: &Value,
    ) -> Result<()> {
        context.guard(Some(self.running))?;
        let output = self.output.join("failed-start-cleanup");
        new_output(context, &output)?;
        let stop = runtime.call(
            "cache-stop",
            json!({"request":request,"private":context.value["private"],"runtime":started,
                "output":output,"context":context.value,"running":self.running}),
        )?;
        check(
            stop["status"] == "redis_process_stopped",
            "Redis 单独重启失败后的精确回收未确认",
        )
    }
}

struct Readback<'a> {
    current: &'a Value,
    request: &'a Value,
    incident: &'a Value,
}

fn check_started(
    context: &Context,
    output: &Path,
    request: &Value,
    started: &Value,
    descriptor: &Value,
) -> Result<()> {
    common::validate_process_identity(&started["supervisor"])?;
    common::validate_process_identity(&started["process"])?;
    check(
        started["format_version"] == 3
            && history::evidence::path_is(&started["output"], output)
            && history::evidence::bound(&context.run, descriptor)? == *started
            && started["request_sha256"] == digest::plan_hash(request)?
            && started["redis"]["platform"] == "windows"
            && started["redis"]["port"] == request["port"]
            && started["redis"]["pid"] == started["process"]["pid"]
            && started["redis"]["started"] == started["process"]["started"]
            && started["process"] != request["previous_identity"]
            && started["redis"]["run_id"].is_string()
            && started["redis"]["run_id"] != request["previous_run_id"],
        "Redis 启动未返回本次新目录内的精确新代；保留现场，禁止自动重放",
    )
}
