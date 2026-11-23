//! 正式参考恢复写入阶段持有原维护来源，普通来源核验继续执行完整 Git 观察。
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::Result;

#[cfg(windows)]
use super::{
    LocalTestPathKind, assets, held_git::HeldGit, require_root, root_window, target_root,
    target_source_binding, validate_absolute_path, validate_pair, window,
};
#[cfg(windows)]
use crate::recovery::clone::native::{source, target_verify::guard::HeldSourceTree};

pub(in crate::recovery) struct HeldTargetSource {
    #[cfg(windows)]
    coordinator: PathBuf,
    #[cfg(windows)]
    declaration: Value,
    #[cfg(windows)]
    root: PathBuf,
    #[cfg(windows)]
    binding: Value,
    #[cfg(windows)]
    window: super::Window,
    #[cfg(windows)]
    roots: Vec<HeldSourceRoot>,
    #[cfg(not(windows))]
    impossible: std::convert::Infallible,
}

#[cfg(windows)]
pub(in crate::recovery) struct HeldSourceRoot {
    worktree: HeldSourceTree,
    git: HeldGit,
}

impl HeldTargetSource {
    #[cfg(windows)]
    pub(in crate::recovery) fn open(
        coordinator: &Path,
        declared: &Value,
        expected_binding: &Value,
    ) -> Result<Self> {
        let root = target_root(coordinator, declared)?;
        let window = root_window(&root, &declared["execution_source"])?;
        let publication = window.publication();
        let binding = target_source_binding(&root, declared, publication, |_, _| Ok(()))?;
        if &binding != expected_binding {
            return Err("待保护维护来源与已核验的恢复目标不一致".into());
        }
        let mut roots = Vec::with_capacity(4);
        for name in ["backend", "frontend"] {
            roots.push(HeldSourceRoot::open(
                &publication["original"]["paths"][name],
                &publication["original"]["current"][name],
                (name == "frontend").then_some(&publication["original"]["contract_assets"]),
                name,
            )?);
            roots.push(HeldSourceRoot::open(
                &publication["published"][name]["path"],
                &publication["published"][name]["inventory"],
                None,
                name,
            )?);
        }
        let protected = Self {
            coordinator: coordinator.to_path_buf(),
            declaration: declared.clone(),
            root,
            binding,
            window,
            roots,
        };
        // 只有四根和 Git 元数据已受保护之后的原完整观察才能建立复用依据。
        protected.check_end()?;
        Ok(protected)
    }

    pub(in crate::recovery) fn verify(
        &self,
        coordinator: &Path,
        declared: &Value,
    ) -> Result<(PathBuf, Value)> {
        #[cfg(windows)]
        {
            if coordinator != self.coordinator || declared != &self.declaration {
                return Err("恢复检查点请求的维护来源与已保护来源不同".into());
            }
            self.checkpoint()?;
            Ok((self.root.clone(), self.binding.clone()))
        }
        #[cfg(not(windows))]
        {
            let _ = (coordinator, declared);
            match self.impossible {}
        }
    }

    /// 在同一受保护来源中观察精确组合，返回结果前再次核对四根和登记文件。
    pub(in crate::recovery) fn with_pair<T>(
        &self,
        root: &Path,
        pair: &Value,
        observe: impl FnOnce(&Value) -> Result<T>,
    ) -> Result<T> {
        #[cfg(windows)]
        {
            validate_pair(pair)?;
            validate_absolute_path(root, LocalTestPathKind::ExistingDirectory)?;
            let publication = self.window.publication();
            require_root(root, publication)?;
            if pair["execution_source"] != self.declaration["execution_source"] {
                return Err("执行来源组合的清单绑定与已保护来源不同".into());
            }
            if pair["sources"] != publication["original"]["source_pair"]["sources"] {
                return Err("执行来源组合的原始来源不匹配".into());
            }
            self.checkpoint()?;
            let result = observe(publication);
            let after = self.checkpoint();
            match result {
                Ok(value) => after.map(|()| value),
                Err(error) => Err(window::failure(error, after)),
            }
        }
        #[cfg(not(windows))]
        {
            let _ = (root, pair, observe);
            match self.impossible {}
        }
    }

    pub(in crate::recovery) fn check_end(&self) -> Result<()> {
        #[cfg(windows)]
        {
            let before = self.checkpoint();
            let completed = self.window.verify();
            super::window::finish([before, completed, self.checkpoint()])
        }
        #[cfg(not(windows))]
        match self.impossible {}
    }

    #[cfg(windows)]
    fn checkpoint(&self) -> Result<()> {
        if self
            .root
            .parent()
            .ok_or("执行源码根缺少父目录")?
            .join("fixture.json")
            .exists()
        {
            return Err("独立执行源码的父目录不能含 fixture.json".into());
        }
        self.window.check_documents()?;
        // 四个根的句柄只读且各自独立；以现有有界线程能力并行观测。
        crate::source_scan::ordered(&self.roots, true, HeldSourceRoot::checkpoint)?;
        self.window.check_documents()
    }
}

#[cfg(windows)]
impl HeldSourceRoot {
    fn open(path: &Value, inventory: &Value, asset: Option<&Value>, role: &str) -> Result<Self> {
        let root = Path::new(path.as_str().ok_or("维护来源清单缺少源码根")?);
        let inventory = source::validate_inventory(inventory)?;
        let files = match asset {
            Some(asset) => assets::expected(&inventory.files, asset)?,
            None => inventory.files,
        };
        Self::open_tree(root, &inventory.source.snapshot.head, &files, role)
    }

    pub(in crate::recovery) fn open_backend(
        root: &Path,
        inventory: &source::Inventory,
    ) -> Result<Self> {
        Self::open_tree(
            root,
            &inventory.source.snapshot.head,
            &inventory.files,
            "backend",
        )
    }

    fn open_tree(
        root: &Path,
        head: &str,
        files: &[source::FileRecord],
        role: &str,
    ) -> Result<Self> {
        let git = HeldGit::open(root)?;
        if git.head() != head {
            return Err("受保护 Git HEAD 与登记的维护来源不同".into());
        }
        let mut files: Vec<_> = files.iter().map(|item| root.join(&item.path)).collect();
        let git_entry = root.join(".git");
        let git_type = git_entry.symlink_metadata()?.file_type();
        let mut skipped = vec![".local-tests"];
        if role == "frontend" {
            skipped.extend(["node_modules", "dist"]);
        }
        if git_type.is_dir() {
            skipped.push(".git");
        } else {
            files.push(git_entry);
        }
        let worktree = HeldSourceTree::open(root, files, &skipped)?;
        Ok(Self { worktree, git })
    }

    pub(in crate::recovery) fn checkpoint(&self) -> Result<()> {
        self.worktree.checkpoint()?;
        self.git.checkpoint()
    }
}
