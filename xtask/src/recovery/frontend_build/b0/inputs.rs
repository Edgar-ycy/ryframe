use std::{
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::{Value, json};

use super::super::{
    fail,
    files::{self, StableFile},
    model, source,
};
use super::{HEAD, TREE};
use crate::Result;

const SOURCE_FILES: [&str; 6] = [
    ".env",
    ".node-version",
    "package.json",
    "pnpm-lock.yaml",
    "pnpm-workspace.yaml",
    "vite.config.ts",
];
const DEPENDENCY_FILES: [&str; 4] = [
    "node_modules/.modules.yaml",
    "node_modules/.pnpm/lock.yaml",
    "node_modules/.pnpm-workspace-state-v1.json",
    if cfg!(windows) {
        "node_modules/.bin/vite.CMD"
    } else {
        "node_modules/.bin/vite"
    },
];
pub(super) const EMPTY_CONFIG: &str = ".local-tests/b0-frontend-no-global-config";
const ABSENT: [&str; 9] = [
    ".env.local",
    ".env.production",
    ".env.production.local",
    ".corepack.env",
    ".npmrc",
    ".pnpmfile.cjs",
    "pnpmfile.cjs",
    ".pnpmfile.mjs",
    EMPTY_CONFIG,
];
const ENV_SHA256: &str = "d24709d7deea3cc0012bb08780bba0a9f8190d47b0335c4dd8a4a32b8050ec64";
const NODE_FILE_SHA256: &str = "b911698dc3cf3227d34d0ad6727f1c4408791203c9665de99ab959331a1fb07d";

pub(super) struct Inputs {
    root: PathBuf,
    held: Vec<StableFile>,
    pub(super) value: Value,
}

impl Inputs {
    pub(super) fn observe(root: &Path, inventory: &Value) -> Result<Self> {
        reject_overrides(root)?;
        let inventory = source::validate_inventory(inventory)?;
        let mut held = Vec::new();
        let mut input_records = Vec::new();
        let mut dependency_records = Vec::new();
        for name in SOURCE_FILES {
            let mut file = stable(&root.join(name))?;
            compare_source(&inventory, name, &file)?;
            match name {
                ".env" if file.held.sha256 != ENV_SHA256 => {
                    return Err(fail("B0 .env 不是固定 Git 原始字节"));
                }
                ".node-version" if file.held.sha256 != NODE_FILE_SHA256 => {
                    return Err(fail("B0 .node-version 不是固定 24.15.0"));
                }
                "package.json" => {
                    let package = files::parse(&file.read()?)?;
                    if package["packageManager"] != "pnpm@11.20.0"
                        || package["scripts"]["build"] != "vite build"
                        || !package["scripts"]["prebuild"].is_null()
                        || !package["scripts"]["postbuild"].is_null()
                    {
                        return Err(fail("B0 package.json 未绑定原 pnpm 与 Vite 构建脚本"));
                    }
                }
                _ => {}
            }
            input_records.push(files::descriptor(root, &file.held.path, &file.held.sha256)?);
            held.push(file);
        }
        for name in DEPENDENCY_FILES {
            let file = stable(&root.join(name))
                .map_err(|_| fail("B0 缺少按原锁文件安装的 pnpm 依赖状态；先核对已有安装"))?;
            dependency_records.push(json!({"path":name,"sha256":file.held.sha256}));
            held.push(file);
        }
        let value = json!({"api_origin":"same-origin","frontend_tree":TREE,
            "inputs":input_records,"dependency_state":dependency_records});
        let result = Self {
            root: root.to_owned(),
            held,
            value,
        };
        shape(&result.value)?;
        result.verify()?;
        Ok(result)
    }

    pub(super) fn verify(&self) -> Result<()> {
        reject_overrides(&self.root)?;
        for file in &self.held {
            file.verify()?;
        }
        Ok(())
    }
}

fn reject_overrides(root: &Path) -> Result<()> {
    for name in ABSENT {
        if files::ordinary(&root.join(name), true, name == EMPTY_CONFIG)?.is_some() {
            return Err(format!("B0 禁止现有构建覆盖文件 {name}，包括空文件；保留现场").into());
        }
    }
    Ok(())
}

pub(super) fn inventory(root: &Path) -> Result<Value> {
    let inventory = source::capture(root)?;
    require_head(&inventory)?;
    let mut command = Command::new("git");
    for (name, _) in std::env::vars_os() {
        if name
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
        {
            command.env_remove(name);
        }
    }
    let observed = command
        .current_dir(root)
        .args(["rev-parse", "HEAD^{tree}"])
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()?;
    if !observed.status.success() || std::str::from_utf8(&observed.stdout)?.trim() != TREE {
        return Err(fail("B0 前端固定 Git tree 不匹配"));
    }
    Ok(source::inventory_value(&inventory))
}

pub(super) fn require_head(inventory: &source::Inventory) -> Result<()> {
    if inventory.source.snapshot.head != HEAD
        || !inventory.source.snapshot.clean
        || !inventory.source.snapshot.files.is_empty()
        || inventory.guard.head != HEAD
    {
        return Err(fail("B0 适配只接受固定且完整干净的前端源码"));
    }
    Ok(())
}

pub(super) fn protect_source(root: &Path, inventory: &Value) -> Result<Vec<StableFile>> {
    let inventory = source::validate_inventory(inventory)?;
    inventory
        .files
        .iter()
        // 旧 Vite 插件会写这两份原声明；前后完整源码清单仍核对两者的实际字节。
        .filter(|item| {
            !(cfg!(windows)
                && matches!(
                    item.path.as_str(),
                    "src/auto-imports.d.ts" | "src/components.d.ts"
                ))
        })
        .map(|item| {
            if !model::relative(&item.path, true) {
                return Err(fail("B0 源码包含无效相对路径"));
            }
            let file = stable(&root.join(&item.path))?;
            if file.held.sha256 != item.sha256 {
                return Err(fail("B0 源码在持有保护句柄前变化"));
            }
            Ok(file)
        })
        .collect()
}

fn compare_source(inventory: &source::Inventory, name: &str, file: &StableFile) -> Result<()> {
    if !inventory
        .files
        .iter()
        .any(|item| item.path == name && item.sha256 == file.held.sha256)
    {
        return Err(fail("B0 原始构建输入未绑定完整来源清单"));
    }
    Ok(())
}

pub(super) fn stable(path: &Path) -> Result<StableFile> {
    let path = files::ordinary(path, false, false)?.ok_or_else(|| fail("B0 缺少已登记文件"))?;
    StableFile::open(&path)
}

pub(super) fn binding(file: &StableFile) -> Value {
    json!({"path":file.held.path,"bytes":file.held.bytes,"sha256":file.held.sha256})
}

pub(super) fn descriptor(value: &Value) -> Result<&Path> {
    model::exact(value, &["path", "bytes", "sha256"])?;
    let path = absolute(&value["path"])?;
    if value["bytes"].as_u64().is_none() || !model::hex(&value["sha256"], 64) {
        return Err(fail("B0 工具文件绑定字节数或 SHA 无效"));
    }
    Ok(path)
}

pub(super) fn absolute(value: &Value) -> Result<&Path> {
    let name = model::text(value)?;
    let path = Path::new(name);
    if !path.is_absolute()
        || name.chars().any(char::is_control)
        || name
            .split(['/', '\\'])
            .any(|part| matches!(part, "." | ".."))
    {
        return Err(fail("B0 工具绑定必须使用无跳转绝对路径"));
    }
    Ok(path)
}

pub(super) fn bound(value: &Value) -> Result<StableFile> {
    let file = stable(descriptor(value)?)?;
    if binding(&file) != *value {
        return Err(fail("B0 原工具文件路径、字节数或 SHA 已变化"));
    }
    Ok(file)
}

pub(super) fn shape(value: &Value) -> Result<()> {
    model::exact(
        value,
        &["api_origin", "frontend_tree", "inputs", "dependency_state"],
    )?;
    if value["api_origin"] != "same-origin" || value["frontend_tree"] != TREE {
        return Err(fail("B0 构建配置不是固定 Git tree 与同源 API"));
    }
    for (field, expected) in [
        ("inputs", SOURCE_FILES.as_slice()),
        ("dependency_state", DEPENDENCY_FILES.as_slice()),
    ] {
        let rows = value[field]
            .as_array()
            .ok_or_else(|| fail("B0 输入必须使用完整有序描述符数组"))?;
        if rows.len() != expected.len() {
            return Err(fail("B0 原始输入描述符数量不同"));
        }
        for (row, name) in rows.iter().zip(expected) {
            model::exact(row, &["path", "sha256"])?;
            if row["path"] != *name || !model::hex(&row["sha256"], 64) {
                return Err(fail("B0 输入路径、排序或 SHA 无效"));
            }
        }
    }
    if value["inputs"][0]["sha256"] != ENV_SHA256
        || value["inputs"][1]["sha256"] != NODE_FILE_SHA256
    {
        return Err(fail("B0 .env 或 .node-version 摘要无效"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn b0_production_rejects_empty_dotenv_overrides() {
        let local = crate::workspace::root_dir().join(".local-tests");
        std::fs::create_dir_all(&local).unwrap();
        let fixture = tempfile::tempdir_in(local).unwrap();
        let root = fixture.path();
        reject_overrides(root).unwrap();
        for name in [".env.local", ".env.production", ".env.production.local"] {
            let path = root.join(name);
            std::fs::write(&path, []).unwrap();
            assert!(reject_overrides(root).is_err());
            std::fs::remove_file(path).unwrap();
            reject_overrides(root).unwrap();
        }
    }
}
