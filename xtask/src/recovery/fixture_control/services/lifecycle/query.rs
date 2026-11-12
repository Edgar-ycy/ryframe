//! 既有私有协议的服务只读操作，复用唯一 Rust 上下文与投影。
use super::*;

pub(crate) fn dispatch(backend: &Path, operation: &str, args: &Value) -> Result<Value> {
    match operation {
        "service-registered" => {
            exact(args, &["value", "settled"])?;
            let mut runtime = readonly::NativeRuntime::new(backend);
            registration::load(
                &mut runtime,
                backend,
                &args["value"],
                args["settled"].as_bool().ok_or("settled 必须为布尔值")?,
            )
        }
        "service-observe" => {
            exact(args, &["services"])?;
            let mut runtime = readonly::NativeRuntime::new(backend);
            observation::observe(&mut runtime, backend, &args["services"])
        }
        "service-run" => {
            exact(args, &["review"])?;
            Ok(json!(context::service_run(&args["review"])?))
        }
        "service-document" => {
            exact(args, &["path"])?;
            let (path, value) = context::document(backend, Path::new(text(args, "path")?))?;
            Ok(json!([path, value]))
        }
        "service-context" => {
            exact(args, &["review", "environment"])?;
            let mut runtime = readonly::NativeRuntime::new(backend);
            context::load(
                &mut runtime,
                backend,
                Path::new(text(args, "review")?),
                Path::new(text(args, "environment")?),
            )
        }
        "service-environment" => environment(backend, args),
        "service-prepared" => {
            exact(args, &["path", "secret_files"])?;
            prepared::load(
                backend,
                Path::new(text(args, "path")?),
                &args["secret_files"],
            )
        }
        "service-guard" => {
            exact(args, &["value", "running"])?;
            let running = if args["running"].is_null() {
                None
            } else {
                Some(&args["running"])
            };
            context::guarded(backend, &args["value"], running)?;
            Ok(Value::Null)
        }
        "service-transition" => {
            exact(args, &["run", "expected"])?;
            let mut runtime = readonly::NativeRuntime::new(backend);
            projection::transition(
                &mut runtime,
                backend,
                Path::new(text(args, "run")?),
                &args["expected"],
            )
        }
        _ => Err(model::error("未知夹具服务只读操作")),
    }
}

fn exact(args: &Value, keys: &[&str]) -> Result<()> {
    json_io::exact(args, keys, "夹具服务只读协议").map(|_| ())
}

fn environment(backend: &Path, args: &Value) -> Result<Value> {
    exact(args, &["review_file", "review", "environment"])?;
    let (review_file, review) = context::document(backend, Path::new(text(args, "review_file")?))?;
    check(review == args["review"], "夹具审阅与传入快照不同")?;
    let mut runtime = readonly::NativeRuntime::new(backend);
    let (path, value, private) = context::environment(
        &mut runtime,
        backend,
        &review_file,
        &review,
        Path::new(text(args, "environment")?),
    )?;
    Ok(json!([path, value, private]))
}

/// 原生 prepare 只读核定全部历史代次，不增加跨进程操作或控制权限。
pub(crate) fn storage_history(backend: &Path, run: &Path) -> Result<Value> {
    let mut runtime = readonly::NativeRuntime::new(backend);
    projection::historical(&mut runtime, backend, run)
}

/// 为已冻结 arm 代次复用原服务进程、监听和认证观察。
pub(in crate::recovery) fn observe_arm_generation(
    backend: &Path,
    run: &Path,
    generation: &Value,
) -> Result<()> {
    let mut runtime = readonly::NativeRuntime::new(backend);
    projection::observe_current(&mut runtime, backend, run, generation)
}
