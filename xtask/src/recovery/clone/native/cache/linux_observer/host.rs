//! Windows 受管调用：显式双端路径、完整来源和 ELF 字节先后绑定。
use super::{FileProof, Identity, ImageProof, LIMIT, Observation, Request};
use crate::recovery::clone::native::{digest, json as document, source};
use crate::{Result, frontend_origin::manifest};
use ryframe_config::{LinuxObserverSettings, ToolSettings};
#[path = "host/owned.rs"]
pub(super) mod owned;
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    process::Stdio,
};

pub(in crate::recovery::clone::native::cache) struct Observer<'a> {
    backend: &'a Path,
    request: &'a Value,
    settings: LinuxObserverSettings,
    sources: Value,
    source: source::Inventory,
}

impl<'a> Observer<'a> {
    pub(in crate::recovery::clone::native::cache) fn new(
        backend: &'a Path,
        request: &'a Value,
    ) -> Result<Self> {
        let settings = configured(backend)?;
        validate(&settings)?;
        if request["distribution"] != settings.distribution {
            return Err("观察器发行版与原缓存请求不同".into());
        }
        let source = manifest::git_source(&settings.source_root, &settings.source_commit)?;
        let sources = manifest::capture(&settings.source_root)?;
        if manifest::sha256(&sources)? != settings.source_sha256 {
            return Err("观察器完整来源摘要不符".into());
        }
        let observer = Self {
            backend,
            request,
            settings,
            sources,
            source,
        };
        super::super::model::validate(backend, request, true)?;
        let description = observer.invoke(&["__frontend-origin", "describe"], &[])?;
        document::exact(
            &description,
            &["format_version", "kind", "executable", "sources"],
            "Linux 观察器来源",
        )?;
        document::exact(
            &description["executable"],
            &["path", "sha256"],
            "Linux 观察器产物",
        )?;
        if description["format_version"] != 1
            || description["kind"] != "frontend-origin-tool"
            || description["sources"] != observer.sources
            || description["executable"]["path"] != observer.settings.linux_program
            || description["executable"]["sha256"] != observer.settings.sha256
        {
            return Err("Linux 编译来源或双端二进制绑定不符".into());
        }
        Ok(observer)
    }

    pub(in crate::recovery::clone::native::cache) fn observe(
        &self,
        files: Vec<FileProof>,
        identity: Option<Identity>,
    ) -> Result<Observation> {
        let mut images = Vec::new();
        if self.request["format_version"] != 2 {
            images.push(serde_json::from_value::<ImageProof>(
                self.request["python"].clone(),
            )?);
        }
        let redis = ImageProof {
            path: document::text(&self.request["launcher"], "Redis 入口")?.into(),
            executable: document::text(&self.request["executable"], "Redis 映像")?.into(),
            sha256: document::text(&self.request["sha256"], "Redis 摘要")?.into(),
        };
        images.push(redis);
        let input = Request {
            format_version: 1,
            uid: self.settings.uid,
            program: self.settings.linux_program.clone(),
            sha256: self.settings.sha256.clone(),
            source_root: self.settings.linux_source_root.clone().into(),
            source_commit: self.settings.source_commit.clone(),
            source_sha256: self.settings.source_sha256.clone(),
            sources: self.sources.clone(),
            files,
            images,
            identity: identity.clone(),
            port: u16::try_from(self.request["port"].as_u64().ok_or("缓存端口无效")?)?,
        };
        validate(&self.settings)?;
        if configured(self.backend)? != self.settings {
            return Err("观察器配置在调用前变化".into());
        }
        let value = self.invoke(&["__linux-observer"], &serde_json::to_vec(&input)?)?;
        let result = decode(&self.settings, &identity, &value)?;
        self.finish()?;
        Ok(result)
    }

    pub(in crate::recovery::clone::native::cache) fn finish(&self) -> Result<()> {
        if configured(self.backend)? != self.settings
            || manifest::git_source(&self.settings.source_root, &self.settings.source_commit)?
                != self.source
            || manifest::capture(&self.settings.source_root)? != self.sources
        {
            return Err("观察器配置或完整源码在观察期间变化".into());
        }
        validate(&self.settings)?;
        super::super::model::validate(self.backend, self.request, true)
    }

