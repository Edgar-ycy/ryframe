//! Linux 内核观察，不发送信号，不创建进程或账本。
use super::{FileProof, Identity, Observation, Request};
use crate::recovery::clone::native::digest;
use crate::{Result, frontend_origin::manifest};
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
    net::{Ipv4Addr, SocketAddrV4},
    path::Path,
};

pub(super) fn observe(request: &Request) -> Result<Observation> {
    observe_with(request, || Ok(()))
}

fn observe_with(
    request: &Request,
    after_observation: impl FnOnce() -> Result<()>,
) -> Result<Observation> {
    let uid = nix::unistd::geteuid().as_raw();
    if uid != request.uid {
        return Err("Linux 观察器实际 UID 不符".into());
    }
    let program = fs::read_link("/proc/self/exe")?;
    if program != Path::new(&request.program) || digest::file(&program)?.sha256 != request.sha256 {
        return Err("Linux 观察器实际二进制不符".into());
    }
    let source = manifest::git_source(&request.source_root, &request.source_commit)?;
    manifest::verify_compiled(&request.source_root, &request.sources)?;
    if manifest::sha256(&request.sources)? != request.source_sha256 {
        return Err("Linux 观察器完整来源摘要不符".into());
    }
    let before = boot()?;
    let held = frozen(request)?;
    let first = request
        .identity
        .as_ref()
        .map(identity)
        .transpose()?
        .flatten();
    let second = request
        .identity
        .as_ref()
        .map(identity)
        .transpose()?
        .flatten();
    if first != second {
        return Err("Linux 进程在观察窗口变化".into());
    }
    let alive = request.identity.is_some() && second == request.identity;
    if !alive {
        bind_and_close(request.port)?;
    }
    after_observation()?;
    for (proof, file) in request.files.iter().zip(held.iter()) {
        verify_file(proof, file)?;
    }
    frozen(request)?;
    manifest::verify_compiled(&request.source_root, &request.sources)?;
    if manifest::git_source(&request.source_root, &request.source_commit)? != source
        || digest::file(&program)?.sha256 != request.sha256
        || boot()? != before
        || request
            .identity
            .as_ref()
            .map(identity)
            .transpose()?
            .flatten()
            != second
    {
        return Err("Linux 观察器来源、产物或 boot 在观察期间变化".into());
    }
    Ok(Observation {
        format_version: 1,
        uid,
        program: request.program.clone(),
        sha256: request.sha256.clone(),
        source_commit: request.source_commit.clone(),
        source_sha256: request.source_sha256.clone(),
        boot_id: before,
        identity: request.identity.clone(),
        alive,
        bound_and_closed: !alive,
    })
}

fn frozen(request: &Request) -> Result<Vec<fs::File>> {
    let mut held = Vec::new();
    for proof in &request.files {
        let path = Path::new(&proof.path);
        plain(path)?;
        let file = fs::File::open(path)?;
        verify_file(proof, &file)?;
        held.push(file);
    }
    for image in &request.images {
        let path = Path::new(&image.executable);
        plain(path)?;
        if fs::canonicalize(&image.path)? != path || digest::file(path)?.sha256 != image.sha256 {
            return Err("Linux 历史工具路径或摘要漂移".into());
        }
    }
    Ok(held)
}

fn plain(path: &Path) -> Result<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
        || fs::canonicalize(path)? != path
        || digest::linked_path(path)?.contains(&true)
    {
        return Err("Linux 冻结路径必须是实际绝对路径且不经过链接".into());
    }
    Ok(())
}

fn verify_file(proof: &FileProof, file: &fs::File) -> Result<()> {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::MetadataExt;
    let path = Path::new(&proof.path);
    let before = file.metadata()?;
    let current = fs::metadata(path)?;
    if !before.is_file()
        || before.dev() != current.dev()
        || before.ino() != current.ino()
        || before.len() != proof.bytes
    {
        return Err("Linux 冻结文件身份不符".into());
    }
    let mut stream = file.try_clone()?;
    stream.seek(SeekFrom::Start(0))?;
    let mut digest = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0; 65536];
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        bytes += read as u64;
        if bytes > proof.bytes {
            return Err("Linux 冻结文件增长".into());
        }
        digest.update(&buffer[..read]);
    }
    let after = fs::metadata(path)?;
    if bytes != proof.bytes
        || hex::encode(digest.finalize()) != proof.sha256
        || (
            before.dev(),
            before.ino(),
            before.len(),
            before.mtime_nsec(),
            before.ctime_nsec(),
            before.mtime(),
            before.ctime(),
        ) != (
            after.dev(),
            after.ino(),
            after.len(),
            after.mtime_nsec(),
            after.ctime_nsec(),
            after.mtime(),
            after.ctime(),
        )
    {
        return Err("Linux 冻结文件字节或身份变化".into());
    }
    Ok(())
}

fn boot() -> Result<String> {
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id")?
        .trim()
        .to_owned();
    if !super::super::model::boot(&serde_json::json!(boot))? {
        return Err("Linux boot 无效".into());
    }
    Ok(boot)
}

