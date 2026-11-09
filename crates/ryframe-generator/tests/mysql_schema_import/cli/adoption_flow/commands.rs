use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use sha2::{Digest, Sha256};

use super::{message, snapshot, workspace::Fixture};

impl Fixture {
    pub(super) fn build(&self, stage: &str) -> Result<PathBuf, String> {
        let source = snapshot::SourceSnapshot::capture(&self.root)?;
        let inputs = source
            .input_paths()
            .map(|path| {
                let digest = match fs::read(self.root.join(path)) {
                    Ok(bytes) => Some(hex::encode(Sha256::digest(bytes))),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(error) => return Err(message(error)),
                };
                Ok(serde_json::json!({"path": path, "sha256": digest}))
            })
            .collect::<Result<Vec<_>, String>>()?;
        fs::write(
            self.directory.join(format!("{stage}-source.json")),
            serde_json::to_vec_pretty(&inputs).map_err(message)?,
        )
        .map_err(message)?;
        let target = super::GenerationBuilder::new(&self.root)
            .operation(super::GenerationOperation::Resource)
            .package("cli-import-business")
            .sync_frontend(false)
            .plan()?
            .target_dir;
        let mut command = Command::new("cargo");
        command
            .current_dir(&self.root)
            .env("APP_ENV", "test")
            .args([
                "build",
                "--locked",
                "--no-default-features",
                "--features",
                "bin-migrate",
                "-p",
                "ryframe",
                "--bin",
                "ryframe-migrate",
                "--message-format=json",
            ])
            .arg("--target-dir")
            .arg(target);
        let output = self.run(command, stage)?;
        source.verify()?;
        let mut executables = Vec::new();
        for line in String::from_utf8(output.stdout).map_err(message)?.lines() {
            let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if value["reason"] == "compiler-artifact"
                && value["target"]["name"] == "ryframe-migrate"
                && let Some(path) = value["executable"].as_str()
            {
                executables.push(PathBuf::from(path));
            }
        }
        if executables.len() != 1 {
            return Err("Cargo JSON 没有给出唯一 migrate 可执行产物".into());
        }
        self.freeze_executable(stage, &executables[0])
    }

    fn freeze_executable(&self, stage: &str, actual: &Path) -> Result<PathBuf, String> {
        let frozen = self
            .directory
            .join(format!("{stage}-migrate{}", std::env::consts::EXE_SUFFIX));
        let before = {
            let bytes = fs::read(actual).map_err(message)?;
            (bytes.len(), hex::encode(Sha256::digest(bytes)))
        };
        fs::copy(actual, &frozen).map_err(message)?;
        let frozen_digest = {
            let bytes = fs::read(&frozen).map_err(message)?;
            (bytes.len(), hex::encode(Sha256::digest(bytes)))
        };
        let after = {
            let bytes = fs::read(actual).map_err(message)?;
            (bytes.len(), hex::encode(Sha256::digest(bytes)))
        };
        if before != frozen_digest || before != after {
            return Err("Cargo 实际产物在冻结期间变化或副本不一致；保留现场，不使用该副本".into());
        }
        let receipt = serde_json::json!({
            "actual": {"path": actual, "bytes": before.0, "sha256": before.1},
            "frozen": {"path": frozen, "bytes": frozen_digest.0, "sha256": frozen_digest.1},
            "actual_after_copy": {"path": actual, "bytes": after.0, "sha256": after.1},
        });
        fs::write(
            self.directory.join(format!("{stage}-migrate-source.json")),
            serde_json::to_vec_pretty(&receipt).map_err(message)?,
        )
        .map_err(message)?;
        fs::write(
            self.directory.join(format!("{stage}-migrate.sha256")),
            &frozen_digest.1,
        )
        .map_err(message)?;
        Ok(frozen)
    }

    pub(super) fn migrate(
        &self,
        executable: &std::path::Path,
        stage: &str,
        args: &[&str],
    ) -> Result<Output, String> {
        self.verify_configuration()?;
        let mut command = Command::new(executable);
        command
            .current_dir(&self.root)
            .env("APP_ENV", "test")
            .arg("--config-dir")
            .arg(&self.migration_configuration)
            .args(args);
        let result = self.run(command, stage);
        self.verify_configuration()?;
        result
    }

    pub(super) fn import(&self) -> Result<(), String> {
        let mut command = super::super::import_command(&self.root, &self.configuration, "table1")?;
        command.args(["--no-sync-frontend", "--write"]);
        self.run(command, "04-import")?;
        self.configuration.verify()
    }

    fn run(&self, mut command: Command, stage: &str) -> Result<Output, String> {
        let receipt = self.directory.join(stage);
        fs::create_dir(&receipt).map_err(message)?;
        fs::write(receipt.join("command.txt"), format!("{command:?}\n")).map_err(message)?;
        let output = command.output().map_err(message)?;
        fs::write(receipt.join("stdout"), &output.stdout).map_err(message)?;
        fs::write(receipt.join("stderr"), &output.stderr).map_err(message)?;
        fs::write(receipt.join("status.txt"), output.status.to_string()).map_err(message)?;
        super::super::require_success(Output {
            status: output.status,
            stdout: Vec::new(),
            stderr: Vec::new(),
        })
        .map_err(|_| {
            format!(
                "阶段 {stage} 失败；保留 {}，不重放未知写入",
                receipt.display()
            )
        })?;
        Ok(output)
    }
}