    fn invoke(&self, arguments: &[&str], input: &[u8]) -> Result<Value> {
        if input.len() > LIMIT {
            return Err("Linux 观察器输入超限".into());
        }
        let local = self.backend.join(".local-tests");
        fs::create_dir_all(&local)?;
        let mut stdin = tempfile::tempfile_in(&local)?;
        stdin.write_all(input)?;
        stdin.seek(SeekFrom::Start(0))?;
        let mut stdout = tempfile::tempfile_in(&local)?;
        let wsl = document::text(&self.request["wsl"]["path"], "WSL 工具")?;
        let mut command = crate::process::child_command(wsl);
        command
            .args([
                "--distribution",
                &self.settings.distribution,
                "--user",
                &self.settings.user,
                "--exec",
                &self.settings.linux_program,
            ])
            .args(arguments)
            .current_dir(self.backend)
            .stdin(Stdio::from(stdin))
            .stdout(Stdio::from(stdout.try_clone()?))
            .stderr(Stdio::null());
        let measure = stdout.try_clone()?;
        let mut child = command
            .spawn()
            .map_err(|_| "无法启动已绑定 Linux 观察器；核对工具配置")?;
        owned::run(
            &mut child,
            || measure.metadata().map(|metadata| metadata.len()),
            |status| {
                if !status.success() {
                    return Err(owned::Cause::Exit(status.code()));
                }
                stdout
                    .seek(SeekFrom::Start(0))
                    .map_err(|error| owned::Cause::Read(error.kind()))?;
                let mut bytes = Vec::new();
                stdout
                    .take((LIMIT + 1) as u64)
                    .read_to_end(&mut bytes)
                    .map_err(|error| owned::Cause::Read(error.kind()))?;
                if bytes.len() > LIMIT {
                    return Err(owned::Cause::OutputLimit);
                }
                parse_output(true, &bytes).map_err(|_| owned::Cause::Protocol)
            },
        )
    }
}

fn configured(backend: &Path) -> Result<LinuxObserverSettings> {
    crate::recovery::runtime::dataset_authority::reads::tool_settings(backend)?;
    ToolSettings::load(backend)?.verification.linux_observer.ok_or_else(||
        super::super::rejected("需要 WSL 观察：请在 config/tools.toml 或 .local-tests/tools.toml 配置 [verification.linux_observer] 的 distribution/user/uid、双端 program/source_root、source_commit/sha256/source_sha256；从明确 SHA 的普通 clone 构建受管 xtask，禁止回退解释器"))
}

pub(in crate::recovery::clone::native::cache) fn validate_paths(
    settings: &LinuxObserverSettings,
) -> Result<()> {
    let name = |value: &str| {
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    };
    let linux_path = |value: &str| {
        value.starts_with('/')
            && value.len() <= 4096
            && !value.contains(['\\', '\0', '\r', '\n'])
            && !value
                .split('/')
                .skip(1)
                .any(|part| matches!(part, "" | "." | ".."))
    };
    if !name(&settings.distribution)
        || !name(&settings.user)
        || !linux_path(&settings.linux_program)
        || !linux_path(&settings.linux_source_root)
        || !manifest::hex_digest(&settings.source_commit, 40)
        || !manifest::hex_digest(&settings.sha256, 64)
        || !manifest::hex_digest(&settings.source_sha256, 64)
    {
        return Err(
            "verification.linux_observer 字段无效；核对发行版、用户、完整绝对路径及小写摘要".into(),
        );
    }
    for path in [&settings.program, &settings.source_root] {
        if !path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
            || path
                .to_str()
                .is_none_or(|value| value.contains(['\0', '\r', '\n']))
            || digest::linked_path(path)?.contains(&true)
        {
            return Err("观察器 Windows 路径必须是无链接的完整绝对路径".into());
        }
    }
    Ok(())
}

pub(in crate::recovery::clone::native::cache) fn validate(
    settings: &LinuxObserverSettings,
) -> Result<()> {
    validate_paths(settings)?;
    let mut file = fs::File::open(&settings.program)?;
    let mut header = [0_u8; 20];
    file.read_exact(&mut header)?;
    if &header[..4] != b"\x7fELF"
        || header[4] != 2
        || header[5] != 1
        || u16::from_le_bytes([header[18], header[19]]) != 62
        || digest::file(&settings.program)?.sha256 != settings.sha256
    {
        return Err("观察器必须是与 sha256 一致的 x86_64 Linux ELF 普通文件".into());
    }
    Ok(())
}

fn decode(
    settings: &LinuxObserverSettings,
    identity: &Option<Identity>,
    value: &Value,
) -> Result<Observation> {
    document::exact(value, &["ok", "result"], "Linux 观察器响应")?;
    if value["ok"] != true {
        return Err("Linux 观察器拒绝请求".into());
    }
    document::exact(
        &value["result"],
        &[
            "format_version",
            "uid",
            "program",
            "sha256",
            "source_commit",
            "source_sha256",
            "boot_id",
            "identity",
            "alive",
            "bound_and_closed",
        ],
        "Linux 观察器结果",
    )?;
    let result: Observation = serde_json::from_value(value["result"].clone())?;
    if result.format_version != 1
        || result.uid != settings.uid
        || result.program != settings.linux_program
        || result.sha256 != settings.sha256
        || result.source_commit != settings.source_commit
        || result.source_sha256 != settings.source_sha256
        || result.identity != *identity
        || result.alive == result.bound_and_closed
        || (result.alive && identity.is_none())
        || !super::super::model::boot(&json!(result.boot_id))?
    {
        return Err("Linux 观察器响应与绑定请求或关闭证明不同".into());
    }
    Ok(result)
}

