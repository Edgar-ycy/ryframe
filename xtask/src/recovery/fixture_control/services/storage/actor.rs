//! 私有 actor 只读取本次固定 RustFS 请求，参数不携带应用凭据。
use super::*;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub(super) const ENTRY: &str = "__fixture-rustfs-actor";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Actor {
    pub(super) backend: PathBuf,
    pub(super) output: PathBuf,
    pub(super) request: Value,
    pub(super) private: Value,
    pub(super) manifest: Value,
    pub(super) controller: Value,
    pub(super) number: u64,
    pub(super) operation_id: String,
    pub(super) tool: Value,
    pub(super) snapshots: Vec<Value>,
    pub(super) owner_binding: Value,
}

impl Actor {
    pub(super) fn path(&self, name: &str) -> PathBuf {
        self.output.join(name)
    }
    pub(super) fn member_path(&self, suffix: &str) -> PathBuf {
        self.path(&format!(
            "rustfs-members-{}-{suffix}.json",
            self.operation_id
        ))
    }
    pub(super) fn timeout(&self) -> Result<u64> {
        self.request["timeout_seconds"]
            .as_u64()
            .filter(|v| *v == 60)
            .ok_or_else(|| "RustFS 期限无效".into())
    }
    pub(super) fn arguments(&self) -> Result<Vec<String>> {
        crate::recovery::clone::native::resources::storage::model::arguments(&self.request)
    }
    pub(super) fn verify(&self) -> Result<()> {
        self.verify_bound()?;
        // 私有入口不能把自签 actor 文件当授权；重新核对原审阅、bootstrap 和持锁阶段。
        let authorized = binding::prepare(&Start {
            backend: &self.backend,
            request: &self.request,
            private: &self.private,
            output: &self.output,
            manifest: &self.manifest,
            controller: &self.controller,
            number: self.number,
        })?;
        check(
            authorized.snapshots == self.snapshots
                && authorized.owner_binding == self.owner_binding,
            "RustFS actor 原始授权输入变化",
        )
    }
    pub(super) fn verify_bound(&self) -> Result<()> {
        check(
            self.operation_id.len() == 32
                && self
                    .operation_id
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
            "RustFS 操作 ID 无效",
        )?;
        check(
            std::fs::canonicalize(&self.backend)?
                == std::fs::canonicalize(crate::workspace::root_dir())?,
            "RustFS actor 不属于当前协调工作树",
        )?;
        common::local_directory(&self.backend, &self.output, "RustFS actor 输出")?;
        check(
            common::binding(&std::env::current_exe()?)? == self.tool,
            "RustFS actor 工具字节变化",
        )?;
        for snapshot in &self.snapshots {
            binding::verify_snapshot(snapshot)?;
        }
        binding::current_owner(self)?;
        check(
            document::read(&self.path("request.json"))? == self.request,
            "RustFS actor 请求内容变化",
        )?;
        common::directory_identity(&self.backend, &self.request["data_directory"])?;
        let values = self.private.as_object().ok_or("RustFS 私有环境无效")?;
        check(
            self.private["APP_ENV"] == "test"
                && values.iter().all(|(key, v)| {
                    ["APP_ENV", "TEMP", "TMP"].contains(&key.as_str()) && v.is_string()
                }),
            "RustFS 私有环境含非法覆盖",
        )?;
        self.timeout()?;
        Ok(())
    }
    pub(super) fn cancelled(&self) -> Result<bool> {
        if !self.path("actor-cancel.json").try_exists()? {
            return Ok(false);
        }
        let value = document::read(&self.path("actor-cancel.json"))?;
        document::exact(&value, &["operation_id"], "RustFS 启动取消")?;
        check(
            value["operation_id"] == self.operation_id,
            "RustFS 取消属于其他操作",
        )?;
        Ok(true)
    }
}

