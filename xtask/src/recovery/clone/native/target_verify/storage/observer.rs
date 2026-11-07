use serde_json::Value;

use super::{StorageInputs, Transport, common, json_io, kernel};
use crate::{Result, recovery::fixture_control::services::lifecycle::readonly::process};

/// 文件证据仍由核验器读取；端口只提供固定目标的外部进程与协议事实。
pub(crate) trait StorageObserver {
    fn process_identity(&self, pid: u32) -> Result<Option<Value>>;
    fn arguments(&self, _context: &StorageInputs<'_>, identity: &Value) -> Result<Vec<String>>;
    fn listener(&self, _context: &StorageInputs<'_>, identity: &Value, url: &str) -> Result<()>;
    fn wsl(&self, context: &StorageInputs<'_>, redis: &Value, args: &[String]) -> Result<String>;
    fn redis(&self, context: &StorageInputs<'_>, redis: &Value, parts: &[&str]) -> Result<Value>;

    fn redis_window<T>(
        &self,
        context: &StorageInputs<'_>,
        redis: &Value,
        read: impl FnOnce(&mut dyn FnMut(&[&str]) -> Result<Value>) -> Result<T>,
    ) -> Result<T>
    where
        Self: Sized,
    {
        if redis["platform"] == "windows" {
            return Err("Windows Redis 读取窗口需要同一已核验连接".into());
        }
        read(&mut |parts| self.redis(context, redis, parts))
    }
}

pub(crate) struct SystemObserver<'a, T: Transport> {
    transport: &'a T,
}

impl<'a, T: Transport> SystemObserver<'a, T> {
    pub(crate) fn new(transport: &'a T) -> Self {
        Self { transport }
    }
}

impl<T: Transport> StorageObserver for SystemObserver<'_, T> {
    fn process_identity(&self, pid: u32) -> Result<Option<Value>> {
        common::process_identity(pid)
    }

    fn arguments(&self, _context: &StorageInputs<'_>, identity: &Value) -> Result<Vec<String>> {
        process::arguments(identity)
    }

    fn listener(&self, _context: &StorageInputs<'_>, identity: &Value, url: &str) -> Result<()> {
        process::listener(identity, url)
    }

    fn wsl(&self, context: &StorageInputs<'_>, redis: &Value, args: &[String]) -> Result<String> {
        let mut arguments = vec![
            "--distribution".into(),
            json_io::text(&redis["distribution"], "Redis distribution")?.into(),
            "--exec".into(),
        ];
        arguments.extend_from_slice(args);
        let program = json_io::text(&redis["wsl"]["path"], "Redis WSL 工具")?.into();
        kernel::observe(
            context,
            self.transport,
            program,
            arguments,
            "target-redis-kernel",
        )
        .map(|output| output.trim().to_owned())
    }

    fn redis(&self, context: &StorageInputs<'_>, redis: &Value, parts: &[&str]) -> Result<Value> {
        let mut client = super::super::redis::Client::authenticated(context, redis)?;
        client.command(parts)
    }

    fn redis_window<R>(
        &self,
        context: &StorageInputs<'_>,
        redis: &Value,
        read: impl FnOnce(&mut dyn FnMut(&[&str]) -> Result<Value>) -> Result<R>,
    ) -> Result<R> {
        if redis["platform"] != "windows" {
            return read(&mut |parts| self.redis(context, redis, parts));
        }
        #[cfg(windows)]
        {
            let mut client = super::super::redis::Client::authenticated(context, redis)?;
            let result = read(&mut |parts| client.command(parts));
            super::super::execution::combine(result, client.recheck_windows_peer())
        }
        #[cfg(not(windows))]
        Err("Windows Redis 读取窗口只能在本机 Windows 核验".into())
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, path::Path};

    use serde_json::json;

    use super::*;

    struct LegacyObserver(Cell<usize>);

    impl StorageObserver for LegacyObserver {
        fn process_identity(&self, _: u32) -> Result<Option<Value>> {
            unreachable!()
        }

        fn arguments(&self, _: &StorageInputs<'_>, _: &Value) -> Result<Vec<String>> {
            unreachable!()
        }

        fn listener(&self, _: &StorageInputs<'_>, _: &Value, _: &str) -> Result<()> {
            unreachable!()
        }

        fn wsl(&self, _: &StorageInputs<'_>, _: &Value, _: &[String]) -> Result<String> {
            unreachable!()
        }

        fn redis(&self, _: &StorageInputs<'_>, _: &Value, parts: &[&str]) -> Result<Value> {
            self.0.set(self.0.get() + 1);
            Ok(json!(parts))
        }
    }

    #[test]
    fn default_window_never_falls_back_to_individual_windows_connections() {
        let observer = LegacyObserver(Cell::new(0));
        let inputs = StorageInputs {
            backend: Path::new("."),
            request: &Value::Null,
            environment: &Default::default(),
            observation_dir: Path::new("."),
        };
        let read = |send: &mut dyn FnMut(&[&str]) -> Result<Value>| send(&["INFO", "server"]);
        let windows = observer.redis_window(&inputs, &json!({"platform":"windows"}), read);
        assert!(windows.is_err());
        assert_eq!(observer.0.get(), 0);
        let legacy = observer.redis_window(&inputs, &Value::Null, read).unwrap();
        assert_eq!(legacy, json!(["INFO", "server"]));
        assert_eq!(observer.0.get(), 1);
    }
}
