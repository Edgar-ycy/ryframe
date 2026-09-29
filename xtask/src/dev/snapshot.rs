use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::{Deserialize, Serialize};

use crate::{Result, watch::SourceRevision};

use super::{
    model::Binaries,
    runtime_secrets::{RuntimeSecrets, snapshot_config_tree},
};

#[path = "snapshot/storage.rs"]
mod storage;
use storage::{
    binary_name, copy_tree, copying_generation_name, directory_hash, file_hash, named_directories,
    nonce, plain_directory, plain_file, session_is_active, valid_generation_name,
    valid_session_name,
};

const SNAPSHOT_FORMAT_VERSION: u32 = 2;
const BINARY_ROOT: &str = "target/xtask/dev";
const RUNTIME_ROOT: &str = ".local-tests/dev-runtime";

#[derive(Debug, Clone)]
pub(crate) struct DevSession {
    id: String,
    generation_parent: PathBuf,
    runtime_parent: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
struct GenerationManifest {
    format_version: u32,
    session: String,
    generation: String,
    source_revision: u64,
    created_unix_nanos: String,
    api_sha256: String,
    worker_sha256: String,
    config_sha256: String,
    locales_sha256: String,
    secret_environment: Vec<String>,
}

struct GenerationPaths {
    generation_staged: PathBuf,
    generation_dir: PathBuf,
    runtime_staged: PathBuf,
    runtime_dir: PathBuf,
}

struct RecoveredGeneration {
    created_unix_nanos: u128,
    session: String,
    generation: String,
    binaries: Binaries,
}

impl DevSession {
    pub(crate) fn prepare(
        root: &Path,
        source_revision: SourceRevision,
    ) -> Result<(Self, Option<Binaries>)> {
        let binary_root = root.join(BINARY_ROOT);
        let runtime_root = root.join(RUNTIME_ROOT);
        let runtime_secrets = RuntimeSecrets::capture(&root.join("config"))?;
        Self::prepare_with_roots(
            &binary_root,
            &runtime_root,
            source_revision,
            true,
            Some(&runtime_secrets),
        )
    }

    pub(crate) fn prepare_isolated(
        storage_root: &Path,
        source_revision: SourceRevision,
    ) -> Result<Self> {
        let binary_root = storage_root.join("binaries");
        let runtime_root = storage_root.join("runtime");
        Self::prepare_with_roots(&binary_root, &runtime_root, source_revision, false, None)
            .map(|(session, _)| session)
    }

    #[allow(dead_code)]
    pub(crate) fn prepare_with_runtime_secrets(
        root: &Path,
        source_revision: SourceRevision,
        runtime_secrets: &RuntimeSecrets,
    ) -> Result<(Self, Option<Binaries>)> {
        Self::prepare_with_roots(
            &root.join(BINARY_ROOT),
            &root.join(RUNTIME_ROOT),
            source_revision,
            true,
            Some(runtime_secrets),
        )
    }

    fn prepare_with_roots(
        binary_root: &Path,
        runtime_root: &Path,
        source_revision: SourceRevision,
        recover: bool,
        runtime_secrets: Option<&RuntimeSecrets>,
    ) -> Result<(Self, Option<Binaries>)> {
        fs::create_dir_all(binary_root)?;
        fs::create_dir_all(runtime_root)?;
        cleanup_copying_directories(binary_root)?;
        cleanup_copying_directories(runtime_root)?;
        if !recover {
            cleanup_inactive_sessions(binary_root)?;
            cleanup_inactive_sessions(runtime_root)?;
        }
        let recovered = if recover {
            recover_latest(
                binary_root,
                runtime_root,
                source_revision,
                runtime_secrets.ok_or("恢复 LKG 缺少当前进程密钥环境")?,
            )?
        } else {
            None
        };
        let id = format!("s-{}-{}", std::process::id(), nonce()?);
        let generation_parent = binary_root.join(&id);
        let runtime_parent = runtime_root.join(&id);
        fs::create_dir(&generation_parent)?;
        fs::create_dir(&runtime_parent)?;
        Ok((
            Self {
                id,
                generation_parent,
                runtime_parent,
            },
            recovered,
        ))
    }

