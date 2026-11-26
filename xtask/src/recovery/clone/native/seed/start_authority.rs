//! 新 seed 只在同一准备、原 API 退场与当前控制器完整核对后获得启动前像。

use std::path::Path;

use serde_json::{Value, json};

use super::{Current, Registration, history, latest, runtime_model};
use crate::{
    Result,
    recovery::{
        clone::native::{
            cache::handoff::registration,
            digest, json as document,
            lifecycle::state,
            post_process::{api_successor, runtime},
            resources::common,
            target_verify::authority::require_closed,
        },
        full_stack::worker::{controller, role::Role},
    },
};

pub(super) fn verify(backend: &Path, run: &Path, number: u64) -> Result<Value> {
    let saved = state::load(run, true)?;
    let attempt = state::running_attempt(&saved)?.ok_or("seed 启动缺少当前运行阶段")?;
    if attempt["stage"] != "seed-runtime"
        || attempt["mode"] != "start"
        || attempt["number"].as_u64() != Some(number)
    {
        return Err("启动授权只属于当前 seed start 阶段".into());
    }
    let current = Current::load(backend, run)?;
    let registered = Registration::load(backend, run, false)?;
    let post = history::post(
        backend,
        run,
        Some(&registered.value["post_copy"]),
        None,
        false,
    )?;
    let inputs = runtime_model::inputs(backend, run)?;
    let handoff = common::binding(&inputs.runtime.join("handoff.json"))?;
    let successor = api_successor::load(backend, run, &post.descriptor)?;
    let generation = ApiGeneration {
        post: &post,
        successor: successor.as_ref(),
    };
    let proof = retirement(
        backend,
        run,
        &saved,
        number,
        &generation,
        &registered.value,
        &handoff,
    )?;
    verify_closed(backend, run, &generation, &inputs, &proof)?;
    registered.assert_unchanged(backend, false)?;
    current.check(backend)?;
    if state::load(run, true)? != saved
        || api_successor::load(backend, run, &post.descriptor)? != successor
        || retirement(
            backend,
            run,
            &saved,
            number,
            &generation,
            &registered.value,
            &handoff,
        )? != proof
    {
        return Err("seed 启动授权期间原退场、登记或阶段证据变化".into());
    }
    Ok(
        json!({"handoff":handoff,"retirement":proof["stage"],"original_api":proof["api_process"],
        "original_tree":proof["api_tree"],"remote_writes":0}),
    )
}

struct ApiGeneration<'a> {
    post: &'a history::Post,
    successor: Option<&'a Value>,
}

impl ApiGeneration<'_> {
    fn directory(&self) -> Result<&Path> {
        match self.successor {
            Some(successor) => Ok(Path::new(document::text(
                &successor["runtime_directory"],
                "API 后继运行目录",
            )?)),
            None => Ok(&self.post.runtime),
        }
    }

    fn require_retirement(&self, backend: &Path, evidence: &Value) -> Result<()> {
        let Some(successor) = self.successor else {
            if evidence.get("successor").is_some() {
                return Err("没有已发布 API 后继，不能选用后继退场收据".into());
            }
            return Ok(());
        };
        let directory = self.directory()?;
        let expected = json!({"intent":successor["intent"],"api_process":successor["api"],
            "api_tree":successor["api_tree"]});
        if directory == self.post.runtime
            || evidence["successor"] != expected
            || common::bound_file(backend, &successor["api"], "退场后继 API")?
                != directory.join("api.json")
            || common::bound_file(backend, &successor["api_tree"], "退场后继监督树")?
                != directory.join("api-tree.json")
        {
            return Err("seed 启动要求同一已发布 API 后继的完整退场结果".into());
        }
        common::bound_file(backend, &successor["intent"], "退场后继启动意图")?;
        Ok(())
    }
}

