//! 完整 Windows Redis 代次只发送一次关闭，之后仅观察原进程与原退出证明。
use super::{
    super::{
        diagnostic::stop::{Stage, capture},
        facts, host_control,
    },
    bound_path, common, document, observe, publish,
};
use crate::Result;
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, ErrorKind},
    path::Path,
    thread,
    time::{Duration, Instant},
};

pub(in crate::recovery::clone::native::cache) fn run(
    backend: &Path,
    request: &Value,
    runtime: &Value,
    output: &Path,
    mut guard: impl FnMut() -> Result<()>,
    wait_port: bool,
) -> Result<Value> {
    let mut stop = Stop {
        backend,
        request,
        runtime,
        deadline: Instant::now() + Duration::from_secs(180),
        phase: Stage::Inputs,
        sent: false,
        exited: false,
    };
    let outcome = stop.execute(output, &mut guard, wait_port);
    outcome.map_err(|error| capture(stop.phase, stop.sent, stop.exited, error))
}

struct Stop<'a> {
    backend: &'a Path,
    request: &'a Value,
    runtime: &'a Value,
    deadline: Instant,
    phase: Stage,
    sent: bool,
    exited: bool,
}

impl Stop<'_> {
    fn execute(
        &mut self,
        output: &Path,
        guard: &mut impl FnMut() -> Result<()>,
        wait_port: bool,
    ) -> Result<Value> {
        common::local_directory(self.backend, output, "Windows Redis 停止输出")?;
        let stopped = output.join("stopped.json");
        match fs::symlink_metadata(&stopped) {
            Ok(_) => return Err("Windows Redis 停止收据已存在，禁止重放".into()),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        self.guarded(guard)?;
        let state = self.status()?;
        if self.runtime["state"] == "failed_cleaned" {
            if state["state"] != "stopped"
                || fs::canonicalize(output)?
                    == fs::canonicalize(bound_path(self.runtime, "output")?)?
            {
                return Err("Windows Redis 失败代次停止必须保留原运行目录和原证明".into());
            }
            return self.publish_failed_stop(&stopped, guard);
        }
        let exited = match state["state"].as_str() {
            Some("stopped") => {
                self.phase = Stage::Exit;
                let binding = self.exited_binding()?;
                self.exited = true;
                binding
            }
            Some("running") => self.shutdown(guard, wait_port)?,
            _ => return Err("Windows Redis 完整代次的运行状态未确认".into()),
        };
        self.phase = Stage::Guard;
        self.guarded(guard)?;
        self.require_same_exit(&exited)?;
        let result = json!({
            "state":"stopped","status":"redis_process_stopped","runtime":self.runtime,
            "resources_deleted":false,"windows":{"identity":self.runtime["process"],"alive":false},
            "exited":exited,
        });
        self.phase = Stage::Publish;
        self.expired()?;
        publish(&stopped, &result)?;
        self.phase = Stage::Guard;
        self.guarded(guard)?;
        self.require_same_exit(&exited)?;
        self.expired()?;
        Ok(result)
    }

    fn publish_failed_stop(
        &mut self,
        stopped: &Path,
        guard: &mut impl FnMut() -> Result<()>,
    ) -> Result<Value> {
        self.phase = Stage::Guard;
        self.guarded(guard)?;
        observe::check_failed(self.backend, self.request, self.runtime, false)?;
        let result = json!({"state":"stopped","status":"redis_process_stopped","runtime":self.runtime,
            "resources_deleted":false,"windows_failure":self.runtime["windows_failure"]});
        self.phase = Stage::PartialPublish;
        self.expired()?;
        publish(stopped, &result)?;
        self.phase = Stage::Guard;
        self.guarded(guard)?;
        observe::check_failed(self.backend, self.request, self.runtime, false)?;
        self.expired()?;
        Ok(result)
    }

    fn shutdown(
        &mut self,
        guard: &mut impl FnMut() -> Result<()>,
        wait_port: bool,
    ) -> Result<Value> {
        self.phase = Stage::Guard;
        self.guarded(guard)?;
        self.phase = Stage::Stop;
        let send_error = match facts::shutdown_windows(
            self.backend,
            self.request,
            &self.runtime["process"],
            &self.runtime["facts"],
            &self.runtime["redis"]["run_id"],
            self.deadline,
            &mut self.sent,
        ) {
            Ok(()) if self.sent => None,
            Ok(()) => return Err("Windows Redis 关闭未登记发送尝试".into()),
            Err(error) if self.sent => Some(error),
            Err(error) => return Err(error),
        };
        // 发出后只读收口；退出无法确认时保留首次发送错误，不重发命令。
        match (send_error, self.confirm(guard, wait_port)) {
            (_, Ok(binding)) => Ok(binding),
            (None, Err(error)) => Err(error),
            (Some(error), Err(_)) => {
                self.phase = Stage::Stop;
                Err(error)
            }
        }
    }

    fn confirm(
        &mut self,
        guard: &mut impl FnMut() -> Result<()>,
        wait_port: bool,
    ) -> Result<Value> {
        self.phase = Stage::Exit;
        loop {
            self.guarded(guard)?;
            if !self.processes_present()? {
                self.expired()?;
                break;
            }
            self.pause(Duration::from_millis(50));
        }
        let exited = self.exited_binding()?;
        self.exited = true;
        self.phase = Stage::Port;
        self.wait_for_port(&exited, guard, wait_port)?;
        self.phase = Stage::Guard;
        self.guarded(guard)?;
        if self.status()?["state"] != "stopped" {
            return Err("Windows Redis 端口释放后未证明原代次完整关闭".into());
        }
        self.require_same_exit(&exited)?;
        self.expired()?;
        Ok(exited)
    }

    fn wait_for_port(
        &mut self,
        exited: &Value,
        guard: &mut impl FnMut() -> Result<()>,
        wait_port: bool,
    ) -> Result<()> {
        let mut last = None;
        while Instant::now() < self.deadline {
            guard()?;
            if self.processes_present()? {
                return Err("Windows Redis 端口释放期间原进程状态变化".into());
            }
            self.require_same_exit(exited)?;
            if Instant::now() >= self.deadline {
                break;
            }
            match host_control::bind_after_stop(self.request, self.sent) {
                Ok(()) if Instant::now() < self.deadline => return Ok(()),
                Ok(()) => break,
                Err(error)
                    if wait_port
                        && error
                            .downcast_ref::<io::Error>()
                            .is_some_and(|error| error.kind() == ErrorKind::AddrInUse) =>
                {
                    last = Some(error);
                }
                Err(error) => return Err(error),
            }
            self.pause(Duration::from_millis(500));
        }
        self.phase = Stage::PortReleaseTimeout;
        Err(last.unwrap_or_else(|| io::Error::from(ErrorKind::TimedOut).into()))
    }

    fn processes_present(&self) -> Result<bool> {
        let supervisor = observe::present(&self.runtime["supervisor"])?;
        let product = observe::present(&self.runtime["process"])?;
        Ok(supervisor || product)
    }

    fn exited_binding(&self) -> Result<Value> {
        let path = bound_path(self.runtime, "output")?.join("exited.json");
        let path = common::common_file(self.backend, &path, "Windows Redis 原退出收据")?;
        let binding = common::binding(&path)?;
        let exited = document::read(&path)?;
        document::exact(
            &exited,
            &["process", "returncode", "job_empty"],
            "Windows Redis 原退出收据",
        )?;
        if exited["process"] != self.runtime["process_receipt"]
            || exited["job_empty"] != true
            || exited["returncode"] != 0
            || common::binding(&path)? != binding
        {
            return Err("Windows Redis 原进程退出收据不完整或变化".into());
        }
        Ok(binding)
    }

    fn require_same_exit(&self, binding: &Value) -> Result<()> {
        if self.exited_binding()? != *binding {
            return Err("Windows Redis 原退出收据在关闭期间变化".into());
        }
        Ok(())
    }

    fn status(&self) -> Result<Value> {
        self.expired()?;
        let state = observe::status(self.backend, self.request, self.runtime, None)?;
        self.expired()?;
        Ok(state)
    }

    fn guarded(&self, guard: &mut impl FnMut() -> Result<()>) -> Result<()> {
        self.expired()?;
        guard()?;
        self.expired()
    }

    fn expired(&self) -> Result<()> {
        if Instant::now() >= self.deadline {
            return Err(io::Error::from(ErrorKind::TimedOut).into());
        }
        Ok(())
    }

    fn pause(&self, duration: Duration) {
        thread::sleep(duration.min(self.deadline.saturating_duration_since(Instant::now())));
    }
}
