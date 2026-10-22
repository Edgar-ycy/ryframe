use std::{fs, path::Path};

pub fn write_configuration(
    directory: &Path,
    api_port: u16,
    health_port: u16,
    password: Option<&str>,
) {
    fs::create_dir_all(directory).unwrap();
    let base = format!(
        r#"snowflake_worker_id = 1
scope_id = "file-config-fixture"
[app]
name = "file-config-fixture"
host = "127.0.0.1"
port = {api_port}
[database.primary]
host = "127.0.0.1"
port = 1
database = "must_not_connect"
username = "fixture"
password = "fixture-database-password"
max_connections = 2
min_connections = 0
[auth]
jwt_secret = "isolated-file-configuration-fixture-secret"
access_token_expire = "1h"
refresh_token_expire = "168h"
[logger]
level = "info"
format = "text"
output = "stdout"
retention_days = 7
[jobs]
mode = "external"
health_port = {health_port}
[object_storage]
backend = "local"
local_base_dir = "must-not-create-storage"
"#
    );
    fs::write(directory.join("app.toml"), base).unwrap();
    let overlay = password.map_or_else(String::new, |password| {
        format!(
            "[bootstrap]\nadmin_password = {}\n",
            serde_json::to_string(password).unwrap()
        )
    });
    fs::write(directory.join("app.test.toml"), overlay).unwrap();
}