    pub(crate) fn install_generation(
        &self,
        source_revision: SourceRevision,
        api_source: &Path,
        worker_source: &Path,
        config_source: &Path,
        locales_source: &Path,
    ) -> Result<Binaries> {
        let created_unix_nanos = nonce()?;
        let generation = format!("g-{}-{created_unix_nanos}", source_revision.value());
        let paths = self.generation_paths(&generation);
        let result = (|| {
            fs::create_dir(&paths.generation_staged)?;
            fs::create_dir(paths.generation_staged.join("bin"))?;
            fs::create_dir(&paths.runtime_staged)?;
            let runtime_secrets = self.write_and_commit_generation(
                &generation,
                created_unix_nanos,
                source_revision,
                api_source,
                worker_source,
                config_source,
                locales_source,
                &paths,
            )?;
            binaries_from_paths(source_revision, &paths, runtime_secrets)
        })();
        match result {
            Ok(binaries) => Ok(binaries),
            Err(error) => {
                let _ = remove_direct_child(
                    &self.generation_parent,
                    &paths.generation_staged,
                    "二进制暂存版本",
                );
                let _ = remove_direct_child(
                    &self.runtime_parent,
                    &paths.runtime_staged,
                    "运行输入暂存版本",
                );
                Err(error)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn write_and_commit_generation(
        &self,
        generation: &str,
        created_unix_nanos: u128,
        source_revision: SourceRevision,
        api_source: &Path,
        worker_source: &Path,
        config_source: &Path,
        locales_source: &Path,
        paths: &GenerationPaths,
    ) -> Result<RuntimeSecrets> {
        let runtime_secrets =
            snapshot_config_tree(config_source, &paths.runtime_staged.join("config"))?;
        copy_tree(locales_source, &paths.runtime_staged.join("locales"))?;
        fs::copy(
            api_source,
            paths
                .generation_staged
                .join("bin")
                .join(binary_name("ryframe")),
        )?;
        fs::copy(
            worker_source,
            paths
                .generation_staged
                .join("bin")
                .join(binary_name("ryframe-worker")),
        )?;
        let manifest = GenerationManifest {
            format_version: SNAPSHOT_FORMAT_VERSION,
            session: self.id.clone(),
            generation: generation.to_owned(),
            source_revision: source_revision.value(),
            created_unix_nanos: created_unix_nanos.to_string(),
            api_sha256: file_hash(
                &paths
                    .generation_staged
                    .join("bin")
                    .join(binary_name("ryframe")),
            )?,
            worker_sha256: file_hash(
                &paths
                    .generation_staged
                    .join("bin")
                    .join(binary_name("ryframe-worker")),
            )?,
            config_sha256: directory_hash(&paths.runtime_staged.join("config"))?,
            locales_sha256: directory_hash(&paths.runtime_staged.join("locales"))?,
            secret_environment: runtime_secrets.environment_names(),
        };
        fs::write(
            paths.generation_staged.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest)?,
        )?;
        // 运行输入先落位，二进制目录作为提交标记；两次 rename 之间崩溃时，启动扫描会
        // 删除没有对应提交标记的孤立运行输入。
        fs::rename(&paths.runtime_staged, &paths.runtime_dir)?;
        if let Err(error) = fs::rename(&paths.generation_staged, &paths.generation_dir) {
            let _ = remove_direct_child(&self.runtime_parent, &paths.runtime_dir, "运行输入版本");
            return Err(error.into());
        }
        Ok(runtime_secrets)
    }

    fn generation_paths(&self, generation: &str) -> GenerationPaths {
        GenerationPaths {
            generation_staged: self
                .generation_parent
                .join(format!(".{generation}.copying")),
            generation_dir: self.generation_parent.join(generation),
            runtime_staged: self.runtime_parent.join(format!(".{generation}.copying")),
            runtime_dir: self.runtime_parent.join(generation),
        }
    }
}

fn recover_latest(
    binary_root: &Path,
    runtime_root: &Path,
    source_revision: SourceRevision,
    runtime_secrets: &RuntimeSecrets,
) -> Result<Option<Binaries>> {
    let mut complete = BTreeSet::new();
    let mut recovered = Vec::new();
    for (session, session_dir) in named_directories(binary_root)? {
        if !valid_session_name(&session) {
            continue;
        }
        let active = session_is_active(&session);
        for (generation, generation_dir) in named_directories(&session_dir)? {
            if !valid_generation_name(&generation) {
                continue;
            }
            match load_generation(
                runtime_root,
                source_revision,
                &session,
                &generation,
                &generation_dir,
                runtime_secrets,
            )? {
                Some(candidate) => {
                    complete.insert((session.clone(), generation.clone()));
                    recovered.push(candidate);
                }
                None if !active => {
                    remove_generation_pair(binary_root, runtime_root, &session, &generation)?;
                }
                None => {}
            }
        }
    }
    cleanup_orphan_runtime_generations(runtime_root, &complete)?;
    recovered.sort_by(|left, right| {
        (
            left.created_unix_nanos,
            left.session.as_str(),
            left.generation.as_str(),
        )
            .cmp(&(
                right.created_unix_nanos,
                right.session.as_str(),
                right.generation.as_str(),
            ))
    });
    Ok(recovered.pop().map(|generation| generation.binaries))
}

fn load_generation(
    runtime_root: &Path,
    source_revision: SourceRevision,
    session: &str,
    generation: &str,
    generation_dir: &Path,
    runtime_secrets: &RuntimeSecrets,
) -> Result<Option<RecoveredGeneration>> {
    let runtime_dir = runtime_root.join(session).join(generation);
    let manifest_path = generation_dir.join("manifest.json");
    if !plain_file(&manifest_path)
        || !plain_file(&generation_dir.join("bin").join(binary_name("ryframe")))
        || !plain_file(
            &generation_dir
                .join("bin")
                .join(binary_name("ryframe-worker")),
        )
        || !plain_directory(&runtime_dir.join("config"))
        || !plain_directory(&runtime_dir.join("locales"))
    {
        return Ok(None);
    }
    let Ok(manifest) = serde_json::from_slice::<GenerationManifest>(&fs::read(manifest_path)?)
    else {
        return Ok(None);
    };
    let Ok(created_unix_nanos) = manifest.created_unix_nanos.parse::<u128>() else {
        return Ok(None);
    };
    if manifest.format_version != SNAPSHOT_FORMAT_VERSION
        || manifest.session != session
        || manifest.generation != generation
        || manifest.generation
            != format!(
                "g-{}-{}",
                manifest.source_revision, manifest.created_unix_nanos
            )
        || file_hash(&generation_dir.join("bin").join(binary_name("ryframe")))?
            != manifest.api_sha256
        || file_hash(
            &generation_dir
                .join("bin")
                .join(binary_name("ryframe-worker")),
        )? != manifest.worker_sha256
        || directory_hash(&runtime_dir.join("config"))? != manifest.config_sha256
        || directory_hash(&runtime_dir.join("locales"))? != manifest.locales_sha256
    {
        return Ok(None);
    }
    runtime_secrets.require_environment(&manifest.secret_environment)?;
    let paths = GenerationPaths {
        generation_staged: PathBuf::new(),
        generation_dir: generation_dir.to_path_buf(),
        runtime_staged: PathBuf::new(),
        runtime_dir,
    };
    Ok(Some(RecoveredGeneration {
        created_unix_nanos,
        session: session.to_owned(),
        generation: generation.to_owned(),
        binaries: binaries_from_paths(source_revision, &paths, runtime_secrets.clone())?,
    }))
}

fn cleanup_copying_directories(root: &Path) -> Result<()> {
    for (name, path) in named_directories(root)? {
        if copying_generation_name(&name).is_some() {
            remove_direct_child(root, &path, "遗留暂存版本")?;
            continue;
        }
        if !valid_session_name(&name) || session_is_active(&name) {
            continue;
        }
        for (child_name, child_path) in named_directories(&path)? {
            if copying_generation_name(&child_name).is_some() {
                remove_direct_child(&path, &child_path, "遗留暂存版本")?;
            }
        }
        remove_empty_directory(&path)?;
    }
    Ok(())
}

fn cleanup_inactive_sessions(root: &Path) -> Result<()> {
    for (name, path) in named_directories(root)? {
        if valid_session_name(&name) && !session_is_active(&name) {
            remove_direct_child(root, &path, "遗留隔离测量 session")?;
        }
    }
    Ok(())
}

fn cleanup_orphan_runtime_generations(
    runtime_root: &Path,
    complete: &BTreeSet<(String, String)>,
) -> Result<()> {
    for (session, session_dir) in named_directories(runtime_root)? {
        if !valid_session_name(&session) || session_is_active(&session) {
            continue;
        }
        for (generation, generation_dir) in named_directories(&session_dir)? {
            if valid_generation_name(&generation)
                && !complete.contains(&(session.clone(), generation.clone()))
            {
                remove_direct_child(&session_dir, &generation_dir, "孤立运行输入版本")?;
            }
        }
        remove_empty_directory(&session_dir)?;
    }
    Ok(())
}

pub(crate) fn cleanup_binaries(binaries: &Binaries) -> Result<()> {
    let binary_root = generation_root(&binaries.generation_dir, "二进制版本")?;
    let runtime_root = generation_root(&binaries.runtime_dir, "运行输入版本")?;
    remove_generation_dir(&binary_root, &binaries.generation_dir, "二进制版本")?;
    remove_generation_dir(&runtime_root, &binaries.runtime_dir, "运行输入版本")
}

fn remove_generation_pair(
    binary_root: &Path,
    runtime_root: &Path,
    session: &str,
    generation: &str,
) -> Result<()> {
    remove_generation_dir(
        binary_root,
        &binary_root.join(session).join(generation),
        "残缺二进制版本",
    )?;
    remove_generation_dir(
        runtime_root,
        &runtime_root.join(session).join(generation),
        "残缺运行输入版本",
    )
}

fn remove_generation_dir(allowed_root: &Path, generation: &Path, label: &str) -> Result<()> {
    let Some(session) = generation.parent() else {
        return Err(format!("{label}目录缺少 session：{}", generation.display()).into());
    };
    let session_name = session.file_name().and_then(|name| name.to_str());
    let generation_name = generation.file_name().and_then(|name| name.to_str());
    if session.parent() != Some(allowed_root)
        || !session_name.is_some_and(valid_session_name)
        || !generation_name.is_some_and(valid_generation_name)
    {
        return Err(format!("{label}目录越过清理白名单：{}", generation.display()).into());
    }
    remove_direct_child(session, generation, label)?;
    remove_empty_directory(session)
}

fn remove_direct_child(parent: &Path, child: &Path, label: &str) -> Result<()> {
    if child.parent() != Some(parent) || child.file_name().is_none() {
        return Err(format!("{label}目录越过清理白名单：{}", child.display()).into());
    }
    let metadata = match fs::symlink_metadata(child) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.file_type().is_dir() {
        return Err(format!("拒绝清理非普通目录的{label}：{}", child.display()).into());
    }
    fs::remove_dir_all(child)
        .map_err(|error| format!("无法清理{label}目录 {}：{error}", child.display()).into())
}

fn remove_empty_directory(path: &Path) -> Result<()> {
    match fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(format!("无法清理空 session 目录 {}：{error}", path.display()).into()),
    }
}

fn binaries_from_paths(
    source_revision: SourceRevision,
    paths: &GenerationPaths,
    runtime_secrets: RuntimeSecrets,
) -> Result<Binaries> {
    Ok(Binaries {
        source_revision,
        api: paths
            .generation_dir
            .join("bin")
            .join(binary_name("ryframe")),
        worker: paths
            .generation_dir
            .join("bin")
            .join(binary_name("ryframe-worker")),
        config_dir: paths.runtime_dir.join("config"),
        locales_dir: paths.runtime_dir.join("locales"),
        generation_dir: paths.generation_dir.clone(),
        runtime_dir: paths.runtime_dir.clone(),
        runtime_secrets: Arc::new(runtime_secrets),
    })
}

fn generation_root(generation: &Path, label: &str) -> Result<PathBuf> {
    generation
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| format!("{label}目录缺少 storage root：{}", generation.display()).into())
}