fn parse_output(success: bool, bytes: &[u8]) -> Result<Value> {
    if !success {
        return Err("Linux 观察器失败；核对绑定、冻结证据或端口占用，禁止自动重放".into());
    }
    if bytes.is_empty() || bytes.len() > LIMIT {
        return Err("Linux 观察器响应为空或超限".into());
    }
    std::str::from_utf8(bytes)
        .map_err(|_| "Linux 观察器响应不是 UTF-8".into())
        .and_then(|text| {
            document::parse_fragment(text, "Linux 观察器响应")
                .map_err(|_| "Linux 观察器响应无效；不回显私有输出".into())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observation_maps_only_complete_bound_identity_and_exclusive_closure_results() {
        let settings = LinuxObserverSettings {
            distribution: "Ubuntu-24.04".into(),
            user: "fixture".into(),
            uid: 1000,
            program: "D:/tool/xtask".into(),
            linux_program: "/mnt/d/tool/xtask".into(),
            source_root: "D:/source".into(),
            linux_source_root: "/mnt/d/source".into(),
            source_commit: "a".repeat(40),
            sha256: "b".repeat(64),
            source_sha256: "c".repeat(64),
        };
        let identity = Some(Identity {
            pid: 300,
            started: "60".into(),
            executable: "/usr/bin/redis-server".into(),
            boot_id: "11111111-1111-1111-1111-111111111111".into(),
        });
        let value = json!({"ok":true,"result":{"format_version":1,"uid":settings.uid,
            "program":settings.linux_program,"sha256":settings.sha256,"source_commit":settings.source_commit,
            "source_sha256":settings.source_sha256,"boot_id":identity.as_ref().unwrap().boot_id,
            "identity":identity,"alive":false,"bound_and_closed":true}});
        assert!(
            decode(&settings, &identity, &value)
                .unwrap()
                .bound_and_closed
        );
        let mut live = value.clone();
        live["result"]["alive"] = json!(true);
        live["result"]["bound_and_closed"] = json!(false);
        assert!(decode(&settings, &identity, &live).unwrap().alive);
        let mut partial = value.clone();
        partial["result"]["identity"] = Value::Null;
        assert!(decode(&settings, &None, &partial).unwrap().bound_and_closed);
        let mut previous_boot = value.clone();
        previous_boot["result"]["boot_id"] = json!("22222222-2222-2222-2222-222222222222");
        assert!(
            decode(&settings, &identity, &previous_boot)
                .unwrap()
                .bound_and_closed
        );
        for (field, invalid) in [
            ("alive", json!(0)),
            ("bound_and_closed", json!(false)),
            ("boot_id", json!("unknown")),
            ("uid", json!(1001)),
            ("program", json!("/other")),
            ("sha256", json!("d".repeat(64))),
            ("source_commit", json!("d".repeat(40))),
            ("source_sha256", json!("d".repeat(64))),
            ("identity", json!({"pid":301})),
            ("terminated", json!(true)),
        ] {
            let mut changed = value.clone();
            changed["result"][field] = invalid;
            assert!(decode(&settings, &identity, &changed).is_err(), "{field}");
        }
        for field in ["identity", "bound_and_closed"] {
            let mut changed = value.clone();
            changed["result"].as_object_mut().unwrap().remove(field);
            assert!(decode(&settings, &identity, &changed).is_err());
        }
        let mut changed = value.clone();
        changed["result"]["identity"]["started"] = json!("61");
        assert!(decode(&settings, &identity, &changed).is_err());
        changed["ok"] = json!(false);
        assert!(decode(&settings, &identity, &changed).is_err());
        assert!(decode(&settings, &None, &live).is_err());
    }

    #[test]
    fn observation_transport_rejects_failure_duplicate_truncated_and_excess_output() {
        assert!(parse_output(false, b"{\"ok\":true,\"result\":{}}").is_err());
        for input in [
            b"{\"ok\":true,\"ok\":false,\"secret\":\"private-value\"}".to_vec(),
            b"{\"ok\":true".to_vec(),
            b"{} {}".to_vec(),
            vec![0xff],
            vec![b' '; LIMIT + 1],
        ] {
            let error = parse_output(true, &input).unwrap_err().to_string();
            assert!(!error.contains("private-value"));
        }
    }
}
