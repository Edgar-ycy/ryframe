use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use serde_json::{Value, json};

use super::super::{
    fail,
    files::{self, StableFile},
    model,
};
use super::{HEAD, inputs};
use crate::Result;

const NODE: &str = "v24.15.0";
const COREPACK: &str = "0.34.6";
const PNPM: &str = "11.20.0";
const VITE: &str = "8.1.5";
const NODE_PROGRAM: &str = if cfg!(windows) { "node.exe" } else { "node" };
#[cfg(windows)]
const NODE_SHA256: &str = "3331e1ffe19874215472217c5e94f5a0c6d8e18c4ac7111d3937aa0ad5e9b4a5";

pub(super) struct Tools {
    node: PathBuf,
    corepack_entry: PathBuf,
    home: PathBuf,
    frontend: PathBuf,
    vite: PathBuf,
    held: Vec<StableFile>,
    pub(super) value: Value,
}

impl Tools {
    pub(super) fn discover(root: &Path) -> Result<Self> {
        let path = env::var_os("PATH").ok_or_else(|| fail("B0 缺少已安装 Node 的明确 PATH"))?;
        let node = env::split_paths(&path)
            .filter(|directory| directory.is_absolute())
            .map(|directory| directory.join(NODE_PROGRAM))
            .find(|path| fs::symlink_metadata(path).is_ok())
            .ok_or_else(|| fail("B0 缺少 Node 24.15.0，先在 PATH 选择明确安装目录"))?;
        let home = env::var_os("COREPACK_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .ok_or_else(|| fail("B0 需要明确绝对 COREPACK_HOME 和已安装 pnpm 11.20.0"))?;
        Self::read(root, &node, &home)
    }

    pub(super) fn registered(root: &Path, value: &Value) -> Result<Self> {
        shape(value)?;
        let node = inputs::descriptor(&value["node"]["file"])?;
        let home = inputs::absolute(&value["pnpm"]["home"])?;
        let actual = Self::read(root, node, home)?;
        if actual.value != *value {
            return Err(fail("B0 登记 Node、Corepack、pnpm 或 Vite 工具发生漂移"));
        }
        Ok(actual)
    }

    fn read(root: &Path, node: &Path, home: &Path) -> Result<Self> {
        if node.file_name().is_none_or(|name| name != NODE_PROGRAM) {
            return Err(fail("B0 Node 必须使用原安装的固定程序名称"));
        }
        let mut held = Vec::new();
        let node_file = file(node, &mut held)?;
        #[cfg(windows)]
        if node_file["sha256"] != NODE_SHA256 {
            return Err(fail(
                "B0 需要原 Node v24.15.0 Windows x64 可执行文件；核对绝对 PATH",
            ));
        }
        let node = inputs::descriptor(&node_file)?.to_owned();
        let node_dir = node.parent().ok_or_else(|| fail("B0 Node 缺少安装目录"))?;
        let corepack = if cfg!(windows) {
            node_dir.join("node_modules/corepack")
        } else {
            node_dir
                .parent()
                .ok_or_else(|| fail("B0 Node 缺少安装根"))?
                .join("lib/node_modules/corepack")
        };
        let (corepack_package, pkg) = document(&corepack.join("package.json"), &mut held)?;
        if pkg["name"] != "corepack" || pkg["version"] != COREPACK {
            return Err(fail("B0 需要 Node 原捆绑 Corepack 0.34.6"));
        }
        let corepack_entry = file(&corepack.join("dist/corepack.js"), &mut held)?;
        let corepack_impl = file(&corepack.join("dist/lib/corepack.cjs"), &mut held)?;
        let home = files::directory(home, home)?;
        let pnpm = home.join("v1/pnpm").join(PNPM);
        let (pnpm_package, pkg) = document(&pnpm.join("package.json"), &mut held)
            .map_err(|_| fail("B0 离线 Corepack 缓存缺少已安装 pnpm 11.20.0"))?;
        let (installation, installed) = document(&pnpm.join(".corepack"), &mut held)?;
        if pkg["name"] != "pnpm"
            || pkg["version"] != PNPM
            || pkg["bin"]["pnpm"] != "bin/pnpm.mjs"
            || installed["locator"]["name"] != "pnpm"
            || installed["locator"]["reference"] != PNPM
            || installed["bin"]["pnpm"] != "./bin/pnpm.mjs"
        {
            return Err(fail("B0 Corepack 缓存内 pnpm 版本、入口或安装记录无效"));
        }
        let pnpm_entry = file(&pnpm.join("bin/pnpm.mjs"), &mut held)?;
        let pnpm_impl = file(&pnpm.join("dist/pnpm.mjs"), &mut held)?;
        let vite = vite_directory(root)?;
        let (vite_package, pkg) = document(&vite.join("package.json"), &mut held)?;
        if pkg["name"] != "vite" || pkg["version"] != VITE || pkg["bin"]["vite"] != "bin/vite.js" {
            return Err(fail("B0 已装 Vite 与原锁定 8.1.5 不一致"));
        }
        let vite_entry = file(&vite.join("bin/vite.js"), &mut held)?;
        let value = json!({
            "node":{"version":NODE,"file":node_file},
            "corepack":{"version":COREPACK,"package":corepack_package,"entry":corepack_entry,"implementation":corepack_impl},
            "pnpm":{"version":PNPM,"home":home,"package":pnpm_package,"entry":pnpm_entry,"implementation":pnpm_impl,"installation":installation},
            "vite":{"version":VITE,"package":vite_package,"entry":vite_entry},
        });
        let result = Self {
            node,
            corepack_entry: inputs::descriptor(&corepack_entry)?.to_owned(),
            home,
            frontend: root.to_owned(),
            vite,
            held,
            value,
        };
        result.verify()?;
        Ok(result)
    }

    pub(super) fn verify(&self) -> Result<()> {
        if vite_directory(&self.frontend)? != self.vite {
            return Err(fail("B0 pnpm 当前 Vite 解析位置发生变化"));
        }
        for file in &self.held {
            file.verify()?;
        }
        Ok(())
    }

    pub(super) fn observe_versions(&self, root: &Path) -> Result<()> {
        let mut node = self.command(root)?;
        node.arg("--version");
        version(node, NODE)?;
        let mut corepack = self.command(root)?;
        corepack.arg(&self.corepack_entry).arg("--version");
        version(corepack, COREPACK)?;
        let mut pnpm = self.command(root)?;
        pnpm.arg(&self.corepack_entry).args(["pnpm", "--version"]);
        version(pnpm, PNPM)?;
        self.verify()
    }

    pub(super) fn build(&self, root: &Path) -> Result<()> {
        let control = root.join(".local-tests");
        match fs::create_dir(&control) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
        files::directory(root, &control)?;
        let mut command = self.command(root)?;
        command
            .arg(&self.corepack_entry)
            .args(["pnpm", "build"])
            .env("VITE_APP_BUILD_COMMIT", HEAD);
        crate::recovery::full_stack::execution::capture(
            command,
            root,
            &[],
            Duration::from_secs(1800),
            None,
            false,
            false,
        )?;
        Ok(())
    }

    fn command(&self, root: &Path) -> Result<Command> {
        let mut command = Command::new(&self.node);
        command.current_dir(root).env_clear().stdin(Stdio::null());
        let mut paths = vec![
            self.node
                .parent()
                .ok_or_else(|| fail("B0 Node 缺少安装目录"))?
                .to_owned(),
        ];
        #[cfg(windows)]
        {
            let system_root = env::var_os("SystemRoot")
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .ok_or_else(|| fail("B0 构建缺少 Windows SystemRoot"))?;
            let system = system_root.join("System32");
            command
                .env("SystemRoot", system_root)
                .env("COMSPEC", system.join("cmd.exe"))
                .env("PATHEXT", ".COM;.EXE;.BAT;.CMD");
            paths.push(system);
        }
        #[cfg(not(windows))]
        paths.extend([PathBuf::from("/usr/bin"), PathBuf::from("/bin")]);
        for name in ["TEMP", "TMP"] {
            if let Some(value) = env::var_os(name) {
                command.env(name, value);
            }
        }
        let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
        command
            .env("PATH", env::join_paths(paths)?)
            .env("COREPACK_HOME", &self.home)
            .env("COREPACK_ENABLE_NETWORK", "0")
            .env("COREPACK_DEFAULT_TO_LATEST", "0")
            .env("COREPACK_ENV_FILE", "0")
            .env("NODE_DISABLE_COMPILE_CACHE", "1")
            .env("XDG_CONFIG_HOME", root.join(inputs::EMPTY_CONFIG))
            .env("npm_config_userconfig", null)
            .env("npm_config_globalconfig", null)
            .env("npm_config_offline", "true")
            .env("pnpm_config_enable_global_virtual_store", "false")
            .env("pnpm_config_verify_deps_before_run", "error");
        Ok(command)
    }
}

fn vite_directory(root: &Path) -> Result<PathBuf> {
    let virtual_store = files::directory(root, &root.join("node_modules/.pnpm"))?.canonicalize()?;
    let vite = root
        .join("node_modules/vite")
        .canonicalize()
        .map_err(|_| fail("B0 缺少已锁定安装的 Vite"))?;
    if !vite.starts_with(&virtual_store) || vite.file_name().is_none_or(|name| name != "vite") {
        return Err(fail("B0 Vite 必须来自原项目的 pnpm 虚拟依赖目录"));
    }
    files::directory(root, &vite)
}

fn document(path: &Path, held: &mut Vec<StableFile>) -> Result<(Value, Value)> {
    let (file, value) = files::document(path)?;
    let descriptor = inputs::binding(&file);
    held.push(file);
    Ok((descriptor, value))
}

fn file(path: &Path, held: &mut Vec<StableFile>) -> Result<Value> {
    let file = inputs::stable(path)?;
    let descriptor = inputs::binding(&file);
    held.push(file);
    Ok(descriptor)
}

fn version(mut command: Command, expected: &str) -> Result<()> {
    let output = command.output()?;
    if !output.status.success() || std::str::from_utf8(&output.stdout)?.trim() != expected {
        return Err(format!("B0 工具实测版本不是 {expected}；检查明确安装路径与离线缓存").into());
    }
    Ok(())
}

pub(super) fn shape(value: &Value) -> Result<()> {
    model::exact(value, &["node", "corepack", "pnpm", "vite"])?;
    for (role, version, files) in [
        ("node", NODE, ["file"].as_slice()),
        (
            "corepack",
            COREPACK,
            ["package", "entry", "implementation"].as_slice(),
        ),
        (
            "pnpm",
            PNPM,
            ["package", "entry", "implementation", "installation"].as_slice(),
        ),
        ("vite", VITE, ["package", "entry"].as_slice()),
    ] {
        let mut fields = vec!["version"];
        fields.extend(files);
        if role == "pnpm" {
            fields.push("home");
            inputs::absolute(&value[role]["home"])?;
        }
        model::exact(&value[role], &fields)?;
        if value[role]["version"] != version {
            return Err(fail("B0 工具收据版本不同于固定原版本"));
        }
        for file in files {
            inputs::descriptor(&value[role][*file])?;
        }
    }
    Ok(())
}
