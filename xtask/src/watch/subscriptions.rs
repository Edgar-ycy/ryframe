use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use super::{
    SourceRevisionTracker,
    dispatch::{Ingress, notify_error},
    inputs::{Mapping, ordinary_directory},
};
use crate::Result;

pub(super) struct Subscriptions {
    // 每代整体拥有一个原生实例；销毁旧代不会操作新代的订阅。
    _watcher: RecommendedWatcher,
    health: Arc<AtomicBool>,
    ingress: Ingress,
    root: PathBuf,
    tools_parent_present: bool,
    tools_present: bool,
    #[cfg(test)]
    registrations: BTreeMap<PathBuf, RecursiveMode>,
}

impl Subscriptions {
    pub(super) fn new(root: &Path, mapping: &Mapping, ingress: Ingress) -> Result<Self> {
        Self::build(root, mapping, ingress, |_, _, _| Ok(()))
    }

    fn build(
        root: &Path,
        mapping: &Mapping,
        ingress: Ingress,
        mut after_register: impl FnMut(&Path, &Arc<AtomicBool>, &Ingress) -> Result<()>,
    ) -> Result<Self> {
        let registrations = plan(mapping)?;
        let health = Arc::new(AtomicBool::new(false));
        let callback_health = Arc::clone(&health);
        let callback_ingress = ingress.clone();
        let mut watcher = notify::recommended_watcher(move |event| {
            callback_ingress.send(&callback_health, event)
        })
        .map_err(|error| {
            format!(
                "创建新原生订阅实例 {} 失败：{}",
                root.display(),
                notify_error("创建后端", &error)
            )
        })?;
        for (path, mode) in &registrations {
            watcher.watch(path, *mode).map_err(|error| {
                format!(
                    "准备新订阅实例：登记 {} 为 {mode:?} 失败；未发布新实例，保留旧实例；{}",
                    path.display(),
                    notify_error("登记原生目录", &error),
                )
            })?;
            after_register(path, &health, &ingress)?;
        }
        let result = Self {
            _watcher: watcher,
            health,
            ingress,
            root: root.to_path_buf(),
            tools_parent_present: root.join(".local-tests").is_dir(),
            tools_present: root.join(".local-tests/tools.toml").exists(),
            #[cfg(test)]
            registrations,
        };
        result.ensure_healthy("新实例全部登记完成后")?;
        Ok(result)
    }

    pub(super) fn health(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.health)
    }

    pub(super) fn ensure_healthy(&self, stage: &str) -> Result<()> {
        if self.health.load(Ordering::SeqCst) {
            return Err(format!("{stage}失败：workspace {} 的原生监听实例已报告错误；拒绝复用不可信事件流，请停止监督器并检查监听路径、权限及系统资源后重新启动", self.root.display()).into());
        }
        Ok(())
    }

    pub(super) fn observe_tools_parent(
        &mut self,
        mapping: &Mapping,
        paths: &[PathBuf],
        tracker: &SourceRevisionTracker,
    ) -> Result<BTreeSet<String>> {
        let parent = self.root.join(".local-tests");
        let tools = parent.join("tools.toml");
        if paths.contains(&tools) {
            self.tools_present = tools.exists();
        }
        if !paths.contains(&parent) {
            return Ok(BTreeSet::new());
        }
        let present = parent.is_dir();
        if !present && !self.tools_parent_present {
            return Ok(BTreeSet::new());
        }
        // 通用源码模式不创建 .local-tests；父目录出现或原子替换时也按整代换新登记。
        self.ensure_healthy("发现本机工具父目录变化后")?;
        let prepared = Self::new(&mapping.root, mapping, self.ingress.clone());
        self.ensure_healthy("发布本机工具父目录监听前，旧实例")?;
        let prepared = prepared?;
        prepared.ensure_healthy("发布本机工具父目录监听前，新实例")?;
        let changed = self.tools_present || prepared.tools_present;
        *tracker
            .health
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = prepared.health();
        let previous = std::mem::replace(self, prepared);
        drop(previous);
        Ok(if changed {
            BTreeSet::from(["config/tools.local.toml".into()])
        } else {
            BTreeSet::new()
        })
    }

    #[cfg(test)]
    pub(super) fn fail_during_prepare(mapping: &Mapping, ingress: Ingress) -> Result<Self> {
        Self::build(&mapping.root, mapping, ingress, |path, health, ingress| {
            ingress.send(
                health,
                Err(notify::Error::io(std::io::Error::from_raw_os_error(5))
                    .add_path(path.to_path_buf())),
            );
            Err(format!(
                "测试注入新实例登记失败：路径 {}；请检查登记权限",
                path.display()
            )
            .into())
        })
    }

    #[cfg(test)]
    pub(super) fn registrations(&self) -> &BTreeMap<PathBuf, RecursiveMode> {
        &self.registrations
    }
}

fn plan(mapping: &Mapping) -> Result<BTreeMap<PathBuf, RecursiveMode>> {
    let mut recursive = BTreeSet::new();
    let mut parents = BTreeSet::from([mapping.root.clone()]);
    for relative in [
        ".cargo", "crates", "config", "catalog", "locales", "vendor", "xtask",
    ] {
        let path = mapping.root.join(relative);
        if path.is_dir() {
            recursive.insert(ordinary_directory(&path, "workspace 默认树")?);
        }
    }
    if let Some(inputs) = &mapping.inputs {
        for directory in [&inputs.config_dir, &inputs.locales_dir] {
            recursive.insert(directory.clone());
            let parent = directory.parent().ok_or_else(|| {
                format!(
                    "登记输入 {} 失败：缺少稳定父目录；请指定具有父目录的普通输入目录",
                    directory.display()
                )
            })?;
            parents.insert(ordinary_directory(parent, "输入稳定父目录")?);
        }
    }
    let tools_parent = mapping.root.join(".local-tests");
    match std::fs::symlink_metadata(&tools_parent) {
        Ok(_) => {
            parents.insert(ordinary_directory(&tools_parent, "本机 tools 父目录")?);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(
                super::inputs::io_error("检查本机 tools 父目录", &tools_parent, &error).into(),
            );
        }
    }
    Ok(compact(recursive, parents))
}

fn compact(
    recursive: BTreeSet<PathBuf>,
    parents: BTreeSet<PathBuf>,
) -> BTreeMap<PathBuf, RecursiveMode> {
    let mut recursive = recursive.into_iter().collect::<Vec<_>>();
    recursive.sort_by_key(|path| path.components().count());
    let mut result = BTreeMap::new();
    for path in recursive {
        if !result
            .keys()
            .any(|ancestor: &PathBuf| path.starts_with(ancestor))
        {
            result.insert(path, RecursiveMode::Recursive);
        }
    }
    for path in parents {
        if !result
            .iter()
            .any(|(ancestor, mode)| *mode == RecursiveMode::Recursive && path.starts_with(ancestor))
        {
            result.insert(path, RecursiveMode::NonRecursive);
        }
    }
    result
}
