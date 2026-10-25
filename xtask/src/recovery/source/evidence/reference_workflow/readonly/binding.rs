use super::{Result, bound_image, defaults, files, inputs, native, path, text};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub(super) struct Inputs {
    pub(super) backend: PathBuf,
    pub(super) work: PathBuf,
    pub(super) registration: PathBuf,
    pub(super) plan: Value,
    files: BTreeMap<PathBuf, Value>,
    directories: BTreeMap<PathBuf, (u64, u64)>,
}

impl Inputs {
    pub(super) fn capture(
        backend: &Path,
        plan_path: &Path,
        plan: &Value,
        work: &Path,
        side: &str,
    ) -> Result<Self> {
        let registration = path(&plan[side]["runtime_dir"])?.join("api.json");
        let mut result = Self {
            backend: backend.to_owned(),
            work: work.to_owned(),
            registration,
            plan: plan.clone(),
            files: BTreeMap::new(),
            directories: BTreeMap::new(),
        };
        result.add(plan_path)?;
        if native::json::read(plan_path)? != *plan {
            return Err("参考计划在读取和绑定之间变化".into());
        }
        result.add(&work.join("reference-owner.json"))?;
        result.add(&result.registration.clone())?;
        for name in ["mysql", "mysqldump"] {
            result.add(path(&plan["tools"][name]["path"])?)?;
        }
        for name in ["source", "target"] {
            for db in plan[name]["databases"].as_array().ok_or("数据库列表缺失")? {
                let defaults = path(&db["defaults_file"])?;
                if name == side {
                    defaults::validate(defaults)?;
                }
                result.add(defaults)?;
            }
            let config = &plan[name]["s3"]["configuration"];
            for filename in config["files"]
                .as_object()
                .ok_or("配置绑定文件缺失")?
                .keys()
            {
                result.add(&path(&config["directory"])?.join(filename))?;
            }
        }
        for directory in [work, path(&plan[side]["runtime_dir"])?] {
            super::super::unlinked(directory)?;
            let metadata = fs::metadata(directory)?;
            result.directories.insert(
                directory.to_owned(),
                native::resources::common::device_inode_for(directory, &metadata)?,
            );
        }
        result.unchanged()?;
        Ok(result)
    }

    fn add(&mut self, path: &Path) -> Result<()> {
        self.files.insert(path.to_owned(), bound_image(path)?);
        Ok(())
    }

    pub(super) fn unchanged(&self) -> Result<()> {
        for (path, expected) in &self.files {
            if bound_image(path)? != *expected {
                return Err("只读参考检查的计划、工具、配置或进程登记文件身份／字节变化".into());
            }
        }
        for (path, expected) in &self.directories {
            super::super::unlinked(path)?;
            if native::resources::common::device_inode_for(path, &fs::metadata(path)?)? != *expected
            {
                return Err("只读参考检查目录身份变化".into());
            }
        }
        inputs::domain::validate_plan(&self.plan, &self.backend)?;
        files::work(&json!({"plan":self.plan}), false)?;
        match fs::symlink_metadata(self.work.join("unused-login.cnf")) {
            Ok(_) => return Err("参考工具不读取隐式 MySQL login-path 文件".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("无法确认受管 MySQL login-path 文件缺失；保留现场".into()),
        }
        Ok(())
    }

    pub(super) fn program(&self) -> Result<PathBuf> {
        Ok(PathBuf::from(text(&self.plan["tools"]["mysql"]["path"])?))
    }
}
