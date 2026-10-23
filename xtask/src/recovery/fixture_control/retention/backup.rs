use super::{
    super::{file_binding, write_new},
    database::Connection,
};
use crate::{Result, process::ChildGroup};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    process::Stdio,
    thread,
    time::{Duration, Instant},
};

pub(super) trait Exporter {
    fn export(&mut self, directory: &Path, migration: i64) -> Result<Value>;
}
impl Exporter for Connection {
    fn export(&mut self, directory: &Path, migration: i64) -> Result<Value> {
        let path = directory.join(format!("business-retention-{migration}.sql"));
        let output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            output.set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        let mut invocation = self.invocation(true)?;
        invocation.command.args([
            "--single-transaction",
            "--quick",
            "--skip-lock-tables",
            "--no-tablespaces",
            "--set-gtid-purged=OFF",
            "--skip-extended-insert",
            "--complete-insert",
            "--routines",
            "--events",
            "--triggers",
            "--hex-blob",
            "--databases",
            &self.schema,
        ]);
        let mut input = tempfile::tempfile_in(directory)?;
        input.write_all(&invocation.prefix)?;
        input.seek(SeekFrom::Start(0))?;
        invocation
            .command
            .stdin(Stdio::from(input))
            .stdout(Stdio::from(output))
            .stderr(Stdio::null());
        let mut child = ChildGroup::new()?.spawn(invocation.command)?;
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            if fs::metadata(&path)?.len() > 64 * 1024 * 1024 {
                return Err("专用目标导出超过 64 MiB；保留失败 SQL，不登记备份".into());
            }
            if let Some(status) = child.try_wait()? {
                if !status.success() {
                    return Err(format!("mysqldump 退出状态 {status}；保留失败 SQL，不登记备份；检查客户端、权限和连接").into());
                }
                break;
            }
            if Instant::now() >= deadline {
                return Err("mysqldump 超时；回收本次进程树，保留失败 SQL，不登记备份".into());
            }
            thread::sleep(Duration::from_millis(20));
        }
        drop(child);
        let binding = file_binding(&path, "专用目标导出")?;
        let mut bytes = Vec::new();
        fs::File::open(&path)?
            .take(64 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        validate_dump(&bytes)?;
        if binding != file_binding(&path, "专用目标导出")? {
            return Err("专用目标导出在摘要核验时漂移；不登记备份".into());
        }
        Ok(
            json!({"path":binding["path"],"bytes":binding["bytes"],"sha256":binding["sha256"],"database":self.schema,"method":"mysqldump-single-transaction","restored":false}),
        )
    }
}

pub(super) fn validate_dump(bytes: &[u8]) -> Result<()> {
    if bytes.is_empty() || bytes.len() > 64 * 1024 * 1024 {
        return Err("专用目标导出大小无效或超过 64 MiB 上限".into());
    }
    if !bytes
        .windows(b"CREATE TABLE `biz_order`".len())
        .any(|v| v == b"CREATE TABLE `biz_order`")
        || bytes
            .windows(b"INSERT INTO `biz_order` ".len())
            .filter(|v| *v == b"INSERT INTO `biz_order` ")
            .count()
            != 3
    {
        return Err("导出未包含业务 crate 结构和三条独立记录".into());
    }
    Ok(())
}

pub(super) fn verify(artifact: &Value, directory: &Path, migration: i64) -> Result<()> {
    let expected = directory.join(format!("business-retention-{migration}.sql"));
    if artifact["path"] != expected.to_string_lossy().as_ref() {
        return Err("备份文件不属于当前运行目录和迁移".into());
    }
    let binding = file_binding(&expected, "历史备份")?;
    if artifact["bytes"] != binding["bytes"] || artifact["sha256"] != binding["sha256"] {
        return Err("备份文件大小或摘要已变化".into());
    }
    Ok(())
}

pub(super) fn publish(path: &Path, value: &Value) -> Result<()> {
    write_new(path, value)
}
