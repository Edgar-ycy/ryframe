//! 分开登记原构建源码和测量期间实际观察的完整工作树。
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::inputs::Observation;
use crate::{
    devex::runtime_native::failure::{Failure, NativeResult},
    local_test_path::{LocalTestPathKind, validate_absolute_path},
    recovery::clone::native::source::{self, Inventory},
};

pub(super) struct Sources {
    pub(super) product_backend: PathBuf,
    pub(super) snapshots: Value,
    pub(super) execution_sources: Value,
    backend: Tree,
    frontend: Tree,
    product: Tree,
    driver: Tree,
    runner_frontend: Tree,
}

impl Sources {
    pub(super) fn read(request: &Value, input: &Observation) -> NativeResult<Self> {
        let runtime = &input.runtime.value;
        let backend_snapshot = built(runtime, request, "backend", "backend_source")?;
        let backend = Tree::read(&input.backend, "backend_source")?;
        let frontend_snapshot = built(runtime, request, "frontend", "frontend_source")?;
        let frontend = Tree::read(&input.frontend, "frontend_source")?;
        let driver = Tree::read(&input.driver, "driver_source")?;
        let driver_source = driver.execution_source("driver_source")?;
        let runner_frontend = Tree::read(&input.runner_frontend, "runner_frontend_source")?;
        let runner_source = runner_frontend.execution_source("runner_frontend_source")?;
        driver.require_fingerprint(&request["driver_fingerprint"], "driver_source")?;
        runner_frontend.require_fingerprint(
            &request["runner_frontend_fingerprint"],
            "runner_frontend_source",
        )?;
        let product_backend = product_path(runtime, "backend_product_source")?;
        let product = Tree::read(&product_backend, "backend_product_source")?;
        product.require_product(runtime, "backend_product_source")?;
        let snapshots = json!({
            "backend_execution":backend_snapshot,"frontend":frontend_snapshot,
            "backend_product":source::binding_value(&product.inventory.source)["snapshot"],
            "driver":driver_source["snapshot"],"runner_frontend":runner_source["snapshot"],
        });
        Ok(Self {
            product_backend,
            snapshots,
            execution_sources: json!({"driver":driver_source,"runner_frontend":runner_source}),
            backend,
            frontend,
            product,
            driver,
            runner_frontend,
        })
    }

    pub(super) fn assert_unchanged(
        &self,
        request: &Value,
        input: &Observation,
    ) -> NativeResult<()> {
        let runtime = &input.runtime.value;
        for (tree, path, role, field, category) in [
            (
                &self.backend,
                &input.backend,
                "backend",
                "backend_execution",
                "backend_source_stable",
            ),
            (
                &self.frontend,
                &input.frontend,
                "frontend",
                "frontend",
                "frontend_source_stable",
            ),
        ] {
            tree.assert_unchanged(path, category)?;
            if built(runtime, request, role, category)? != self.snapshots[field] {
                return Err(Failure::new(category));
            }
        }
        self.driver
            .assert_unchanged(&input.driver, "driver_source_stable")?;
        self.driver
            .require_fingerprint(&request["driver_fingerprint"], "driver_source_stable")?;
        self.runner_frontend
            .assert_unchanged(&input.runner_frontend, "runner_frontend_source_stable")?;
        self.runner_frontend.require_fingerprint(
            &request["runner_frontend_fingerprint"],
            "runner_frontend_source_stable",
        )?;
        let category = "backend_product_source_stable";
        self.product
            .assert_unchanged(&product_path(runtime, category)?, category)?;
        self.product.require_product(runtime, category)
    }
}

struct Tree {
    path: PathBuf,
    inventory: Inventory,
}

impl Tree {
    fn read(path: &Path, category: &'static str) -> NativeResult<Self> {
        let inventory = source::capture(path).map_err(|_| Failure::new(category))?;
        Ok(Self {
            path: path.to_owned(),
            inventory,
        })
    }

    fn assert_unchanged(&self, path: &Path, category: &'static str) -> NativeResult<()> {
        if path != self.path.as_path()
            || source::capture(path).map_err(|_| Failure::new(category))? != self.inventory
        {
            return Err(Failure::new(category));
        }
        Ok(())
    }

    fn require_fingerprint(&self, requested: &Value, category: &'static str) -> NativeResult<()> {
        if requested.as_str() != Some(self.inventory.source.worktree_fingerprint.as_str()) {
            return Err(Failure::new(category));
        }
        Ok(())
    }

    fn require_product(&self, runtime: &Value, category: &'static str) -> NativeResult<()> {
        let snapshot = &self.inventory.source.snapshot;
        if !snapshot.clean
            || runtime["source"]["backend_product_sha"].as_str() != Some(snapshot.head.as_str())
        {
            return Err(Failure::new(category));
        }
        Ok(())
    }

    fn execution_source(&self, category: &'static str) -> NativeResult<Value> {
        source::execution_source(&self.inventory).map_err(|_| Failure::new(category))
    }
}

fn product_path(runtime: &Value, category: &'static str) -> NativeResult<PathBuf> {
    let path = runtime["paths"]["backend_product_root"]
        .as_str()
        .map(Path::new)
        .ok_or_else(|| Failure::new(category))?;
    validate_absolute_path(path, LocalTestPathKind::ExistingDirectory)
        .map_err(|_| Failure::new(category))
}

fn built(
    runtime: &Value,
    request: &Value,
    role: &str,
    category: &'static str,
) -> NativeResult<Value> {
    // 构建快照来自登记收据；实际源码、产物和工具链由在线核验器交叉检查。
    let inventory = source::validate_inventory(&runtime[role]["sources"]["full"])
        .map_err(|_| Failure::new(category))?;
    if request["source_fingerprints"][role].as_str()
        != Some(inventory.source.worktree_fingerprint.as_str())
    {
        return Err(Failure::new(category));
    }
    Ok(source::binding_value(&inventory.source)["snapshot"].clone())
}
