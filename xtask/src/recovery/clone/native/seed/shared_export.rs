//! 从同一已发布 seed 代次签发唯一共享导出；采用只读复核原候选。

use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::super::{
    bridge::scope::Scope,
    export::{capture, frozen, input::Context, proof, recovery, transport::SystemTransport},
    json as document,
    lifecycle::{
        identity, lock,
        stage::{StageRequest, StageRunner},
        state,
    },
    post_process,
    resources::{common, storage::registered},
    target_verify::guard::Inputs,
};
use crate::{Result, cli::CloneSide, recovery::source::evidence::comparison};

const EXPORT: &str = "source-export";
const RECONCILE: &str = "source-export-reconcile";

pub(crate) struct Runner;

impl StageRunner for Runner {
    fn preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        let saved = state::load(request.run_dir, true)?;
        let prior = export_rows(state::attempts(&saved)?);
        match origin(&prior, request.mode)? {
            None => Ok(()),
            Some(record) => candidate(request.backend, request.run_dir, record).map(|_| ()),
        }
    }

    fn execute(&self, request: &StageRequest<'_>) -> Result<Value> {
        let saved = state::load(request.run_dir, true)?;
        let rows = state::attempts(&saved)?;
        let current = active(request, rows)?;
        let prior = export_rows(&rows[..rows.len() - 1]);
        let original = origin(&prior, request.mode)?;
        let number = number(current)?;
        post_process::require_quiet(request.backend, request.run_dir, Some(number))?;
        let source = comparison::shared_export_source(request.backend, request.run_dir, &saved)?;
        let window = Window::open(request, number, &source)?;
        let result = match original {
            None => window.capture(current),
            Some(original) => window.adopt(current, original, &prior),
        };
        let checked = window.finish(request, &saved, current);
        match (result, checked) {
            (Ok(published), Ok(())) => published.close(request.backend, request.run_dir),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(check)) => {
                Err(format!("共享 seed 导出失败：{error}；最终核验失败：{check}").into())
            }
        }
    }
}

struct Window<'a> {
    context: Context,
    source: &'a Value,
    scope: Scope,
    held: Inputs,
}

struct Publication {
    origin: u64,
    result: Value,
    evidence: frozen::FrozenExport,
    seal: Option<Value>,
}

impl Publication {
    fn close(self, backend: &Path, run: &Path) -> Result<Value> {
        common::bound_file(backend, &self.result["origin_intent"], "最终原共享导出意图")?;
        let path = seal_path(run, self.origin);
        if read_seal(backend, &path)? != self.seal {
            return Err("原共享导出封存于最终核验期间变化".into());
        }
        self.evidence
            .check_proofs(backend, &self.result["export"])?;
        Ok(self.result)
    }
}

impl<'a> Window<'a> {
    fn open(request: &StageRequest<'_>, number: u64, source: &'a Value) -> Result<Self> {
        let backend = request.backend;
        let run = request.run_dir;
        let held = Inputs::open([
            run.join("manifest.json"),
            common::bound_file(backend, &source["source_request"], "共享导出源请求")?,
            common::bound_file(backend, &source["source_environment"], "共享导出源环境")?,
        ])?;
        let context = Context::from_bound_seed(
            backend,
            run,
            number,
            request.mode,
            &source["source_request"],
            &source["source_environment"],
        )?;
        if context.request != source["request"] {
            return Err("共享导出未采用同一冻结源请求".into());
        }
        let bridges = document::array(&context.manifest["build_bridges"], "共享导出原构建桥接")?;
        let scope = Scope::load(backend, bridges, &[])?;
        Ok(Self {
            context,
            source,
            scope,
            held,
        })
    }

    fn generation(&self) -> Result<Value> {
        self.held.check()?;
        proof::copy::observe_in(&self.context, &self.scope)
    }

    fn storage(&self) -> Result<()> {
        let current = registered::current(
            &self.context.backend,
            &self.context.run_dir,
            CloneSide::Target,
        )?;
        if current.as_ref() != Some(&self.source["storage"]["storage"]) {
            return Err("原共享 seed 导出的实际存储代次变化".into());
        }
        Ok(())
    }

