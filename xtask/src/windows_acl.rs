//! Windows 私有文件 ACL：有限字节解析和同工具受限子进程，不读取凭据内容。

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[path = "windows_acl/descriptor.rs"]
mod descriptor;
#[cfg(windows)]
#[path = "windows_acl/native.rs"]
mod native;
#[cfg(windows)]
#[path = "windows_acl/protocol.rs"]
pub(crate) mod protocol;

#[cfg(windows)]
pub(crate) use protocol::{read, secure_directory};

const ERROR: &str = "Windows ACL 核验失败；保留现场，不自动重试或放宽权限";
const FULL_CONTROL: u32 = 0x001f_01ff;
const OBJECT_AND_CONTAINER_INHERIT: u8 = 0x03;
pub(crate) const SYNCHRONIZE: u32 = 0x0010_0000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Descriptor {
    current_sid: String,
    owner_sid: String,
    protected: bool,
    entries: Vec<Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Entry {
    sid: String,
    kind: u8,
    rights: u32,
    flags: u8,
}

impl Descriptor {
    pub(crate) fn monitoring_value(&self) -> Value {
        json!({
            "current_sid": self.current_sid,
            "owner_sid": self.owner_sid,
            "protected": self.protected,
            "entries": self.entries.iter().map(|entry| json!({
                "sid": entry.sid, "type": entry.kind, "rights": entry.rights,
                "inherited": entry.flags & 0x10 != 0,
            })).collect::<Vec<_>>(),
        })
    }

    pub(crate) fn verify_mysql(&self, newly_secured: bool) -> Result<(), &'static str> {
        if self.owner_sid != self.current_sid
            || self.entries.is_empty()
            || self.entries.iter().any(|entry| {
                entry.sid != self.current_sid || entry.kind != 0 || entry.rights != FULL_CONTROL
            })
            || (newly_secured
                && (!self.protected
                    || self
                        .entries
                        .iter()
                        .any(|entry| entry.flags != OBJECT_AND_CONTAINER_INHERIT)))
        {
            return Err(ERROR);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/unit/windows_acl.rs"]
mod tests;