fn identity(expected: &Identity) -> Result<Option<Identity>> {
    if expected.pid <= 1
        || !expected.started.bytes().all(|byte| byte.is_ascii_digit())
        || expected.started.is_empty()
        || !Path::new(&expected.executable).is_absolute()
        || !super::super::model::boot(&serde_json::json!(expected.boot_id))?
    {
        return Err("Linux 创建身份无效".into());
    }
    let boot = boot()?;
    let path = format!("/proc/{}", expected.pid);
    let stat = match fs::read_to_string(format!("{path}/stat")) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let fields = stat
        .rsplit_once(')')
        .ok_or("Linux stat 无名称边界")?
        .1
        .split_whitespace()
        .collect::<Vec<_>>();
    let started = fields.get(19).ok_or("Linux stat 无创建时间")?.to_string();
    let executable = fs::read_link(format!("{path}/exe"))?
        .to_str()
        .ok_or("Linux 映像不是 UTF-8")?
        .to_owned();
    let actual = Identity {
        pid: expected.pid,
        started,
        executable,
        boot_id: boot,
    };
    if actual.boot_id == expected.boot_id && actual != *expected {
        return Err("Linux PID 已复用，不能认定原代次已关闭".into());
    }
    Ok(Some(actual))
}

fn bind_and_close(port: u16) -> Result<()> {
    // 不启用 SO_REUSEADDR/SO_REUSEPORT；bind 自身证明独占，不依赖 LISTEN 或 connect。
    let socket = tokio::net::TcpSocket::new_v4()?;
    socket.set_reuseaddr(false)?;
    socket.bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into())?;
    drop(socket);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn bound_without_listen_blocks_closure_and_drop_releases_the_port() {
        let held = tokio::net::TcpSocket::new_v4().unwrap();
        held.bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0).into())
            .unwrap();
        let port = held.local_addr().unwrap().port();
        assert!(bind_and_close(port).is_err());
        drop(held);
        bind_and_close(port).unwrap();
        bind_and_close(port).unwrap();
    }

    #[test]
    fn live_identity_and_frozen_bytes_are_observed_without_control_side_effects() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let inventory = crate::recovery::clone::native::source::capture(root).unwrap();
        let sources = manifest::capture(root).unwrap();
        let program = fs::read_link("/proc/self/exe").unwrap();
        let pid = std::process::id();
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        let started = stat
            .rsplit_once(')')
            .unwrap()
            .1
            .split_whitespace()
            .nth(19)
            .unwrap()
            .into();
        let local = root.join(".local-tests");
        fs::create_dir_all(&local).unwrap();
        let file = tempfile::NamedTempFile::new_in(local).unwrap();
        let proof = FileProof {
            path: file.path().to_str().unwrap().into(),
            bytes: 0,
            sha256: digest::file(file.path()).unwrap().sha256,
        };
        let mut request = Request {
            format_version: 1,
            uid: nix::unistd::geteuid().as_raw(),
            program: program.to_str().unwrap().into(),
            sha256: digest::file(&program).unwrap().sha256,
            source_root: root.into(),
            source_commit: inventory.source.snapshot.head,
            source_sha256: manifest::sha256(&sources).unwrap(),
            sources,
            files: vec![proof],
            images: vec![],
            identity: Some(Identity {
                pid,
                started,
                executable: program.to_str().unwrap().into(),
                boot_id: boot().unwrap(),
            }),
            port: 16390,
        };
        let result = observe(&request).unwrap();
        assert!(result.alive);
        assert!(!result.bound_and_closed);
        assert_eq!(json!(result.identity), json!(request.identity));
        let recorded = request.identity.clone().unwrap();
        let held = tokio::net::TcpSocket::new_v4().unwrap();
        held.bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0).into())
            .unwrap();
        request.port = held.local_addr().unwrap().port();
        drop(held);
        let previous_boot = if recorded.boot_id == "11111111-1111-1111-1111-111111111111" {
            "22222222-2222-2222-2222-222222222222"
        } else {
            "11111111-1111-1111-1111-111111111111"
        };
        request.identity.as_mut().unwrap().boot_id = previous_boot.into();
        let result = observe(&request).unwrap();
        assert!(!result.alive && result.bound_and_closed);
        assert_eq!(result.boot_id, recorded.boot_id);
        request.identity = Some(Identity {
            pid: u32::MAX,
            ..recorded.clone()
        });
        let result = observe(&request).unwrap();
        assert!(!result.alive && result.bound_and_closed);
        assert_eq!(result.boot_id, recorded.boot_id);
        assert_eq!(identity(&recorded).unwrap(), Some(recorded.clone()));
        request.identity = None;
        let result = observe(&request).unwrap();
        assert!(!result.alive && result.bound_and_closed && result.identity.is_none());
        request.identity = Some(recorded.clone());
        let original_sha256 = request.sha256.clone();
        request.identity.as_mut().unwrap().started = "0".into();
        assert!(observe(&request).is_err());
        request.uid ^= 1;
        assert!(observe(&request).is_err());
        request.uid ^= 1;
        request.sha256 = "0".repeat(64);
        assert!(observe(&request).is_err());
        request.sha256 = original_sha256;
        request.source_sha256 = "0".repeat(64);
        assert!(observe(&request).is_err());
        request.source_sha256 = manifest::sha256(&request.sources).unwrap();
        request.identity = Some(recorded);
        let failure = observe_with(&request, || {
            fs::write(file.path(), b"changed")?;
            Ok(())
        })
        .unwrap_err();
        assert!(failure.to_string().contains("冻结文件"));
    }
}
