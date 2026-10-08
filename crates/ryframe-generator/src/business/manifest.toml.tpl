[package]
name = "{package}"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
authors.workspace = true
license.workspace = true
repository.workspace = true

[package.metadata.ryframe]
kind = "business"
module = "{module}"

[features]
default = ["api", "migration"]
catalog = []
runtime = ["ryframe-sdk/runtime"]
persistence = ["runtime", "ryframe-sdk/persistence", "dep:sea-orm"]
api = ["persistence", "ryframe-sdk/api", "dep:serde", "dep:utoipa", "dep:validator"]
migration = ["persistence", "ryframe-sdk/migration"]

[dependencies]
ryframe-sdk = {{ path = "../ryframe-sdk", default-features = false }}
sea-orm = {{ workspace = true, optional = true }}
serde = {{ workspace = true, optional = true }}
utoipa = {{ workspace = true, optional = true }}
validator = {{ workspace = true, optional = true }}

[lints]
workspace = true
