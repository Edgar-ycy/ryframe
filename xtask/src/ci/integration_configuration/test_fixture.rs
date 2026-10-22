use std::{
    fs,
    path::{Path, PathBuf},
};

pub const SECRET: &str = "service-config-private-sentinel";
pub const BASE: &str = r#"snowflake_worker_id = 1
scope_id = "service-fixture"
[app]
name = "service-fixture"
host = "127.0.0.1"
port = 8080
[database.primary]
host = "127.0.0.1"
port = 1
database = "must_not_connect"
username = "fixture"
password = "service-config-private-sentinel"
max_connections = 2
min_connections = 0
[auth]
jwt_secret = "service-config-private-sentinel"
access_token_expire = "1h"
refresh_token_expire = "168h"
[logger]
level = "info"
format = "text"
output = "stdout"
retention_days = 7
[jobs]
mode = "external"
health_port = 9091
[object_storage]
backend = "local"
local_base_dir = "must-not-create-storage"
[redis]
mode = "required"
host = "127.0.0.1"
port = 16379
password = "service-config-private-sentinel"
database = 15
max_pool_size = 9
timeout_secs = 7
tls = false
"#;

pub fn fixture() -> tempfile::TempDir {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join("architecture/crate-boundaries.toml").is_file())
        .unwrap();
    let parent = root.join(".local-tests/service-config-offline");
    fs::create_dir_all(&parent).unwrap();
    tempfile::Builder::new()
        .prefix("private-")
        .tempdir_in(parent)
        .unwrap()
}

pub fn source(root: &Path, common: bool) -> PathBuf {
    let source = root.join("source");
    fs::create_dir(&source).unwrap();
    if common {
        fs::write(source.join("app.toml"), BASE).unwrap();
    }
    fs::write(
        source.join("app.test.toml"),
        if common { "# selected\n" } else { BASE },
    )
    .unwrap();
    source
}