fn verify_closed(
    backend: &Path,
    run: &Path,
    generation: &ApiGeneration<'_>,
    inputs: &runtime_model::Inputs,
    proof: &Value,
) -> Result<()> {
    let post = generation.post;
    let directory = generation.directory()?;
    let runtime = registration::cleanup_runtime(backend, run, post)?;
    let product = runtime::product_root(backend, &runtime)?;
    let seed_runtime =
        common::read_bound(backend, &inputs.handoff["runtime"], "seed 固定运行合同")?;
    if runtime != seed_runtime {
        return Err("seed 启动必须复用原 API 和 Worker 运行合同".into());
    }
    let original = api_identity(backend, &proof["api_process"], &runtime)?;
    let active = if let Some(successor) = generation.successor {
        for name in ["runtime.json", "binaries.json"] {
            let file = common::common_file(backend, &directory.join(name), "API 后继固定文件")?;
            if digest::file(&file)? != digest::file(&post.runtime.join(name))? {
                return Err("seed 启动时 API 后继运行合同或构建清单不同于原冻结字节".into());
            }
        }
        Some(api_identity(backend, &successor["api"], &runtime)?)
    } else {
        None
    };
    let identities = std::iter::once(&original).chain(active.as_ref());
    for identity in identities.clone() {
        require_pid_absent(identity, &mut common::process_identity)?;
    }
    let observed = controller::observe_bound(
        Role::Api,
        &product,
        directory,
        &post.request["api_environment"],
    )?;
    if observed != json!({"state":"stopped","identity":null}) {
        return Err("seed 启动前未取得已登记 API 的完整监督树关闭证明".into());
    }
    require_closed(document::text(&inputs.handoff["api_url"], "seed API 地址")?)?;
    require_closed(document::text(
        &runtime["worker_ready_url"],
        "seed Worker 地址",
    )?)?;
    for identity in identities {
        require_pid_absent(identity, &mut common::process_identity)?;
    }
    Ok(())
}

fn api_identity(backend: &Path, descriptor: &Value, runtime: &Value) -> Result<Value> {
    let process = common::read_bound(backend, descriptor, "seed API 进程收据")?;
    if process["scope_id"] != runtime["scope_id"]
        || process["identity"]["executable"] != runtime["artifacts"]["api"]["path"]
    {
        return Err("seed 启动必须复用原 API 运行合同".into());
    }
    common::validate_process_identity(&process["identity"])
}

fn retirement(
    backend: &Path,
    run: &Path,
    saved: &Value,
    current: u64,
    generation: &ApiGeneration<'_>,
    registered: &Value,
    handoff: &Value,
) -> Result<Value> {
    let post = generation.post;
    let directory = generation.directory()?;
    let prepare = latest(saved, "seed-runtime", &["prepare"], Some(current), false)?;
    let old = latest(
        saved,
        "runtime-target",
        &["start", "stop", "restart-api", "reconcile-api"],
        Some(current),
        false,
    )?;
    let prepared = common::read_bound(backend, &prepare["result"], "seed 启动准备结果")?;
    let stopped = common::read_bound(backend, &old["result"], "seed 原 API 退场结果")?;
    let evidence = &stopped["seed_retirement"];
    document::exact(
        evidence,
        if generation.successor.is_some() {
            &["prepare", "api_process", "api_tree", "successor"]
        } else {
            &["prepare", "api_process", "api_tree"]
        },
        "seed 原 API 退场绑定",
    )?;
    generation.require_retirement(backend, evidence)?;
    if old["mode"] != "stop"
        || old["number"].as_u64() <= prepare["number"].as_u64()
        || prepared["status"] != "seed_runtime_prepared"
        || prepared["handoff"] != *handoff
        || evidence["prepare"] != prepare["result"]
        || evidence["api_process"] != registered["api_process"]
        || stopped["format_version"] != 1
        || stopped["kind"] != "devex-clone-runtime-control"
        || stopped["operation"] != "stop"
        || stopped["runtime_directory"] != json!(directory)
        || stopped["processes"] != json!({"api":{"state":"stopped","identity":null,"ready":false}})
        || stopped["producer_history"] != json!(directory.join("producer-history.json"))
        || common::bound_file(backend, &evidence["api_process"], "原 API 进程")?
            != post.runtime.join("api.json")
        || common::bound_file(backend, &evidence["api_tree"], "原 API 监督树")?
            != post.runtime.join("api-tree.json")
    {
        return Err("seed 启动需要当前准备之后同一原 API 的完整退场结果".into());
    }
    let runtime = common::read_bound(backend, handoff, "seed 运行交接")?;
    if stopped["scope_id"] != runtime["scope_id"]
        || runtime["original_api"] != evidence["api_process"]
        || common::binding(&run.join("seed-runtime/runtime/handoff.json"))? != *handoff
    {
        return Err("seed 原 API 退场作用域或运行交接变化".into());
    }
    Ok(
        json!({"stage":old["result"],"api_process":evidence["api_process"],"api_tree":evidence["api_tree"]}),
    )
}