    fn capture(&self, current: &Value) -> Result<Publication> {
        let context = &self.context;
        let number = number(current)?;
        let intent = self.expected_intent(current)?;
        recovery::write_intent(context, &intent)?;
        let intent_path = intent_path(&context.run_dir, number);
        let intent_binding = common::binding(&intent_path)?;
        let output = context.output();
        fs::create_dir(&output)?;
        let exported = match capture::capture_data(
            context,
            &SystemTransport,
            &|| self.generation(),
            &|| self.storage(),
        ) {
            Ok(exported) => exported,
            Err(error) => {
                document::write_new(
                    &output.join("failure.json"),
                    &json!({
                        "format_version":1,"status":"source_export_failed","stage":"source_export",
                        "error_type":"StageError","request_file":context.request_path,
                        "request_digest":{"bytes":self.source["source_request"]["bytes"],
                            "sha256":self.source["source_request"]["sha256"]},
                        "reason":"共享 seed 导出未完成，请核对对应诊断文件",
                        "stdout":"","stderr":"","clone_verified":false,"restore_qualified":false
                    }),
                )?;
                return Err(error);
            }
        };
        let (summary, evidence) = self.summary(&exported)?;
        if common::read_bound(&context.backend, &intent_binding, "共享导出原意图")? != intent
        {
            return Err("共享导出原意图发生变化".into());
        }
        let seal = recovery::seal(number, &intent_binding, &intent, &exported, &summary);
        document::write_new(&seal_path(&context.run_dir, number), &seal)?;
        Ok(Publication {
            origin: number,
            result: self.result(number, &intent_binding, &exported, &summary),
            evidence,
            seal: Some(seal),
        })
    }

    fn adopt(&self, current: &Value, original: &Value, prior: &[&Value]) -> Result<Publication> {
        let backend = &self.context.backend;
        let run = &self.context.run_dir;
        let number = number(original)?;
        let path = common::common_file(backend, &intent_path(run, number), "原共享导出意图")?;
        let intent_binding = common::binding(&path)?;
        let intent = common::read_bound(backend, &intent_binding, "原共享导出意图")?;
        if intent != self.expected_intent(original)?
            || recovery::product(&intent["execution_source"])?
                != recovery::product(&current["sources"])?
        {
            return Err("原共享导出意图、存储或产品来源与当前候选不同".into());
        }
        let exported = common::binding(&candidate(backend, run, original)?)?;
        let (summary, evidence) = self.summary(&exported)?;
        if common::read_bound(backend, &intent_binding, "原共享导出意图")? != intent
            || common::binding(&candidate(backend, run, original)?)? != exported
        {
            return Err("原候选或导出意图在复核期间变化".into());
        }
        let seal = read_seal(backend, &seal_path(run, number))?;
        let expected_seal = recovery::seal(number, &intent_binding, &intent, &exported, &summary);
        if seal.as_ref().is_some_and(|saved| *saved != expected_seal) {
            return Err("原共享导出封存与完整复核摘要不同".into());
        }
        let result = self.result(number, &intent_binding, &exported, &summary);
        check_prior_results(backend, run, prior, &result)?;
        Ok(Publication {
            origin: number,
            result,
            evidence,
            seal,
        })
    }

    fn expected_intent(&self, attempt: &Value) -> Result<Value> {
        recovery::expected_intent(
            &self.context,
            attempt,
            &self.source["source_request"],
            &self.source["storage"]["storage"],
            recovery::original_controller(&self.context, attempt)?,
        )
    }

    fn summary(&self, exported: &Value) -> Result<(Value, frozen::FrozenExport)> {
        let backend = &self.context.backend;
        let verified = frozen::verify(backend, exported)?;
        let export = common::read_bound(backend, exported, "已复核共享导出")?;
        if verified.request != self.source["request"]
            || export["request"] != self.source["source_request"]
            || self.generation()? != verified.generation
        {
            return Err("共享导出的请求或保存代次与实际冻结来源不同".into());
        }
        self.storage()?;
        verified.check_proofs(backend, exported)?;
        let summary = recovery::summary_for(&export, &verified.generation, &verified.proof_files)?;
        Ok((summary, verified))
    }

    fn result(&self, number: u64, intent: &Value, export: &Value, summary: &Value) -> Value {
        json!({"status":"seed_source_export_published","origin_attempt":number,
            "origin_intent":intent,"source_registration":self.source["review_successor"]["source_result"],
            "source_rebind":self.source["source_rebind"],"source_generation":self.source["source_generation"],
            "review_successor":self.source["review_successor_binding"],"source_request":self.source["source_request"],
            "source_storage":self.source["storage"]["storage"],"export":export,"summary":summary,
            "remote_writes":0,"restore_qualified":false})
    }