pub(super) fn entry() -> Option<i32> {
    let mut args = std::env::args().skip(1);
    if args.next()?.as_str() != ENTRY {
        return None;
    }
    let result = (|| {
        let operation = args.next().ok_or("RustFS actor 缺少固定操作")?;
        let encoded = args.next().ok_or("RustFS actor 缺少固定绑定")?;
        check(
            args.next().is_none() && encoded.len() <= 4096,
            "RustFS actor 参数无效",
        )?;
        check(
            matches!(operation.as_str(), "detach" | "monitor" | "supervise"),
            "RustFS actor 不接受其他角色或任意命令",
        )?;
        let descriptor = document::parse_fragment(&encoded, "RustFS actor 绑定")?;
        let path = crate::recovery::clone::native::lifecycle::manifest::bound_file(
            &crate::workspace::root_dir(),
            &descriptor,
            "RustFS actor",
            true,
        )?;
        let actor: Actor = serde_json::from_value(document::read(&path)?)?;
        platform::supported()?;
        check(
            path == actor.path("actor.json"),
            "RustFS actor 绑定不是本次固定文件",
        )?;
        actor.verify()?;
        match operation.as_str() {
            "detach" => launch::detach(&actor, &encoded),
            "monitor" => monitor::run(&actor, &encoded),
            "supervise" => supervisor::run(&actor),
            _ => Err("RustFS actor 不接受其他角色或任意命令".into()),
        }
    })();
    Some(if result.is_ok() { 0 } else { 1 })
}

pub(super) fn require(identity: &Value) -> Result<()> {
    super::super::lifecycle::readonly::process::require(identity)
}

pub(super) fn tree(actor: &Actor) -> Result<Value> {
    use super::super::lifecycle::{Runtime, readonly::NativeRuntime};
    let value = NativeRuntime::new(&actor.backend).call(
        "tree-read",
        json!({"output":actor.output,"scope_id":actor.request["scope_id"]}),
    )?;
    check(
        value["operation_id"] == actor.operation_id,
        "RustFS 树属于其他操作",
    )?;
    Ok(value)
}

pub(super) fn running_tree(actor: &Actor, expected: &Value) -> Result<()> {
    check(tree(actor)? == *expected, "RustFS 原监督树发生变化")?;
    let product = document::read(&actor.path("rustfs.json"))?;
    check(
        product
            == json!({"format_version":1,"role":"rustfs",
        "scope_id":actor.request["scope_id"],"identity":expected["process"]}),
        "RustFS 产品收据不同于原监督树",
    )?;
    for role in ["supervisor", "process", "monitor"] {
        require(&expected[role])?;
    }
    let arguments = super::super::lifecycle::readonly::process::arguments(&expected["process"])?;
    check(
        arguments == actor.arguments()?,
        "RustFS 当前完整参数不同于冻结请求",
    )
}

pub(super) fn validate_members(actor: &Actor, value: &Value) -> Result<()> {
    document::exact(
        value,
        &[
            "format_version",
            "operation_id",
            "directory",
            "role",
            "scope_id",
            "supervisor",
            "monitor",
            "status",
            "members",
            "error_type",
        ],
        "RustFS 完整成员",
    )?;
    check(
        value["format_version"] == 1
            && value["operation_id"] == actor.operation_id
            && value["directory"] == json!(actor.output)
            && value["role"] == "rustfs"
            && value["scope_id"] == actor.request["scope_id"]
            && value["status"] == "stopped"
            && value["error_type"].is_null(),
        "RustFS 完整成员关闭未确认",
    )?;
    let members = value["members"].as_array().ok_or("成员清单无效")?;
    check(
        members.contains(&value["supervisor"]),
        "完整成员缺少监督进程",
    )?;
    for member in members {
        common::validate_process_identity(member)?;
        let pid = u32::try_from(member["pid"].as_u64().ok_or("成员 PID 无效")?)?;
        check(
            common::process_identity(pid)?.is_none(),
            "原 RustFS 成员仍存活或 PID 已复用",
        )?;
    }
    let pid = u32::try_from(value["monitor"]["pid"].as_u64().ok_or("monitor PID 无效")?)?;
    check(
        common::process_identity(pid)?.is_none(),
        "RustFS monitor 尚未退出或 PID 已复用",
    )
}
