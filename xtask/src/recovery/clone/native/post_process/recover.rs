//! 显式回收历史生产者，只接受用户选定的进程收据及原启动登记。

use std::path::Path;

use serde_json::{Value, json};

use super::{protocol, registration};
use crate::{
    Result,
    recovery::clone::native::{
        json as document,
        lifecycle::{
            identity,
            stage::{self, StageRequest, StageRunner},
            state,
        },
        resources::common,
        target_verify::guard::Inputs,
    },
};

pub(in crate::recovery::clone::native) fn run(
    backend: &Path,
    directory: &Path,
    stage_name: &'static str,
    binding: &Path,
) -> Result<Value> {
    let input = common::binding(&common::common_file(backend, binding, "会话回收绑定输入")?)?;
    let runner = Runner {
        stage: stage_name,
        input: &input,
    };
    let value = stage::run_named(backend, directory, stage_name, "recover-session", &runner)?;
    Ok(super::entry::summary(&value))
}

struct Runner<'a> {
    stage: &'static str,
    input: &'a Value,
}

impl StageRunner for Runner<'_> {
    fn preflight(&self, request: &StageRequest<'_>) -> Result<()> {
        if request.stage != self.stage
            || !matches!(self.stage, "post-copy" | "seed-runtime")
            || request.mode != "recover-session"
        {
            return Err("回收选项不属于明确的复制后或 seed 会话阶段".into());
        }
        descriptor(request.backend, self.input).map(|_| ())
    }

    fn execute(&self, request: &StageRequest<'_>) -> Result<Value> {
        self.preflight(request)?;
        let number = request.attempt.ok_or("回收缺少控制阶段序号")?;
        let saved = state::load(request.run_dir, false)?;
        let producer = descriptor(request.backend, self.input)?;
        let attempt = prior_attempt(request, number, &saved, &producer)?;
        let path = common::bound_file(request.backend, &producer, "用户选定的生产者")?;
        let launch = path.with_file_name("session-launch.json");
        common::common_file(request.backend, &launch, "生产者历史启动")?;
        let paths = [
            common::bound_file(request.backend, self.input, "回收绑定输入")?,
            path,
            launch.clone(),
            request.run_dir.join("state.json"),
        ];
        let protected = Inputs::open(paths)?;
        if descriptor(request.backend, self.input)? != producer
            || state::load(request.run_dir, false)? != saved
        {
            return Err("回收输入或原阶段在取得保护前变化".into());
        }
        let registration_binding = document::read(&launch)?["registration"].clone();
        let original = registration::resolve(
            request.backend,
            request.run_dir,
            request.stage,
            &registration_binding,
            attempt["number"].as_u64(),
            true,
        )?;
        let process = common::read_bound(request.backend, &producer, "历史原生进程收据")?;
        let identity_lock = protocol::identity_lock_request(
            request.backend,
            request.run_dir,
            attempt,
            &original,
            &process,
        )?;
        protected.check()?;
        let result = protocol::recover_session(&json!({
            "backend":request.backend,"run_dir":request.run_dir,"current_attempt":number,
            "descriptor":producer,"attempt":attempt,"registration":original,
            "registration_binding":registration_binding,"identity_lock":identity_lock,
            "controller_identity":identity::current()?
        }));
        super::files::combine(result, protected.check())
    }
}

fn descriptor(backend: &Path, input: &Value) -> Result<Value> {
    let producer = common::read_bound(backend, input, "用户提供的回收绑定")?;
    common::bound_file(backend, &producer, "用户选定的历史生产者进程")?;
    common::bound_file(backend, input, "用户提供的回收绑定后像")?;
    Ok(producer)
}

fn prior_attempt<'a>(
    request: &StageRequest<'_>,
    number: u64,
    saved: &'a Value,
    producer: &Value,
) -> Result<&'a Value> {
    let path = Path::new(document::text(&producer["path"], "生产者收据路径")?);
    let mut matching = state::attempts(saved)?.iter().filter(|attempt| {
        let Some(earlier) = attempt["number"].as_u64().filter(|value| *value < number) else {
            return false;
        };
        attempt["stage"] == request.stage
            && path
                == request
                    .run_dir
                    .join(request.stage)
                    .join(format!("attempt-{earlier:04}/session-process.json"))
    });
    let only = matching
        .next()
        .ok_or("回收只接受本运行历史阶段的明确生产者")?;
    if matching.next().is_some() {
        return Err("同一生产者收据匹配了多个历史阶段".into());
    }
    Ok(only)
}
