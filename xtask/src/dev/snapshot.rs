use super::{inputs, model::Binaries, runtime_config::RuntimeConfig};
use crate::{
    Result,
    cli::DevOptions,
    watch::{SourceRevision, WatchInputRoots},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

#[path = "snapshot/recovery.rs"]
mod recovery;
#[path = "snapshot/storage.rs"]
pub(super) mod storage;
use storage::{
    binary_name, copy_tree, directory_hash, file_hash, io_error, nonce, plain_directory,
    valid_generation_name, valid_session_name,
};

const SNAPSHOT_FORMAT_VERSION: u32 = 3;
const BINARY_ROOT: &str = "target/xtask/dev";
const RUNTIME_ROOT: &str = ".local-tests/dev-runtime";

#[derive(Debug, Clone)]
pub(crate) struct DevSession {
    id: String,
    generation_parent: PathBuf,
    runtime_parent: PathBuf,
    root: PathBuf,
    explicit_config: Option<PathBuf>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GenerationManifest {
    format_version: u32,
    session: String,
    generation: String,
    source_revision: u64,
    created_unix_nanos: String,
    api_sha256: String,
    worker_sha256: String,
    runtime_sha256: String,
}

#[derive(Debug)]
pub(crate) struct RuntimeSnapshot {
    pub(crate) directory: PathBuf,
    pub(super) config: Arc<RuntimeConfig>,
}

impl DevSession {
    pub(crate) fn prepare(root: &Path, options: &DevOptions) -> Result<(Self, Option<Binaries>)> {
        Self::prepare_roots(
            root,
            &root.join(BINARY_ROOT),
            &root.join(RUNTIME_ROOT),
            options,
            true,
        )
    }

    pub(crate) fn prepare_isolated(
        root: &Path,
        storage: &Path,
        options: &DevOptions,
    ) -> Result<Self> {
        Self::prepare_roots(
            root,
            &storage.join("binaries"),
            &storage.join("runtime"),
            options,
            false,
        )
        .map(|(session, _)| session)
    }

    fn prepare_roots(
        root: &Path,
        binary: &Path,
        runtime: &Path,
        options: &DevOptions,
        recover: bool,
    ) -> Result<(Self, Option<Binaries>)> {
        storage::create_plain_directory(binary)?;
        storage::create_plain_directory(runtime)?;
        storage::require_plain_tree(binary)?;
        storage::require_plain_tree(runtime)?;
        let recovered = if recover {
            recovery::latest(binary, runtime)?
        } else {
            None
        };
        let id = format!("s-{}-{}", std::process::id(), nonce()?);
        let generation_parent = binary.join(&id);
        let runtime_parent = runtime.join(&id);
        fs::create_dir(&generation_parent)
            .map_err(|error| io_error("创建二进制 session", &generation_parent, error))?;
        fs::create_dir(&runtime_parent)
            .map_err(|error| io_error("创建私有运行 session", &runtime_parent, error))?;
        Ok((
            Self {
                id,
                generation_parent,
                runtime_parent,
                root: root.to_owned(),
                explicit_config: options.config_dir.clone(),
            },
            recovered,
        ))
    }

    pub(super) fn capture(&self) -> Result<(RuntimeConfig, WatchInputRoots, Vec<String>)> {
        let (source, targets) = inputs::selected(&self.root, self.explicit_config.as_deref())?;
        let config = RuntimeConfig::capture(&source)?;
        let roots = inputs::watch_roots(&self.root, &config, source);
        Ok((config, roots, targets))
    }

    pub(crate) fn snapshot_inputs(
        &self,
        revision: SourceRevision,
        config: RuntimeConfig,
        locales: &Path,
        previous: Option<&Binaries>,
    ) -> Result<RuntimeSnapshot> {
        let generation = format!("g-{}-{}", revision.value(), nonce()?);
        let directory = self.runtime_parent.join(&generation);
        let staged = self.runtime_parent.join(format!(".{generation}.copying"));
        fs::create_dir(&staged).map_err(|error| io_error("创建候选暂存目录", &staged, error))?;
        // 本次调用只拥有刚创建的暂存目录；失败保留现场，不清理任何既有代次。
        storage::require_plain_tree(locales)?;
        let before = directory_hash(locales)?;
        copy_tree(locales, &staged.join("locales"))?;
        if before != directory_hash(locales)? || before != directory_hash(&staged.join("locales"))?
        {
            return Err("捕获 locales 时文件发生漂移；请保存完成后重试，暂存现场已保留".into());
        }
        inputs::write_roles(&config, &staged, &directory, previous)?;
        fs::rename(&staged, &directory)
            .map_err(|error| io_error("发布已捕获私有运行输入", &directory, error))?;
        Ok(RuntimeSnapshot {
            directory,
            config: Arc::new(config),
        })
    }

    pub(crate) fn install_generation(
        &self,
        revision: SourceRevision,
        api: &Path,
        worker: &Path,
        runtime: RuntimeSnapshot,
    ) -> Result<Binaries> {
        let generation = runtime
            .directory
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or("运行代次目录名无效")?;
        let created = generation.rsplit_once('-').ok_or("运行代次时间无效")?.1;
        let target = self.generation_parent.join(generation);
        let staged = self
            .generation_parent
            .join(format!(".{generation}.copying"));
        fs::create_dir(&staged).map_err(|error| io_error("创建候选暂存目录", &staged, error))?;
        fs::create_dir(staged.join("bin"))
            .map_err(|error| io_error("创建候选二进制目录", &staged.join("bin"), error))?;
        for (source, name) in [(api, "ryframe"), (worker, "ryframe-worker")] {
            storage::require_plain_file(source)?;
            let destination = staged.join("bin").join(binary_name(name));
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&destination)
                .map_err(|error| io_error("创建候选产物", &destination, error))?;
            let mut input = fs::File::open(source)
                .map_err(|error| io_error("打开候选来源产物", source, error))?;
            std::io::copy(&mut input, &mut output)
                .map_err(|error| io_error("复制候选二进制", &destination, error))?;
            fs::set_permissions(&destination, fs::metadata(source)?.permissions())
                .map_err(|error| io_error("保存候选权限", &destination, error))?;
        }
        let manifest = GenerationManifest {
            format_version: SNAPSHOT_FORMAT_VERSION,
            session: self.id.clone(),
            generation: generation.into(),
            source_revision: revision.value(),
            created_unix_nanos: created.into(),
            api_sha256: file_hash(&staged.join("bin").join(binary_name("ryframe")))?,
            worker_sha256: file_hash(&staged.join("bin").join(binary_name("ryframe-worker")))?,
            runtime_sha256: directory_hash(&runtime.directory)?,
        };
        use std::io::Write;
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(staged.join("manifest.json"))
            .map_err(|error| io_error("创建公开 manifest", &staged.join("manifest.json"), error))?
            .write_all(&serde_json::to_vec_pretty(&manifest)?)
            .map_err(|error| io_error("写入公开 manifest", &staged.join("manifest.json"), error))?;
        fs::rename(&staged, &target)
            .map_err(|error| io_error("发布候选二进制及 manifest", &target, error))?;
        Ok(binaries(
            revision,
            target,
            runtime.directory,
            runtime.config,
        ))
    }
}

fn binaries(
    revision: SourceRevision,
    generation_dir: PathBuf,
    runtime_dir: PathBuf,
    config: Arc<RuntimeConfig>,
) -> Binaries {
    Binaries {
        source_revision: revision,
        api: generation_dir.join("bin").join(binary_name("ryframe")),
        worker: generation_dir
            .join("bin")
            .join(binary_name("ryframe-worker")),
        config_dir: runtime_dir.join("source-config"),
        locales_dir: runtime_dir.join("locales"),
        generation_dir,
        runtime_dir,
        runtime_config: config,
    }
}

pub(crate) fn recovery_seed(recovered: Option<&Binaries>) -> Result<SourceRevision> {
    recovered.map_or(Ok(SourceRevision::from_value(0)), |value| {
        value
            .source_revision
            .value()
            .checked_add(1)
            .map(SourceRevision::from_value)
            .ok_or_else(|| "历史 source_revision 已耗尽；拒绝启动监督器，请检查代次来源".into())
    })
}

pub(crate) fn cleanup_binaries(bundle: &Binaries) -> Result<()> {
    remove_owned(&bundle.generation_dir)?;
    remove_owned(&bundle.runtime_dir)
}

pub(super) fn cleanup_runtime(runtime: &RuntimeSnapshot) -> Result<()> {
    remove_owned(&runtime.directory)
}

pub(super) fn verify_binaries(bundle: &Binaries) -> Result<()> {
    recovery::verify(bundle)
}

fn remove_owned(path: &Path) -> Result<()> {
    let session = path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .ok_or("清理路径缺少 session")?;
    let generation = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("清理路径缺少 generation")?;
    if !valid_session_name(session)
        || !valid_generation_name(generation)
        || !session.starts_with(&format!("s-{}-", std::process::id()))
    {
        return Err(format!(
            "拒绝清理不属于当前进程的候选 {}；请保留现场核对所有权",
            path.display()
        )
        .into());
    }
    if !path.exists() {
        return Ok(());
    }
    if !plain_directory(path) {
        return Err("候选清理路径不是普通目录；请保留现场".into());
    }
    storage::require_plain_tree(path)?;
    let resolved = path.canonicalize()?;
    let parent = path.parent().ok_or("候选路径缺少父目录")?.canonicalize()?;
    if resolved.parent() != Some(parent.as_path()) {
        return Err("候选清理越过所属 session；请保留现场".into());
    }
    fs::remove_dir_all(path).map_err(|error| io_error("清理本进程已停止候选", path, error))?;
    Ok(())
}
