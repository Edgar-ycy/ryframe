use std::{path::Path, process::Command};

use sha2::{Digest, Sha256};

use crate::{Result, devex::BaselineContract};

#[test]
fn embedded_stable_readiness_adapter_has_registered_digest_and_scope() {
    let patch = BaselineContract::STABLE_READINESS_B0_ADAPTER_PATCH;
    assert_eq!(
        sha256(patch),
        BaselineContract::STABLE_READINESS_B0_PATCH_SHA256
    );
    let patch = std::str::from_utf8(patch).unwrap();
    for path in BaselineContract::STABLE_READINESS_B0_ADAPTER_PATHS {
        assert!(patch.contains(&format!("diff --git a/{path} b/{path}")));
    }
    assert_eq!(
        patch.matches("diff --git ").count(),
        BaselineContract::STABLE_READINESS_B0_ADAPTER_PATHS.len()
    );
}

#[test]
fn embedded_adapter_reconstructs_the_registered_b0_tree() {
    let root = crate::workspace::root_dir();
    let index = root.join("target").join(format!(
        "stable-readiness-b0-adapter-{}.index",
        std::process::id()
    ));
    let patch = root.join("xtask/assets/baseline-adapters/stable-readiness-b0-v1.patch");
    let _ = std::fs::remove_file(&index);
    let result = (|| -> Result<()> {
        run_git_with_index(
            &root,
            &index,
            &[
                "read-tree",
                BaselineContract::STABLE_READINESS_B0_BASE_COMMIT,
            ],
        )?;
        let output = Command::new("git")
            .args(["apply", "--cached", "--whitespace=nowarn"])
            .arg(&patch)
            .env("GIT_INDEX_FILE", &index)
            .current_dir(&root)
            .output()?;
        if !output.status.success() {
            return Err(format!(
                "无法从内嵌补丁重建 B0 适配树：{}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
            .into());
        }
        let tree = run_git_with_index(&root, &index, &["write-tree"])?;
        if tree != BaselineContract::STABLE_READINESS_B0_ADAPTER_TREE {
            return Err(format!("B0 适配树不一致：{tree}").into());
        }
        Ok(())
    })();
    let _ = std::fs::remove_file(index);
    result.unwrap();
}

fn run_git_with_index(root: &Path, index: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .env("GIT_INDEX_FILE", index)
        .current_dir(root)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "Git 临时索引命令失败：git {}：{}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let hex = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("sha256:{hex}")
}
