use super::super::{
    inputs, model::Binaries, model::ServiceLaunchMode, runtime_config::RuntimeConfig,
};
use super::{GenerationManifest, SNAPSHOT_FORMAT_VERSION, binaries, storage::*};
use crate::{Result, watch::SourceRevision};
use std::{fs, path::Path, sync::Arc};

pub(super) fn latest(binary: &Path, runtime: &Path) -> Result<Option<Binaries>> {
    let mut complete = Vec::new();
    for (session, session_dir) in named_directories(binary)? {
        if !valid_session_name(&session) {
            continue;
        }
        for (generation, directory) in named_directories(&session_dir)? {
            if !valid_generation_name(&generation) {
                continue;
            }
            if let Some((created, bundle)) = load(runtime, &session, &generation, &directory)? {
                complete.push((created, session.clone(), generation, bundle));
            }
        }
    }
    // 不删除扫描到的残缺或损坏现场，也不以当前进程配置替换历史输入。
    complete
        .sort_by(|left, right| (&left.0, &left.1, &left.2).cmp(&(&right.0, &right.1, &right.2)));
    Ok(complete.pop().map(|(_, _, _, bundle)| bundle))
}

pub(super) fn verify(bundle: &Binaries) -> Result<()> {
    if bundle.config_dir != bundle.runtime_dir.join("source-config")
        || bundle.locales_dir != bundle.runtime_dir.join("locales")
    {
        return Err("恢复 LKG 路径绑定不一致；请保留现场并选择完整可信代次".into());
    }
    let session = bundle
        .generation_dir
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str())
        .ok_or("LKG 缺少 session；请保留现场")?;
    let generation = bundle
        .generation_dir
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or("LKG 缺少 generation；请保留现场")?;
    let runtime_root = bundle
        .runtime_dir
        .parent()
        .and_then(Path::parent)
        .ok_or("LKG 缺少私有运行根；请保留现场")?;
    let verified = load(runtime_root, session, generation, &bundle.generation_dir)?;
    if !verified.is_some_and(|(_, saved)| {
        saved.runtime_dir == bundle.runtime_dir && saved.source_revision == bundle.source_revision
    }) {
        return Err("恢复 LKG 完整性校验失败：二进制或私有运行输入缺失、篡改或来源不符；请保留现场并选择可信快照，拒绝启动 Worker".into());
    }
    Ok(())
}

fn load(
    runtime_root: &Path,
    session: &str,
    generation: &str,
    directory: &Path,
) -> Result<Option<(u128, Binaries)>> {
    let manifest_path = directory.join("manifest.json");
    if !plain_file(&manifest_path) {
        return Ok(None);
    }
    let manifest: GenerationManifest = serde_json::from_slice(&fs::read(&manifest_path)?).map_err(|error|format!("恢复 manifest {} 失败：字段或 JSON 无效（line={}, column={}）；拒绝旧格式或未知字段，请保留现场并显式选择可信代次",manifest_path.display(),error.line(),error.column()))?;
    if manifest.format_version != SNAPSHOT_FORMAT_VERSION {
        return Err(format!(
            "恢复 manifest {} 失败：仅接受 format_version=3；不自动转换旧快照，请保留现场",
            manifest_path.display()
        )
        .into());
    }
    let runtime = runtime_root.join(session).join(generation);
    let api = directory.join("bin").join(binary_name("ryframe"));
    let worker = directory.join("bin").join(binary_name("ryframe-worker"));
    if !plain_directory(&runtime) || !plain_file(&api) || !plain_file(&worker) {
        return Ok(None);
    }
    require_plain_tree(directory)?;
    require_plain_tree(&runtime)?;
    let Some(created) = manifest.created_unix_nanos.parse::<u128>().ok() else {
        return Ok(None);
    };
    if manifest.session != session
        || manifest.generation != generation
        || generation
            != format!(
                "g-{}-{}",
                manifest.source_revision, manifest.created_unix_nanos
            )
        || file_hash(&api)? != manifest.api_sha256
        || file_hash(&worker)? != manifest.worker_sha256
        || directory_hash(&runtime)? != manifest.runtime_sha256
    {
        return Ok(None);
    }
    let expected = [
        "source-config",
        "locales",
        "api",
        "worker",
        "probe-api",
        "probe-worker",
        "migrate",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<std::collections::BTreeSet<_>>();
    let actual = fs::read_dir(&runtime)?
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<std::io::Result<std::collections::BTreeSet<_>>>()?;
    if actual != expected {
        return Ok(None);
    }
    let config = RuntimeConfig::capture(&runtime.join("source-config"))?;
    for role in ["api", "worker", "probe-api", "probe-worker", "migrate"] {
        let app = inputs::load_role(&runtime.join(role))?;
        if app.app.locales_dir != runtime.join("locales") {
            return Ok(None);
        }
    }
    inputs::load_ports(&runtime, ServiceLaunchMode::Development)?;
    inputs::load_ports(&runtime, ServiceLaunchMode::Probe)?;
    Ok(Some((
        created,
        binaries(
            SourceRevision::from_value(manifest.source_revision),
            directory.to_owned(),
            runtime,
            Arc::new(config),
        ),
    )))
}
