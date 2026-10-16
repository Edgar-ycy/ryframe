# RyFrame

[简体中文](README.md)

RyFrame is a Rust 2024 framework for enterprise administration, paired with a Vue 3 frontend. This is a `0.x` development version: use current contracts and a new isolated database rather than overwriting an older development installation.

It includes users, roles, permissions, organizations, product plans, tenancy, configuration transfer, database migration, monitoring, messages, scheduling, imports, exports, profiles, passwords, avatars and login sessions. Plans may have no optional capabilities. Notices and tenant message publishing can be enabled per plan; disabling them preserves history and does not disable system notifications, scheduling, imports, exports, backups or recovery.

The business runtime is Rust. API, Worker, migration, reset and generation have direct commands; `cargo xtask` maintains the framework itself.

## First run after downloading

Backend and frontend are separate repositories. Run these commands from the backend root. Examples use Windows PowerShell; MySQL, Redis and RustFS may run on any suitable platform or remote server. WSL is not required.

1. Install rustup and run `rustup show` in the repository to select the stable toolchain, rustfmt and Clippy pinned by `rust-toolchain.toml`.
2. Prepare MySQL 8.0.16 or newer, Redis, and a database account with schema, migration and application permissions. MariaDB is not a substitute. File storage defaults to a local directory; RustFS, MinIO and S3 are also supported.
3. Create an empty database with a database client, for example:

```sql
CREATE DATABASE ryframe_dev CHARACTER SET utf8mb4 COLLATE utf8mb4_general_ci;
```

4. Create UTF-8 secret files in ignored `.local-tests/secrets/`: `mysql-password.txt` for the database password, `admin-password.txt` for the initial administrator password, and `jwt-secret.txt` for a random JWT secret of at least 32 bytes. Administrator passwords require 8–72 visible ASCII characters, including uppercase, lowercase, digits and symbols.
5. Configure and initialize. Adjust addresses, ports and the database user for your services. This example assumes Redis is accessible on its default port; set `APP_REDIS_PASSWORD_FILE` if authentication is required:

```powershell
$env:APP_ENV = "dev"
$env:APP_SCOPE_ID = "my-development"
$env:APP_DATABASE_HOST = "127.0.0.1"
$env:APP_DATABASE_NAME = "ryframe_dev"
$env:APP_DATABASE_USERNAME = "ryframe_app"
$env:APP_DATABASE_PASSWORD_FILE = ".local-tests/secrets/mysql-password.txt"
$env:APP_AUTH_JWT_SECRET_FILE = ".local-tests/secrets/jwt-secret.txt"
$env:RYFRAME_INIT_ADMIN_PASSWORD_FILE = ".local-tests/secrets/admin-password.txt"
cargo migrate -- control up
cargo migrate -- control verify
cargo serve
```

`up` initializes or upgrades the selected control database; `verify` only validates. Initialize before starting: API and Worker do not automatically upgrade databases. The default API port is 8080. Run `cargo worker` in another terminal with the same configuration when background jobs are needed. Install and start the frontend separately, following its README.

The initial ordinary account `user`, newly created users and imported ordinary users start with `123456`; administrators use the explicitly configured password. Every new account must change its password before business access, then sign in again. Repeated migration does not reset existing passwords or the change requirement.

New configurations default to single tenancy, using the internal system tenant without a login tenant selector. Enable multiple tenants with `multi_tenancy.enabled = true` or `APP_MULTI_TENANCY_ENABLED=true`. Existing explicit settings remain effective. Configuration transfer and data maintenance remain available in single tenancy.

## Databases, configuration and SQL import

