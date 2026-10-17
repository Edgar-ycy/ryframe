# RyFrame

[简体中文](README.md)

RyFrame is a Rust 2024 framework for enterprise administration, paired with a Vue 3 frontend. This is a `0.x` development version: use current contracts and a new isolated database rather than overwriting an older development installation.

It includes users, roles, permissions, organizations, product plans, tenancy, configuration transfer, database migration, monitoring, messages, scheduling, imports, exports, profiles, passwords, avatars and login sessions. Plans may have no optional capabilities. Notices and tenant message publishing can be enabled per plan; disabling them preserves history and does not disable system notifications, scheduling, imports, exports, backups or recovery.

The business runtime is Rust. API, Worker, migration, reset and generation have direct commands; `cargo xtask` maintains the framework itself.

## First run after downloading

Backend and frontend are separate repositories. Run these commands from the backend root. Examples use Windows PowerShell; MySQL, Redis and RustFS/MinIO/S3-compatible storage may run on any suitable platform or remote server. WSL is not required.

1. Install rustup and run `rustup show` in the repository to select the stable toolchain, rustfmt and Clippy pinned by `rust-toolchain.toml`.
2. Prepare MySQL 8.0.16 or newer (MariaDB is not a substitute) and an account with the required schema, migration and application permissions. Redis uses a RESP connection; Redis 7.x is the current acceptance baseline (CI uses 7.4.9), with AUTH, an isolated database index and TLS configured as required by the deployment. File storage defaults to a local directory and also supports RustFS, MinIO and S3-compatible services.
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
$env:APP_OBJECT_STORAGE_BACKEND = "local"
$env:APP_OBJECT_STORAGE_LOCAL_BASE_DIR = ".local-tests/uploads"
cargo migrate -- control up
cargo migrate -- control verify
cargo migrate -- tenant-data up --target shared-control
cargo migrate -- tenant-data verify --target shared-control
cargo serve
```

`up` initializes or upgrades the selected control database; `verify` only validates. On a new empty database, running `control verify` first reports an incomplete ledger or schema; it cannot replace first-time initialization. The control database and the `shared-control` tenant data plane have separate ledgers, so the default single-tenant setup also requires both initialization and verification commands. API and Worker do not execute DDL at startup. The default API port is 8080. In nonproduction environments, `jobs.mode="embedded"` is the default and `cargo serve` consumes background jobs in the same process. To run a separate Worker, set `$env:APP_JOBS_MODE = "external"` in both terminals, then run `cargo serve` and `cargo worker` separately. The overrides above switch the development configuration from RustFS to `.local-tests/uploads`; for RustFS, MinIO or S3, set the corresponding backend, HTTPS endpoint, credentials and region, then confirm the service protocol and connectivity. If the isolated development MySQL server does not enable TLS, set `$env:APP_DATABASE_TLS_MODE = "disabled"` explicitly; remote production must use `verify_identity`. Install and start the frontend separately, following its README.

The initial ordinary account `user`, newly created users and imported ordinary users start with `123456`; administrators use the explicitly configured password. Every new account must change its password before business access, then sign in again. Repeated migration does not reset existing passwords or the change requirement. For an existing development account with an unknown password, use the [explicit recovery flow](docs/api.md#异常账号恢复): an administrator issues a one-time link and the user chooses a new password.

New configurations default to single tenancy, using the internal system tenant without a login tenant selector. Enable multiple tenants with `multi_tenancy.enabled = true` or `APP_MULTI_TENANCY_ENABLED=true`. Existing explicit settings remain effective. Configuration transfer and data maintenance remain available in single tenancy.

## Databases, configuration and SQL import

Configuration applies `app.toml`, `app.<environment>.toml` and `APP_*` environment overrides in that order. Set the exact value `APP_ENV=dev|test|prod`; `APP_CONFIG_DIR` can select another configuration directory. Supported secrets accept a corresponding `_FILE`, and the direct variable cannot be set together with its `_FILE`. Arrays are replaced as a whole at every layer and cannot be patched item by item. The [data guide](docs/data.md#租户路由) explains primary, replicas, sources, shared control storage, shared business storage, dedicated tenant databases, placement, TLS, pool limits and a complete TOML example.

Migration is the recommended initialization path. To import through a database tool, export a complete script:

```powershell
cargo migrate -- control export-sql --output .local-tests/control-init.sql --write
```

Choose an empty database and import with stop-on-error behavior as described in the [data guide](docs/data.md#控制库初始化-sql), then run `cargo migrate -- control verify`. The deployment export includes password hashes, deferred constraints, seeds and the migration ledger. `sql/ryframe_config.sql` is a source review snapshot, not a deployment script. Interrupted imports are not automatically replayed or cleared, and `control up` is not used to guess how to repair them. This SQL initializes the control database only; tenant business targets still require `tenant-data up` and `verify`.

Create and register empty tenant business databases before running `cargo migrate -- tenant-data up --target <target-key>` and `verify`. `--target` takes the configured target key, not the database name; `--all` processes registered targets only. Use `cargo reset -- <arguments>` for controlled nonproduction rebuilding after stopping API, Worker and schedulers; see the [operations guide](docs/operations.md).

## Develop business modules

`new-business` creates the business crate and SDK/process registration. `resource` generates CRUD, migrations and managed assets from handwritten Rust models. Both preview by default and write only with `--write`. The crate, registration and lockfile are installed together without upgrading existing locked dependencies. Define and register models before generation as explained in the [development guide](docs/development.md#创建业务-crate):

Tenant business models use `database="tenant"`, table names starting with `biz_`, and a primary key containing `tenant_id`. Global models without `tenant_id` use `database="control"`, allow safe table identifiers such as `table1`, and are restricted to the system tenant by default. Enabling multi tenancy does not share global tables automatically; shared reads require explicit business extensions. After generation, review and run `cargo migrate -- control up` to initialize business menus and permissions, then run `control verify`. New tenant administrators receive tenant business CRUD permissions by default; ordinary users and extension operations require explicit grants.

```powershell
cargo generate -- new-business order-business --write
cargo generate -- resource --package order-business
cargo generate -- resource --package order-business --model Order --write
```

Keep handwritten code in `resources/` and `extensions/`. The default generator is offline; enable `schema-import` for an existing database:

```powershell
cargo generate --features schema-import -- import --package order-business --connection primary --database tenant --tables biz_order,biz_order_item
```

`--connection` selects only the read-only schema source and accepts `primary`, `sources.<name>` or `tenant_data.targets.<target-key>`. `--database` independently selects the generated models' runtime scope; the two settings are not coupled. Import generates models, CRUD and migrations as one batch, preserves original schemas in `.ryframe/imports/`, and changes neither the database nor ownership of existing tables. `owner_field` names an `i64` or `Option<i64>` field containing a user ID; generated creation writes the current user, and list, detail, update and delete use the same data scope. Enable `generator.sync_frontend` or `--sync-frontend` to generate the current candidate OpenAPI, typed callers, lists, forms and routes; type and lint checks finish before both repositories are installed together. Preview writes only isolated caches; `--no-sync-frontend` temporarily disables synchronization. Unsupported structures reject the whole batch. Field `sql_type` preserves supported physical types, capacities and datetime precision; table and column collations participate in verification. Configure tool defaults and the frontend directory in `config/tools.toml`; ignored `.local-tests/tools.toml` supplies local overrides, and CLI options take precedence. `cargo generate -- run` consumes configured `tables` or `models`. The Rust `GenerationBuilder` accepts `.include_tables(["biz_order", "biz_order_item"])`; edit and run the [example](crates/ryframe-generator/examples/generate.rs). See the development guide for the complete workflow and the [data guide](docs/data.md#已有表接管) for explicit adoption plans, execution and failure inspection of existing tables.

Exact Decimal fields must declare `sql_type="DECIMAL(p,s)"` explicitly. The current native type supports the complete domain for 1≤p≤28 and 0≤s≤p. API payloads and forms use decimal strings; values exceeding the declared capacity are rejected. See the development guide for models and defaults.

Generated forms use text or textarea, integer, switch, date, date-time, enumerated radio and exact Decimal text controls according to the field type. JSON uses an editable textarea that is parsed strictly before submission. JSON filters are rejected in standard query parameters; use a strongly typed extension for structured conditions.

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
