//! 保留服务历史的路径、描述和前后复核；精确编码共用lifecycle state能力。
use super::evidence::{binding, check, exact, text};
use crate::{Result, recovery::clone::native::lifecycle::state};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{path::Path, rc::Rc};

#[cfg(windows)]
use crate::recovery::clone::native::target_verify::guard::Inputs;

const CHANGED: &str = "阶段记录在历史前缀核对期间变化";

pub(super) fn count(run: &Path, descriptor: &Value) -> Result<usize> {
    let descriptor = Descriptor::read(run, descriptor)?;
    let ledger = Ledger::read(run)?;
    let count = ledger.count(descriptor)?;
    ledger.check_binding(run)?;
    Ok(count)
}

pub(super) struct Descriptor<'a> {
    bytes: u64,
    sha256: &'a str,
}

impl<'a> Descriptor<'a> {
    pub(super) fn read(run: &Path, descriptor: &'a Value) -> Result<Self> {
        exact(descriptor, &["path", "bytes", "sha256"])?;
        check(
            super::evidence::path_is(&descriptor["path"], &run.join("state.json")),
            "历史阶段记录描述不属于固定运行目录",
        )?;
        let bytes = descriptor["bytes"]
            .as_u64()
            .filter(|bytes| *bytes > 0)
            .ok_or("历史阶段记录描述不属于固定运行目录")?;
        let sha256 = text(descriptor, "sha256")?;
        check(
            sha256.len() == 64
                && sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
            "历史阶段记录摘要无效",
        )?;
        Ok(Self { bytes, sha256 })
    }
}

#[derive(PartialEq, Eq)]
struct Ledger {
    binding: Value,
    raw: Vec<u8>,
}

impl Ledger {
    fn read(run: &Path) -> Result<Self> {
        let path = run.join("state.json");
        let before = binding(&path)?;
        let value = state::load(run, true)?;
        let raw = std::fs::read(&path)?;
        check(
            Some(raw.len() as u64) == before["bytes"].as_u64()
                && hex::encode(Sha256::digest(&raw)) == text(&before, "sha256")?
                && serde_json::from_slice::<Value>(&raw)? == value,
            CHANGED,
        )?;
        Ok(Self {
            binding: before,
            raw,
        })
    }

    fn count(&self, descriptor: Descriptor<'_>) -> Result<usize> {
        state::prefix::count(&self.raw, descriptor.bytes, descriptor.sha256)
    }

    fn check_binding(&self, run: &Path) -> Result<()> {
        check(binding(&run.join("state.json"))? == self.binding, CHANGED)
    }
}

pub(super) struct Snapshot<'a> {
    run: &'a Path,
    ledger: Ledger,
    prefixes: Rc<state::prefix::PrefixIndex>,
    #[cfg(windows)]
    protection: Inputs,
}

impl<'a> Snapshot<'a> {
    pub(super) fn open(run: &'a Path) -> Result<Self> {
        #[cfg(windows)]
        let protection = Inputs::open([run.join("state.json")])?;
        let ledger = Ledger::read(run)?;
        ledger.check_binding(run)?;
        #[cfg(windows)]
        protection.check()?;
        let prefixes = state::prefix::PrefixIndex::shared(&ledger.raw)?;
        Ok(Self {
            run,
            ledger,
            prefixes,
            #[cfg(windows)]
            protection,
        })
    }

    pub(super) fn count(&self, descriptor: Descriptor<'_>) -> Result<usize> {
        self.prefixes.count(descriptor.bytes, descriptor.sha256)
    }

    pub(super) fn check_end(&self) -> Result<()> {
        // 重新核对完整账本及所有已登记结果。调用方可提供已观察的历史前缀。
        let current = Ledger::read(self.run)?;
        current.check_binding(self.run)?;
        check(current == self.ledger, CHANGED)?;
        #[cfg(windows)]
        self.protection.check()?;
        Ok(())
    }
}