fn require_pid_absent(
    identity: &Value,
    inspect: &mut dyn FnMut(u32) -> Result<Option<Value>>,
) -> Result<()> {
    let pid = u32::try_from(identity["pid"].as_u64().ok_or("seed 原 API PID 无效")?)?;
    if inspect(pid)?.is_some() {
        return Err("原 API 尚未停止或 PID 已复用，不能启动新运行对".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::clone::native::seed::fixture::Fixture;

    #[test]
    fn original_api_pid_must_be_absent_instead_of_reused() {
        let identity = json!({"pid":991,"started":"100"});
        for live in [identity.clone(), json!({"pid":991,"started":"200"})] {
            assert!(require_pid_absent(&identity, &mut |_| Ok(Some(live.clone()))).is_err());
        }
        assert!(require_pid_absent(&identity, &mut |_| Err("unknown".into())).is_err());
        assert!(
            require_pid_absent(&identity, &mut |pid| {
                assert_eq!(pid, 991);
                Ok(None)
            })
            .is_ok()
        );
    }

    #[test]
    fn retirement_must_follow_the_latest_prepare_and_match_both_original_files() {
        let mut fixture = Fixture::new();
        let old = fixture.old.clone();
        let registered = fixture.request.clone();
        let api = registered["api_process"].clone();
        let tree = fixture.file("unrelated-tree", &json!({"fixture":true}));
        let tree_path = old.join("api-tree.json");
        document::write_new(&tree_path, &json!({"original":true})).unwrap();
        let original_tree = common::binding(&tree_path).unwrap();
        let runtime = fixture.run.join("seed-runtime/runtime");
        std::fs::create_dir_all(&runtime).unwrap();
        let handoff = runtime.join("handoff.json");
        document::write_new(&handoff, &json!({"scope_id":"test","original_api":api})).unwrap();
        let handoff = common::binding(&handoff).unwrap();
        let post = history::Post {
            request: Value::Null,
            descriptor: Value::Null,
            runtime: old.clone(),
        };
        let prep = json!({"status":"seed_runtime_prepared","handoff":handoff});
        let prepared = fixture.stage("seed-runtime", "prepare", Some(prep.clone()), None);
        let mut stopped = json!({"format_version":1,"kind":"devex-clone-runtime-control","operation":"stop",
            "runtime_directory":old,"scope_id":"test","producer_history":old.join("producer-history.json"),
            "processes":{"api":{"state":"stopped","identity":null,"ready":false}},
            "seed_retirement":{"prepare":prepared,"api_process":api,"api_tree":tree}});
        fixture.stage("runtime-target", "stop", Some(stopped.clone()), None);
        let verify = |fixture: &Fixture| {
            retirement(
                &fixture.backend,
                &fixture.run,
                &state::load(&fixture.run, true).unwrap(),
                999,
                &ApiGeneration {
                    post: &post,
                    successor: None,
                },
                &registered,
                &handoff,
            )
        };
        assert!(verify(&fixture).is_err());
        stopped["seed_retirement"]["api_tree"] = original_tree.clone();
        let stop = fixture.stage("runtime-target", "stop", Some(stopped.clone()), None);
        assert_eq!(verify(&fixture).unwrap()["stage"], stop);
        let new_prepare = fixture.stage("seed-runtime", "prepare", Some(prep), None);
        assert!(verify(&fixture).is_err());

        let active = fixture.run.join("post-copy/api-successors/runtime");
        std::fs::create_dir_all(&active).unwrap();
        let active_api = active.join("api.json");
        let active_tree = active.join("api-tree.json");
        document::write_new(&active_api, &json!({"successor":true})).unwrap();
        document::write_new(&active_tree, &json!({"operation_id":"new"})).unwrap();
        let active_api = common::binding(&active_api).unwrap();
        let active_tree = common::binding(&active_tree).unwrap();
        let intent = fixture.file("api-successor-intent", &json!({"intent":true}));
        let successor = json!({"runtime_directory":active,"intent":intent,
            "api":active_api,"api_tree":active_tree});
        let selected = ApiGeneration {
            post: &post,
            successor: Some(&successor),
        };
        let verify_active = |fixture: &Fixture| {
            retirement(
                &fixture.backend,
                &fixture.run,
                &state::load(&fixture.run, true).unwrap(),
                999,
                &selected,
                &registered,
                &handoff,
            )
        };
        fixture.stage(
            "runtime-target",
            "restart-api",
            Some(successor.clone()),
            None,
        );
        assert!(verify_active(&fixture).is_err());
        stopped["seed_retirement"]["prepare"] = new_prepare;
        stopped["seed_retirement"]["successor"] = json!({
            "intent":intent,"api_process":active_api,"api_tree":active_tree
        });
        fixture.stage("runtime-target", "stop", Some(stopped.clone()), None);
        assert!(verify_active(&fixture).is_err());

        stopped["runtime_directory"] = json!(active);
        stopped["producer_history"] = json!(active.join("producer-history.json"));
        let stop = fixture.stage("runtime-target", "stop", Some(stopped), None);
        let proof = verify_active(&fixture).unwrap();
        assert_eq!(proof["stage"], stop);
        assert_eq!(proof["api_process"], api);
        assert_eq!(proof["api_tree"], original_tree);
    }
}