    fn finish(&self, request: &StageRequest<'_>, before: &Value, current: &Value) -> Result<()> {
        self.held.check()?;
        self.scope.assert_unchanged()?;
        let saved = state::load(request.run_dir, true)?;
        if saved != *before
            || active(request, state::attempts(&saved)?)? != current
            || comparison::shared_export_source(request.backend, request.run_dir, &saved)?
                != *self.source
        {
            return Err("seed 导出期间发布来源、存储、生产者或执行阶段变化".into());
        }
        self.held.check()
    }
}

fn active<'a>(request: &StageRequest<'_>, rows: &'a [Value]) -> Result<&'a Value> {
    let row = rows.last().ok_or("共享 seed 导出缺少运行阶段")?;
    let number = number(row)?;
    if request.attempt != Some(number)
        || request.stage != "seed-runtime"
        || row["stage"] != request.stage
        || row["mode"] != request.mode
        || row["status"] != "running"
    {
        return Err("共享 seed 导出不属于当前登记阶段".into());
    }
    let (_, controller) = state::controller_record(request.run_dir, number, row)?;
    let (_, owner) = lock::owner_observation(request.run_dir)?.ok_or("共享 seed 导出缺少运行锁")?;
    if controller != owner || owner["identity"] != identity::current()? {
        return Err("共享 seed 导出要求当前进程持有同一运行锁".into());
    }
    Ok(row)
}

fn export_rows(rows: &[Value]) -> Vec<&Value> {
    rows.iter()
        .filter(|row| {
            row["stage"] == "seed-runtime"
                && matches!(row["mode"].as_str(), Some(EXPORT | RECONCILE))
        })
        .collect()
}

fn origin<'a>(prior: &[&'a Value], mode: &str) -> Result<Option<&'a Value>> {
    match (mode, prior) {
        (EXPORT, []) => Ok(None),
        (RECONCILE, [origin, rest @ ..])
            if origin["mode"] == EXPORT
                && origin["status"] == "failed"
                && rest
                    .iter()
                    .all(|row| row["mode"] == RECONCILE && row["status"] == "failed") =>
        {
            Ok(Some(origin))
        }
        (EXPORT, _) => {
            Err("seed 只允许一次 source-export；失败先 status/recover，再显式采用".into())
        }
        _ => Err("source-export-reconcile 只能采用唯一失败原导出；已发布后不得重复采用".into()),
    }
}

fn candidate(backend: &Path, run: &Path, record: &Value) -> Result<PathBuf> {
    let output = run.join(format!("e{:04}", number(record)?));
    match fs::symlink_metadata(output.join("failure.json")) {
        Ok(_) => return Err("原失败导出候选包含失败标记；禁止重新读取源".into()),
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    common::common_file(
        backend,
        &output.join("export.json"),
        "原失败导出缺少完整 export 候选；禁止重新读取源",
    )
}

fn number(row: &Value) -> Result<u64> {
    row["number"]
        .as_u64()
        .filter(|n| *n > 0)
        .ok_or_else(|| "共享导出阶段序号无效".into())
}

fn intent_path(run: &Path, number: u64) -> PathBuf {
    run.join(format!("export-{number:04}.intent.json"))
}

fn seal_path(run: &Path, number: u64) -> PathBuf {
    run.join(format!("export-{number:04}.verified.json"))
}

fn read_seal(backend: &Path, path: &Path) -> Result<Option<Value>> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            let path = common::common_file(backend, path, "原共享导出封存")?;
            Ok(Some(document::read(&path)?))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn check_prior_results(
    backend: &Path,
    run: &Path,
    prior: &[&Value],
    expected: &Value,
) -> Result<()> {
    for row in prior {
        if row["mode"] == RECONCILE && !row["result"].is_null() {
            let path = run.join(format!("results/{:04}.json", number(row)?));
            if common::bound_file(backend, &row["result"], "原共享导出采用结果")? != path
                || common::read_bound(backend, &row["result"], "原共享导出采用结果")? != *expected
            {
                return Err("共享导出重试采用结果与先前已写出的只读结果不同".into());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../tests/unit/recovery/clone/native/seed/shared_export_tests.rs"]
mod tests;
