use super::{message, snapshot::SourceSnapshot};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(super) struct Candidate {
    path: PathBuf,
    target: PathBuf,
    original: SourceSnapshot,
    _directory: tempfile::TempDir,
}

impl Candidate {
    pub(super) fn from_control() -> Result<Option<Self>, String> {
        let control = std::env::current_dir()
            .map_err(message)?
            .join("resource-workspace-control.json");
        match fs::symlink_metadata(&control) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                eprintln!("本次仅核验导入后端闭环；跨仓页面校验需从受管控制目录启动");
                return Ok(None);
            }
            Err(error) => {
                return Err(format!(
                    "无法检查资源 Workspace 控制文件 {}：{error}",
                    control.display()
                ));
            }
            Ok(_) => {}
        }
        let (root, _profile, target) = crate::resource_workspace_control::load()?;
        let original = SourceSnapshot::capture(&root)?;
        let cache = root.join(".local-tests/mysql-cli-import");
        fs::create_dir_all(&cache).map_err(message)?;
        let directory = tempfile::tempdir_in(cache).map_err(message)?;
        let path = directory.path().join("frontend");
        original.copy_to(&path)?;
        println!("→ 真实两表反向导入继续核验候选契约和前端页面");
        Ok(Some(Self {
            path,
            target,
            original,
            _directory: directory,
        }))
    }

    pub(super) fn configure_target(&self, backend: &Path) -> Result<(), String> {
        fs::create_dir_all(backend.join(".local-tests")).map_err(message)?;
        let mut generator = toml::Table::new();
        generator.insert(
            "target_dir".into(),
            self.target
                .to_str()
                .ok_or("受管构建目录不是有效 UTF-8")?
                .into(),
        );
        let mut tools = toml::Table::new();
        tools.insert("generator".into(), generator.into());
        fs::write(
            backend.join(".local-tests/tools.toml"),
            toml::to_string(&tools).map_err(message)?,
        )
        .map_err(message)
    }

    pub(super) fn configure(&self, backend: &Path) -> Result<(), String> {
        let path = backend.join(".local-tests/tools.toml");
        let mut tools: toml::Table =
            toml::from_str(&fs::read_to_string(&path).map_err(message)?).map_err(message)?;
        let workspace = tools
            .entry("workspace")
            .or_insert_with(|| toml::Value::Table(toml::Table::new()))
            .as_table_mut()
            .ok_or("本机工具配置 workspace 必须是表")?;
        workspace.insert(
            "frontend_dir".into(),
            self.path
                .to_str()
                .ok_or("候选前端目录不是有效 UTF-8")?
                .into(),
        );
        fs::write(path, toml::to_string(&tools).map_err(message)?).map_err(message)
    }

    pub(super) fn verify(&self, backend: &Path) -> Result<(), String> {
        let document: serde_json::Value = serde_json::from_slice(
            &fs::read(backend.join("openapi/openapi.json")).map_err(message)?,
        )
        .map_err(message)?;
        let directory = document["x-ryframe-menu-routes"]["routes"]
            .as_array()
            .ok_or("缺少候选菜单目录")?
            .iter()
            .find(|route| route["route_key"] == "cli-import")
            .ok_or("缺少本批业务根目录")?;
        assert_eq!(directory["menu_type"], "M");
        assert!(directory["page_key"].is_null());
        assert!(directory["permission_code"].is_null());
        assert_eq!(
            fs::read(backend.join("openapi/openapi.json")).map_err(message)?,
            fs::read(self.path.join("openapi/openapi.json")).map_err(message)?
        );
        let operations =
            fs::read_to_string(self.path.join("src/api/generated/operations/system.ts"))
                .map_err(message)?;
        for name in ["table1", "table2"] {
            let root = self.path.join("src/generated/resources").join(name);
            assert!(root.join("page.vue").is_file());
            let api = fs::read_to_string(root.join("api.ts")).map_err(message)?;
            assert!(api.contains("@/api/generated/operations/system"));
            assert!(operations.contains(&format!("get_business_cli_import_{name}")));
            let registration = fs::read_to_string(root.join("registration.ts")).map_err(message)?;
            assert!(registration.contains("requiresSystemTenant: true"));
        }
        self.original.verify()
    }
}