Configuration merges the base file, environment file and environment overrides. Select the environment with `APP_ENV`; supported secrets accept a corresponding `_FILE`. The [data guide](docs/data.md#租户路由) explains primary, replicas, sources, shared control storage, shared business storage, dedicated tenant databases, TLS, pool limits and array replacement.

Migration is the recommended initialization path. To import through a database tool, export a complete script:

```powershell
cargo migrate -- control export-sql --output .local-tests/control-init.sql --write
```

Choose an empty database and import with stop-on-error behavior as described in the [data guide](docs/data.md#控制库初始化-sql), then run `cargo migrate -- control verify`. The deployment export includes password hashes, deferred constraints, seeds and the migration ledger. `sql/ryframe_config.sql` is a source review snapshot, not a deployment script. Interrupted imports are not automatically replayed or cleared.

Create and register empty tenant business databases before running `cargo migrate -- tenant-data up --target <target-key>` and `verify`. Use `cargo reset -- <arguments>` for controlled nonproduction rebuilding; see the [operations guide](docs/operations.md).

## Develop business modules

`new-business` creates the business crate and SDK/process registration. `resource` generates CRUD, migrations and managed assets from handwritten Rust models. Both preview by default and write only with `--write`. Refresh the lockfile after creating a crate, then define and register models before generation as explained in the [development guide](docs/development.md#创建业务-crate):

Tenant business models use `database="tenant"`, table names starting with `biz_`, and a primary key containing `tenant_id`. Global models without `tenant_id` use `database="control"`, allow safe table identifiers such as `table1`, and are restricted to the system tenant by default. Enabling multi tenancy does not share global tables automatically; shared reads require explicit business extensions.

```powershell
cargo generate -- new-business order-business --write
cargo generate-lockfile --offline
cargo generate -- resource --package order-business
cargo generate -- resource --package order-business --model Order --write
```

Keep handwritten code in `resources/` and `extensions/`. The default generator is offline. With `schema-import`, `import --tables biz_order,biz_order_item` preflights the whole batch, registers model descriptors and preserves original schemas in `.ryframe/imports/` without modifying the database. Model fields use `sql_type` to retain supported physical types, lengths and datetime precision, checked against Rust types and capacity constraints before generation. Tables and string columns can declare their own UTF-8 collations, retained for schema verification after import. Configure tool defaults and the frontend directory in `config/tools.toml`; ignored `.local-tests/tools.toml` supplies local overrides, and CLI options take precedence. `cargo generate -- run` consumes configured `tables` or `models`. The Rust `GenerationBuilder` accepts `.include_tables(["biz_order", "biz_order_item"])`; edit and run the [example](crates/ryframe-generator/examples/generate.rs). See the development guide for the complete workflow and the [data guide](docs/data.md) for explicit adoption plans, execution and failure inspection of existing tables.

Exact Decimal fields must declare `sql_type="DECIMAL(p,s)"` explicitly. The current native type supports the complete domain for 1≤p≤28 and 0≤s≤p. API payloads and forms use decimal strings; values exceeding the declared capacity are rejected. See the development guide for models and defaults.

## Checks, contracts and builds

Daily backend checks do not require the frontend. Explicitly choose `all` for complete cross-repository checks. Update tool configuration when renaming or relocating the frontend:

```powershell
cargo xtask check
cargo xtask check --scope all --full
cargo xtask generate api --write
cargo xtask build --plan
cargo xtask build
```

Contract synchronization exports the current backend candidate OpenAPI and atomically updates frontend derivatives. Formal source binding has additional verification requirements. Builds compare source fingerprints before and after execution; `--plan` does not execute tasks. Recovery, browser and performance acceptance still require evidence for the current source. Historical measurements do not establish readiness of the modified version, and this candidate has not been declared ready for a stable release.

## Documentation

- [Architecture and extension points](docs/architecture.md): module boundaries, SDK and assembly.
- [Development](docs/development.md): toolchain, startup, models and generation.
- [API](docs/api.md): sessions, authentication, errors and contracts.
- [Data](docs/data.md): initialization, multiple databases, tenant routing, copying and recovery.
- [Operations](docs/operations.md): deployment, upgrades, reset and troubleshooting.

OpenAPI, the access catalog, configuration types and command help define fields, menus, permissions and defaults. Optional MQTT, email and optimistic-locking plugins are future work; this iteration adds no placeholder interfaces or extra service requirements.
