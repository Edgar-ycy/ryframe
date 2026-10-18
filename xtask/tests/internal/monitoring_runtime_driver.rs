use std::{collections::BTreeMap, path::PathBuf};

use serde_json::{Map, Value};

use super::super::recovery::monitoring::{
    config,
    contract::{ROLES, VALIDATION_NAMES, file_descriptor},
    runtime::{Commands, Environment, Paths, Runtime},
    supervisor,
};

/// 只替换外部工具验证与就绪探测；生命周期和实际子进程清理仍使用产品实现。
pub(super) struct FakeRuntime {
    pub commands: BTreeMap<String, Vec<String>>,
    pub fail_role: Option<String>,
    pub supervisor_executable: PathBuf,
}

impl Runtime for FakeRuntime {
    fn validate(
        &self,
        _binding: &Value,
        paths: &Paths,
        _environment: &Environment,
    ) -> crate::Result<Value> {
        let mut receipts = Map::new();
        for (name, _) in VALIDATION_NAMES {
            let path = &paths[&format!("validate-{name}")];
            config::write_bytes_new(path, b"fixture validation\n")?;
            receipts.insert(name.to_owned(), file_descriptor(path)?);
        }
        Ok(Value::Object(receipts))
    }

    fn commands(&self, _binding: &Value, _paths: &Paths) -> crate::Result<Commands> {
        ROLES
            .into_iter()
            .map(|role| {
                self.commands
                    .get(role)
                    .cloned()
                    .map(|command| (role, command))
                    .ok_or_else(|| format!("fixture 缺少角色命令：{role}").into())
            })
            .collect()
    }

    fn ready(&self, _tree: &Value, _url: &str) -> crate::Result<()> {
        Ok(())
    }

    fn invocation(&self, protocol: &supervisor::Protocol) -> crate::Result<supervisor::Invocation> {
        if self.fail_role.as_deref() == Some(protocol.role.as_str()) {
            return Err(format!("fixture {} launch failure", protocol.role).into());
        }
        supervisor::supervisor_invocation_with_executable(protocol, &self.supervisor_executable)
    }
}
