//! 原已审计失败的固定证据数据；不解释或执行历史脚本。
use serde_json::{Value, json};

pub(super) const AUDITED_TREE: &str = "110383e2c6859b86633c2c3b65b037a23c6367e3";
pub(super) const COLLECTION_TREE: &str = "f6312c497c5863c64d53eadfb30ed6d30e0119db";
pub(super) const COLLECTION_MAINTENANCE: &str =
    "86fb5ff5fca2a83a4588f5f52bdf53a2986d7725900f453936ffadeccb401556";
pub(super) const OBSERVATION_TREE: &str = "277a6d8db15e5db1fc4c58645e7885d841dd9080";
pub(super) const OBSERVATION_PRODUCT_TREE: &str = "dad947d8d622264c975aca8fea5fe58e6ae05190";
pub(super) const OBSERVATION_FILES: &str =
    "68df30b793b0953bbb3bedd4c42df3b8653ed3c73065f5ead29de2892f37e9ed";
pub(super) const OBSERVATION_MAINTENANCE: &str =
    "f7c39f0e2939bc130a542485066b624362441b3324336dfe7f5197a449f40a5d";
pub(super) const REREGISTRATION_TREE: &str = "57636e10d241ba8a3ee9917de8a0fb800660479c";

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Start,
    Collection,
    Observation,
    Reregistration,
}

pub(super) fn frames(kind: Kind) -> Value {
    match kind {
        Kind::Start => {
            json!([{"file": "scripts/devex_clone_run.py", "function": "execute", "line": 705}, {"file": "scripts/devex_clone_seed_runtime.py", "function": "execute_seed", "line": 341}, {"file": "scripts/devex_clone_seed_generation.py", "function": "execute_generation", "line": 221}, {"file": "scripts/devex_clone_seed_generation_runtime.py", "function": "preflight", "line": 183}, {"file": "scripts/devex_clone_seed_generation_runtime.py", "function": "checkpoint", "line": 192}, {"file": "scripts/devex_provenance.py", "function": "verify_source", "line": 174}, {"file": "scripts/source_fingerprints.py", "function": "reusable_artifact_source", "line": 349}])
        }
        Kind::Collection => {
            json!([{"file": "scripts/devex_clone_run.py", "function": "execute", "line": 717}, {"file": "scripts/devex_clone_seed_runtime.py", "function": "execute_seed", "line": 341}, {"file": "scripts/devex_clone_seed_generation_control.py", "function": "execute_recover", "line": 208}, {"file": "scripts/devex_clone_seed_generation_prelaunch.py", "function": "execute", "line": 207}, {"file": "scripts/devex_clone_seed_generation_images.py", "function": "capture_image", "line": 45}, {"file": "scripts/devex_clone_export.py", "function": "verify_migrations", "line": 70}, {"file": "scripts/devex_clone_export.py", "function": "run_cli", "line": 55}])
        }
        Kind::Observation => {
            json!([{"file": "scripts/devex_clone_run.py", "function": "execute", "line": 717}, {"file": "scripts/devex_clone_seed_runtime.py", "function": "execute_seed", "line": 341}, {"file": "scripts/devex_clone_seed_generation_control.py", "function": "execute_recover", "line": 209}, {"file": "scripts/devex_clone_seed_generation_prelaunch.py", "function": "execute", "line": 321}, {"file": "scripts/devex_clone_seed_generation_images.py", "function": "capture_image", "line": 51}, {"file": "scripts/devex_clone_export.py", "function": "capture_inventory", "line": 146}, {"file": "scripts/devex_clone_export.py", "function": "run_cli", "line": 55}])
        }
        Kind::Reregistration => {
            json!([{"file": "tools/python/devex_clone_run.py", "function": "execute", "line": 717}, {"file": "tools/python/devex_clone_seed_runtime.py", "function": "execute_seed", "line": 341}, {"file": "tools/python/devex_clone_seed_generation.py", "function": "execute_generation", "line": 255}, {"file": "tools/python/devex_clone_seed_generation.py", "function": "_archived_request", "line": 223}])
        }
    }
}

pub(super) const COLLECTION_FILES: &[(&str, &str)] = &[
    (
        "before/migrations-control.command.json",
        "a301844382215c36cf56ae0e6c1359780b2d7e2996f0bd4493152c7d7584f193",
    ),
    (
        "before/migrations-control.diagnostic.json",
        "57b26e9f805c0d4868b088f722e7082b7ca139ec2b2e8c206054e0803edd45b1",
    ),
    (
        "before/redis-kernel-092424efb7a3421b9b1bfe1b65fd6263.command.json",
        "6eafe6904d152d0f4f3f2a386195e2fcd8570ad21c0d54998cd19fb916fefa77",
    ),
    (
        "before/redis-kernel-5487f322f1ae4bcfa15b3b5763f4a671.command.json",
        "5f8e7d6fcc981566200de0ec1cb6f7b8633b347850cbd18671ced250eb4c2740",
    ),
    (
        "before/redis-kernel-6e408b07957047fc8654dfc90055e857.command.json",
        "b940b84cb4b520c8c527cc8a5a5af6b80c58932e080b0a6f3a7ff96a1a7ce3db",
    ),
    (
        "before/redis-kernel-f3894f4f3eb94497938d9c85b1393cbd.command.json",
        "a53d6a73166255c487eb107ef4d5eaf4e39d01c137329fba126b94c2d86575e8",
    ),
    (
        "before/redis-kernel-f4a244167c2949d2b189b4091dafee9f.command.json",
        "ccbd72c2212d39cf1c3efb1ecbba5d101ac5b85661a6e4e6945b54cfc4dab6bb",
    ),
    (
        "before/storage-layout-4478cbf34ce04c4ca0eeed96443b42bd.json",
        "b43d520f943aef98b56ea749f2ae343c0040defd0c4d11edaf19995641ba2a74",
    ),
    (
        "runtime/binaries.json",
        "86fea2999e3c4323569b672c1b7af6310af0aa09d9a83e7c5dafa753de091d3c",
    ),
    (
        "runtime/runtime.json",
        "b292edafceefad1924651fb526026aa63b4d8ee776a3df714963f1cb1db00567",
    ),
];
