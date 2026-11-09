use serde_json::Value;

use super::{StorageInputs, common, redis_layout};
use crate::{Result, recovery::fixture_control::services::lifecycle::readonly::process};

/// 文件证据仍由核验器读取；端口只提供固定目标的外部进程与协议事实。
pub(crate) trait StorageObserver {
    fn process_identity(&self, pid: u32) -> Result<Option<Value>>;
    fn arguments(&self, _context: &StorageInputs<'_>, identity: &Value) -> Result<Vec<String>>;
    fn listener(&self, _context: &StorageInputs<'_>, identity: &Value, url: &str) -> Result<()>;
    // 历史夹具只从内存提供冻结的内核观察输出。
    #[cfg(test)]
    fn wsl(&self, _: &StorageInputs<'_>, _: &Value, _: &[String]) -> Result<String> {
        Err(redis_layout::WINDOWS_REQUIRED.into())
    }
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

pub(crate) struct SystemObserver;

impl StorageObserver for SystemObserver {
    fn process_identity(&self, pid: u32) -> Result<Option<Value>> {
        common::process_identity(pid)
    }

    fn arguments(&self, _context: &StorageInputs<'_>, identity: &Value) -> Result<Vec<String>> {
        process::arguments(identity)
    }

    fn listener(&self, _context: &StorageInputs<'_>, identity: &Value, url: &str) -> Result<()> {
        process::listener(identity, url)
    }

    fn redis(&self, context: &StorageInputs<'_>, redis: &Value, parts: &[&str]) -> Result<Value> {
        redis_layout::require_windows(redis)?;
        super::super::redis::Client::authenticated(context, redis)?.command(parts)
    }

    fn redis_window<R>(
        &self,
        context: &StorageInputs<'_>,
        redis: &Value,
        read: impl FnOnce(&mut dyn FnMut(&[&str]) -> Result<Value>) -> Result<R>,
    ) -> Result<R> {
        redis_layout::require_windows(redis)?;
        #[cfg(windows)]
        {
            let mut client = super::super::redis::Client::authenticated(context, redis)?;
            let result = read(&mut |parts| client.command(parts));
            super::super::execution::combine(result, client.recheck_windows_peer())
        }
        #[cfg(not(windows))]
        {
            let _ = (context, read);
            Err("Windows Redis 读取窗口只能在本机 Windows 核验".into())
        }
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
    #[test]
    fn live_observer_rejects_historical_redis_before_connecting() {
        let inputs = StorageInputs {
            backend: Path::new("."),
            request: &Value::Null,
            environment: &Default::default(),
            observation_dir: Path::new("."),
        };
        let legacy = json!({"wsl": {"path": "missing-wsl.exe"}, "distribution": "legacy"});
        let error = SystemObserver
            .redis_window(&inputs, &legacy, |_| -> Result<Value> {
                panic!("不应打开旧 Redis 连接")
            })
            .unwrap_err();
        assert_eq!(error.to_string(), redis_layout::WINDOWS_REQUIRED);
        let error = SystemObserver
            .redis(&inputs, &legacy, &["INFO", "server"])
            .unwrap_err();
        assert_eq!(error.to_string(), redis_layout::WINDOWS_REQUIRED);
    }
}
