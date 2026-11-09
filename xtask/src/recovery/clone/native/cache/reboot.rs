//! Windows 缓存主机重启只读观察与原收尾的历史复核。
use std::{
    collections::BTreeMap,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use crate::{
    Result,
    recovery::{monitoring::execution::ProtectedInputs, source::evidence::reboot_process_exited},
};

use super::{
    super::{json as document, lifecycle::state, resources::common},
    binding, history, require_windows, stage,
};

const CLOSE: &str = "source-generation-reboot-close";
const STATUS: &str = "cache_status";
const ROW: &[&str] = &[
    "attempt",
    "state",
    "runtime",
    "supervisor",
    "process",
    "proof",
];
type Observation = (Value, Vec<String>);

pub(in crate::recovery) fn observe(backend: &Path, run: &Path, boot: u64) -> Result<Observation> {
    if !cfg!(windows) {
        return Ok(blocked(
            "error",
            Value::Null,
            "缓存重启观察仅支持本机 Windows",
        ));
    }
    let ledger = Ledger::open(backend, run)?;
    let result = observe_registered(backend, &ledger, boot);
    ledger.unchanged()?;
    result
}

pub(in crate::recovery) fn verify_recorded(
    backend: &Path,
    run: &Path,
    cache: &Value,
    boot: u64,
    before: u64,
) -> Result<()> {
    let ledger = Ledger::open(backend, run)?;
    let result = verify_history(backend, &ledger, cache, boot, before);
    ledger.unchanged()?;
    result
}

struct Ledger {
    run: PathBuf,
    value: Value,
    registration: Option<(Value, Value)>,
    inputs: ProtectedInputs,
}

impl Ledger {
    fn open(backend: &Path, run: &Path) -> Result<Self> {
        let run = common::local_directory(backend, run, "缓存重启运行")
            .map_err(|_| "缓存重启运行目录不是有效受管路径")?;
        let cache = run.join(stage::STAGE);
        let registration = cache.join("registration.json");
        let present = entry_exists(&registration)?;
        let mut paths = vec![run.join("state.json"), run.join("manifest.json")];
        if present {
            paths.extend([registration, cache.join("request.json")]);
        }
        let inputs = protect(backend, paths)?;
        let value = state::load(&run, true).map_err(|_| "缓存重启无法核对原完整账本")?;
        let registration = if present {
            Some(
                serde_json::from_value(stage_action(backend, &run, "registration", json!({}))?)
                    .map_err(|_| "缓存重启登记返回无效")?,
            )
        } else {
            None
        };
        inputs
            .assert_unchanged()
            .map_err(|_| "缓存重启原账本、清单或登记变化")?;
        Ok(Self {
            run,
            value,
            registration,
            inputs,
        })
    }

    fn attempts(&self) -> Result<&[Value]> {
        state::attempts(&self.value).map_err(|_| "缓存重启阶段账本无效".into())
    }

    fn unchanged(&self) -> Result<()> {
        self.inputs
            .assert_unchanged()
            .map_err(|_| "缓存重启观察期间原账本、清单或登记变化".into())
    }

    fn creations(&self, backend: &Path, before: Option<u64>) -> Result<Vec<Creation>> {
        let (request, descriptor) = self.registration.as_ref().ok_or("缓存重启缺少原登记")?;
        require_windows(&request["process"])?;
        let response = stage_action(
            backend,
            &self.run,
            "reboot-starts",
            json!({"request":request,"descriptor":descriptor,"before":before}),
        )?;
        let rows: Vec<history::Generation> =
            serde_json::from_value(response).map_err(|_| "缓存重启原创建记录无效")?;
        let mut indexed = BTreeMap::new();
        for (attempt, output, runtime) in rows {
            if indexed
                .insert(stage::number(&attempt)?, (attempt, output, runtime))
                .is_some()
            {
                return Err("缓存重启原创建记录重复".into());
            }
        }
        let mut created = Vec::new();
        for attempt in self.attempts()?.iter().filter(|row| {
            row["stage"] == stage::STAGE
                && row["mode"] == "restart"
                && before.is_none_or(|limit| row["number"].as_u64().is_some_and(|n| n < limit))
        }) {
            let number = stage::number(attempt)?;
            let output = self.run.join(stage::STAGE).join(format!("a{number:04}"));
            if let Some((record, actual, runtime)) = indexed.remove(&number) {
                if record != *attempt || actual != output {
                    return Err("缓存重启原创建记录与当前账本不同".into());
                }
                created.push(Creation {
                    attempt: number,
                    output,
                    runtime,
                });
            } else if attempt["status"] == "passed"
                || entry_exists(&output.join("start.json"))?
                || entry_exists(&output.join("process"))?
            {
                // 已有创建前缀却没有完整 runtime，不能把该代次从重启观察中略去。
                created.push(Creation {
                    attempt: number,
                    output,
                    runtime: Value::Null,
                });
            }
        }
        if !indexed.is_empty() {
            return Err("缓存重启创建记录包含账本切点之外的代次".into());
        }
        Ok(created)
    }
}

fn observe_registered(backend: &Path, ledger: &Ledger, boot: u64) -> Result<Observation> {
    let Some((request, descriptor)) = &ledger.registration else {
        return Ok(blocked(
            "cache_unregistered",
            Value::Null,
            "缓存登记为空，无法核对重启前代次",
        ));
    };
    if require_windows(&request["process"]).is_err() {
        return Ok(blocked(
            "error",
            descriptor.clone(),
            "缓存重启需要原 Windows Redis v3 登记",
        ));
    }
    let running = ledger
        .attempts()?
        .iter()
        .filter(|row| row["status"] == "running")
        .collect::<Vec<_>>();
    if !(running.is_empty()
        || matches!(running.as_slice(), [row]
        if row["stage"] == "seed-runtime" && row["mode"] == CLOSE))
    {
        return Ok(blocked(
            "error",
            descriptor.clone(),
            "统一运行还有未收尾阶段，不能观察缓存重启",
        ));
    }
    let Ok(created) = ledger.creations(backend, None) else {
        return Ok(blocked(
            "error",
            descriptor.clone(),
            "缓存重启无法完整核对原创建记录",
        ));
    };
    if created.is_empty() {
        return Ok(blocked(
            STATUS,
            descriptor.clone(),
            "缓存登记为空，无法核对重启前代次",
        ));
    }
    let mut generations = Vec::new();
    let mut blockers = Vec::new();
    for creation in &created {
        match observe_creation(backend, ledger, request, creation, boot) {
            Ok(row) => generations.push(row),
            Err(_) => {
                generations.push(json!({"attempt":creation.attempt,"state":"unknown",
                    "runtime":null,"supervisor":null,"process":null,"proof":null}));
                blockers.push(format!(
                    "缓存代次 {} 缺少完整的重启前退出证明",
                    creation.attempt
                ));
            }
        }
    }
    Ok((
        json!({"status":STATUS,"request":descriptor,"generations":generations}),
        blockers,
    ))
}

fn verify_history(
    backend: &Path,
    ledger: &Ledger,
    cache: &Value,
    boot: u64,
    before: u64,
) -> Result<()> {
    document::exact(cache, &["status", "request", "generations"], "缓存重启收尾")?;
    let (_, descriptor) = ledger
        .registration
        .as_ref()
        .ok_or("缓存重启收尾缺少原登记")?;
    if cache["status"] != STATUS
        || cache["request"] != *descriptor
        || !ledger.attempts()?.iter().any(|row| {
            row["number"].as_u64() == Some(before)
                && row["stage"] == "seed-runtime"
                && row["mode"] == CLOSE
                && row["status"] == "passed"
        })
    {
        return Err("缓存重启收尾未绑定原登记和成功收尾切点".into());
    }
    let saved = document::array(&cache["generations"], "缓存重启收尾代次")?;
    let created = ledger.creations(backend, Some(before))?;
    if saved.is_empty() || saved.len() != created.len() {
        return Err("缓存重启收尾缺少切点前完整代次集合".into());
    }
    for (row, creation) in saved.iter().zip(&created) {
        verify_creation(backend, row, creation, boot)
            .map_err(|_| "缓存重启收尾代次未绑定原运行、两身份及原退出证明")?;
    }
    Ok(())
}

struct Creation {
    attempt: u64,
    output: PathBuf,
    runtime: Value,
}

impl Creation {
    fn bound_runtime(&self, backend: &Path, boot: u64) -> Result<(Value, ProtectedInputs)> {
        let process_output = self.output.join("process");
        require_windows(&self.runtime)?;
        if self.runtime["redis"]["platform"] != "windows"
            || stage::path(&self.runtime["output"])? != process_output
        {
            return Err("缓存重启代次未绑定原 Windows 运行目录".into());
        }
        let path = process_output.join("runtime.json");
        let guard = protect(backend, [self.output.join("start.json"), path.clone()])?;
        let binding = common::binding(&path)?;
        if common::read_bound(backend, &binding, "缓存重启原运行")? != self.runtime
            || (
                reboot_process_exited(&self.runtime["supervisor"], boot)?,
                reboot_process_exited(&self.runtime["process"], boot)?,
            ) != (true, true)
        {
            return Err("缓存重启原运行不同，或两个原进程均早于启动余量未获证明".into());
        }
        Ok((binding, guard))
    }

    fn row(&self, binding: &Value, proof: &Value) -> Value {
        json!({"attempt":self.attempt,"state":"stopped","runtime":binding,
            "supervisor":self.runtime["supervisor"],"process":self.runtime["process"],"proof":proof})
    }

    fn external(&self, binding: &Value) -> Value {
        json!({"kind":"windows-redis-external-termination","runtime":binding,
            "supervisor":self.runtime["supervisor"],"process":self.runtime["process"],
            "port":self.runtime["redis"]["port"],"normal_job_exit_proof":null})
    }

    fn exited(&self) -> PathBuf {
        self.output.join("process/exited.json")
    }

    fn stopped(&self) -> Value {
        json!({"state":"stopped","launcher_alive":false,
            "windows":{"identity":self.runtime["process"],"alive":false}})
    }

    fn normal(&self, backend: &Path, receipt: &Value) -> Result<Value> {
        if common::bound_file(backend, receipt, "缓存重启原退出收据")? != self.exited() {
            return Err("缓存重启正常退出未绑定原收据路径".into());
        }
        let exit = common::read_bound(backend, receipt, "缓存重启原退出收据")?;
        document::exact(
            &exit,
            &["process", "returncode", "job_empty"],
            "缓存重启原退出收据",
        )?;
        if exit["process"] != self.runtime["process_receipt"]
            || exit["job_empty"] != true
            || exit["returncode"]
                .as_i64()
                .is_none_or(|code| i32::try_from(code).is_err())
        {
            return Err("缓存重启正常退出收据未证明原进程和完整 Job".into());
        }
        Ok(json!({"normal_exit":receipt,"observation":self.stopped()}))
    }
}

fn observe_creation(
    backend: &Path,
    ledger: &Ledger,
    request: &Value,
    creation: &Creation,
    boot: u64,
) -> Result<Value> {
    let (binding, guard) = creation.bound_runtime(backend, boot)?;
    let result = (|| {
        let (process, _tools): (Value, Value) = serde_json::from_value(stage_action(
            backend,
            &ledger.run,
            "reboot-generation",
            json!({"request":request,"output":creation.output}),
        )?)?;
        require_windows(&process)?;
        let exited = creation.exited();
        let proof = if entry_exists(&exited)? {
            let exit_guard = protect(backend, [exited.clone()])?;
            let result: Result<Value> = (|| {
                let normal = creation.normal(backend, &common::binding(&exited)?)?;
                let current = observe_exit(backend, &process, &creation.runtime)?;
                if current != normal["observation"] {
                    return Err("缓存重启正常退出没有获得当前停止证明".into());
                }
                Ok(normal)
            })();
            exit_guard.assert_unchanged()?;
            result?
        } else {
            let actual = observe_exit(backend, &process, &creation.runtime)?;
            if actual != creation.external(&binding) {
                return Err("缓存重启外部终止观察未证明原两身份及端口关闭".into());
            }
            actual
        };
        Ok(creation.row(&binding, &proof))
    })();
    guard.assert_unchanged()?;
    result
}

fn observe_exit(backend: &Path, process: &Value, runtime: &Value) -> Result<Value> {
    #[cfg(windows)]
    return super::windows::reboot_exit(backend, process, runtime);
    #[cfg(not(windows))]
    {
        let _ = (backend, process, runtime);
        Err("原缓存退出只在本机 Windows 观察".into())
    }
}

fn verify_creation(backend: &Path, row: &Value, creation: &Creation, boot: u64) -> Result<()> {
    let (binding, guard) = creation.bound_runtime(backend, boot)?;
    let result = (|| {
        document::exact(row, ROW, "缓存重启收尾代次")?;
        let proof = &row["proof"];
        if proof.get("normal_exit").is_some() {
            let exit_guard = protect(backend, [creation.exited()])?;
            let result = creation.normal(backend, &proof["normal_exit"]);
            exit_guard.assert_unchanged()?;
            if *proof != result? {
                return Err("缓存重启正常退出历史证明不同".into());
            }
        } else if *proof != creation.external(&binding) {
            return Err("缓存重启外部终止历史证明不同".into());
        }
        if *row != creation.row(&binding, proof) {
            return Err("缓存重启收尾代次的序号、停止状态或原进程身份不同".into());
        }
        Ok(())
    })();
    guard.assert_unchanged()?;
    result
}

fn stage_action(backend: &Path, run: &Path, action: &str, parameters: Value) -> Result<Value> {
    binding::execute_stage(
        backend,
        &json!({"action":action,"directory":run,"caller":std::process::id(),"parameters":parameters}),
        None,
    )
    .map_err(|_| "缓存重启账本查询失败，保留原现场".into())
}

fn protect(backend: &Path, paths: impl IntoIterator<Item = PathBuf>) -> Result<ProtectedInputs> {
    let paths = paths
        .into_iter()
        .map(|path| common::common_file(backend, &path, "缓存重启原文件"))
        .collect::<Result<Vec<_>>>()
        .map_err(|_| "缓存重启原文件缺失或不是无链接受管文件")?;
    ProtectedInputs::open(paths).map_err(|_| "缓存重启无法保护原文件".into())
}

fn entry_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(false),
        Err(_) => Err("缓存重启无法读取原前缀状态".into()),
    }
}

fn blocked(status: &str, request: Value, message: &str) -> Observation {
    (
        json!({"status":status,"request":request,"generations":[]}),
        vec![message.to_owned()],
    )
}
