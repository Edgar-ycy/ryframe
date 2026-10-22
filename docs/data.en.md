[简体中文](data.md)

# Data

## Initializing a New Environment

First create an isolated empty MySQL database, then select `dev`, `test`, or `prod` through `APP_ENV`. `config/app.toml` supplies common defaults, and the selected `config/app.<environment>.toml` must exist. Set database passwords, JWT and `[bootstrap] admin_password` directly in plaintext configuration; see [README](../README.en.md#first-run-after-downloading). Use `--config-dir <directory>` to select another directory; application-relative paths resolve from the current working directory. Default single tenancy still has separate control and `shared-control` ledgers. Run the following in order from the backend root: When continuing with the ignored directory from README, add `--config-dir .local-tests/dev-config` after `--` in this guide's Cargo alias examples that otherwise use default `config`, for example `cargo migrate -- --config-dir .local-tests/dev-config control verify` or `cargo serve -- --config-dir .local-tests/dev-config`. Keep selecting the same directory for verification, export and runtime commands to avoid connecting through a different, uninitialized configuration.

```powershell
cargo migrate -- control up
cargo migrate -- control verify
cargo migrate -- tenant-data up --target shared-control
cargo migrate -- tenant-data verify --target shared-control
cargo serve
```

`control up` is the only entry point for creating or upgrading the control database. `control verify` only validates the ledger, structure, and compile-time business catalog; it cannot replace initialization on a new empty database. First-time initialization requires `[bootstrap] admin_password` in configuration. The API and Worker only validate at startup and do not execute DDL. Non-production configurations default to `jobs.mode="embedded"`, so `cargo serve` already consumes background tasks. To validate separate processes, follow the [development guide](development.en.md#starting-the-backend) to create private API and Worker configurations, both using `jobs.mode="external"`, with different instance IDs and explicitly selected configuration directories. For every MySQL tenant target, first create an empty database, register its configuration, and run `tenant-data up/verify` separately.

## Control Database Initialization SQL

```powershell
cargo migrate -- control export-sql --output .local-tests/control-init.sql --plan
cargo migrate -- control export-sql --output .local-tests/control-init.sql --write
```

`control export-sql --plan` reads no passwords, application configuration or database and writes no files. Actual export with `--write` loads file configuration selected by `APP_ENV` and `--config-dir <directory>` and reads `[bootstrap] admin_password` to generate hashes, without connecting to a database. The actual export contains the control database structure, post-creation constraints, system data, registered control database business migrations, and the ledger, without plaintext passwords. Handwritten business migrations must provide schema and initialization SQL from the same source as `up`. Select an empty database and configure the client to stop on errors; do not use `--force`. In PowerShell, use:

```powershell
mysql --default-character-set=utf8mb4 -h 127.0.0.1 -P 3306 -u ryframe_app -p ryframe_dev --execute="source .local-tests/control-init.sql"
```

bash can use `< .local-tests/control-init.sql`. PowerShell does not support this redirection, so do not use the bash command unchanged in PowerShell. After importing, run:

```powershell
cargo migrate -- control verify
cargo migrate -- tenant-data up --target shared-control
cargo migrate -- tenant-data verify --target shared-control
```

This SQL initializes only the control database. `sql/ryframe_config.sql` is a review snapshot containing placeholder hashes and no ledger; it cannot be deployed. After an interrupted import, do not guess how to complete it by repeating `up` or automatically delete objects. Confirm their purpose, then create another isolated database. MySQL DDL cannot rely on an outer transaction to roll back the entire operation.

## Data Boundaries and Migrations

The control database stores identity, authorization, products, the tenant directory, tasks, file metadata, and placement. By default, control database business data without `tenant_id` is accessible only to the system tenant. Tenant business data is routed through placement to `shared-control`, a shared business target, or a dedicated tenant target. The control database and tenant data plane use different ledgers. The compile-time catalog records tables, columns, cursors, and schemas. Startup, `verify`, and the generator fail closed when they find missing items, drift, unknown ledgers, or ownership conflicts; they do not automatically create tables or rewrite authorization.

The current `0.x` versions support only new isolated databases. Old structures or invalid capability codes are not converted automatically. After `v1.0.0`, frozen migrations permit only additions and roll-forward. Migration accounts need access to business table metadata. DDL may commit implicitly, and ledger transactions do not guarantee rollback of the entire migration. If validation fails after execution, preserve the state; do not assume rollback or replay automatically. Common checks are:

```powershell
cargo migrate -- control status
cargo migrate -- control verify
cargo migrate -- tenant-data status --all
cargo migrate -- tenant-data verify --all
```

Framework maintainers can use `cargo xtask data migrate check` to validate the migration catalog, registration order, generated migration wiring, and digests in `catalog/migrations.lock.toml` without writing. Use `cargo xtask data migrate check --require-frozen` for pre-commit checks. Explicit baseline consolidation in a development version uses `cargo xtask data migrate baseline --write`. Stable versions freeze only new migrations in the current working tree, using `cargo xtask data migrate freeze`. `cargo xtask data migrate reconcile --write` selects baseline or freeze according to the current version phase, and the full-stack fixture reuses it. These commands do not connect to or modify databases.

## Adopting Existing Tables

Existing tables can be adopted only explicitly. First import or handwrite the model, register the descriptor, and generate a matching migration. Stop the API/Worker, then save, inspect, and apply the plan:

```powershell
cargo migrate -- control adopt --module order --output .local-tests/order-adopt.json --write
cargo migrate -- control adopt --inspect .local-tests/order-adopt.json
cargo migrate -- control adopt --apply .local-tests/order-adopt.json --write
cargo migrate -- control verify
```

For tenant tables, replace `control` with `tenant-data` and explicitly provide `--target business-shared`; `--all` is not accepted. `--output` must be used with `--write`, while `--inspect` prohibits writes. Plans may use only new paths within `.local-tests`. `adopt --plan` does not read application configuration, connect to databases, or write; it differs from `adopt --output ... --write`, which reads actual structure and saves an adoption plan. Adoption only registers the existing structure; it does not execute historical DDL or change business rows. For partial registration, structural drift, or unknown outcomes, inspect the original plan first rather than replaying with a different plan. When the target is empty, create tables through normal `up` execution.

Authorization, task claiming, export requests, deletion acceptance, and configuration transfers use the primary database and explicit transactions. Eventual consistency is used only for list reads that can tolerate stale data; pre-execution validation, downloads, and state transitions use strong consistency. Tenant data scope is driven by `sys_tenant.data_scope_version`: a control database transaction writes to the outbox, and the Worker publishes and atomically switches the projection at the placement target. If the authenticated principal's version is behind or the projection is unavailable, the operation fails closed. There is no distributed transaction between the two databases, and the control database cannot roll back writes that have already entered the tenant database concurrently.

## Backup, Recovery, and Retention

Stop the API, Worker, scheduler, and message producers before a formal backup. The inventory uses `--quiesced-at` to indicate that the external system has been quiesced. `--observed-at` records only the start of observation and does not establish eligibility for a formal backup. Backup registration validates file sizes and SHA-256. Recovery data validation compares the current schema, complete table digests, placement, ownership, and object manifests; resources and registration metadata are excluded from business digests. An isolated recovery must use a new scope, databases, and object prefix while retaining target ownership. Success is registered only after data validation, API/Worker readiness, and real business proof pass. The reference objectives are a 24-hour recovery point and a 60-minute RTO; actual results cannot be inferred from a plan or a single inventory. The parent directories of the following outputs must already exist, and the target files must not exist. For detailed recovery commands, see the [Operations Guide](operations.en.md#failed-or-expired-backups).

```powershell
cargo xtask data backup inventory --output .local-tests/backup/inventory.json --source-sha <40-character-sha> --quiesced-at <RFC3339>
cargo xtask data target inventory --target <target-key> --output .local-tests/backup/target-inventory.json
```

For development copying, first freeze a plan with `cargo xtask check recovery clone plan --input <manifest> --output <new-plan> --write`, then perform a read-only recheck with `cargo xtask check recovery clone verify --plan <plan>`. Both entry points execute natively in Rust, without starting Python or accessing databases, networks, or shared services. They strictly validate input object fields, the 16 MiB limit, schemas and the business catalog, SQL data dumps, object sizes and SHA-256, tenant placement/fences, task relationships, and deterministic plan hashes. `bridge --build <receipt> --inventory <manifest> --output <new-bridge-file> --write` registers the reuse boundary only after Rust validates the explicit build receipt, complete source snapshot, file set, artifact bytes, build context, and current product inputs. It does not compile, modify old receipts, or establish recovery eligibility; the output must use create-new. `maintenance --operation build --output <new-directory> --write` uses the unified process layer and Cargo offline mode to build the three maintenance CLIs `reset`, `migrate`, and `tenant-data` individually, freezing the binaries, Cargo JSON events, logs, source, toolchain, and target receipts. Failures leave `failed.json` and original logs; successful receipts do not conceal partial writes. `maintenance --operation verify --output <directory-or-build.json>` rechecks frozen receipts, current source, and the toolchain without writing. Rebuilding into an existing output is rejected before Cargo starts. `storage` is also handled by Rust: fixed RustFS requests, data directory identity, original startup and actual process generations, listening ports, service logs, failed and unknown startup receipts, service identity changes, and precise process cleanup are validated at native entry points. `storage status` does not create attempts or evidence files; other operations require explicit `--write`. The successor and previous-tool replacement paths for `cache` have not been fully migrated, so this version continues to use the original state machine and does not retain a partially completed Rust entry point.

For `source_to_seed`, Rust handles run directory records, kernel mutual exclusion, controller identity, monotonic attempts, final source checks, and result publication for `init`, read-only `status`, and `stage`; `init/status` do not start Python. `stage export` has also moved to Rust. Preflight reads the fixed source request, environment, build, runtime, and producer receipts. Execution performs read-only validation of MySQL/object ownership, migrations, complete schemas, inventories before and after execution, SQL dumps, object bytes, scheduling relationships, and generation/storage drift. All local evidence is published with create-new inside `.local-tests`. `stage target-verify` uses Rust to orchestrate complete read-only database, object, and Redis rechecks, protecting initialization files, configuration, and actual process identity before and after execution. Complete history and recovery lineage are still validated through a private read-only Python protocol. Raw MySQL output retains the same file handle; a success receipt is first written completely and synchronized to a temporary file, then published exclusively. `copy` continues to use a single private Python subprocess that produces only the business results for the current stage and does not hold a second lifecycle state machine. `seed_to_arm` still requires deep validation of the target file tree, external storage generations, and published sources, so its `init/status/stage` continue to use the original Python state machine as a whole, without leaving a partial migration in Rust. Run directories, evidence, and outputs must reside within the current backend's `.local-tests`. Duplicate fields, links, paths outside the boundary, unknown writes, overlapping sources and targets, and unregistered resources are rejected. Plans, bridges, and registry directories are written only with create-new; status receipts are published atomically. The remaining `runtime`, `recover`, post-copy, seed, and cache stages continue to use existing private protocols. After an interruption, first use read-only `clone status` or `storage status` and complete before/after images to determine the next action. Only `reconcile`, `resume`, or `recover` bound to the creation identity may be executed according to the original ledger; unknown writes must not be replayed. API, Worker, Redis, and storage restarts separately register configuration, credential references, ports, process identity, and data directories. After a source generation start/stop failure, check `source-generation-status` first. Ordinary recovery cleans up only registered processes of the same generation and does not retroactively attest historical before images or zero data drift.

Export tasks freeze canonical filters, the authorization fingerprint, `snapshot_at`, and `upper_id`. Specified IDs remain subject to tenant and data permissions. Default limits are 500,000 rows, 1000 rows per batch, 1800 seconds, 512 MiB, and two running tasks per tenant. Deletion first writes `delete_pending_at`, then idempotently removes objects and records. A missing object is treated as success; a storage failure retains the tombstone for retry. Each deployment uses a stable, unique `scope_id`, and Redis, objects, and databases all validate ownership. `FLUSHDB`, `KEYS`, broad bucket deletion, and scanning shared servers are prohibited.

## File Configuration and Credentials

`APP_ENV` is the only project configuration environment entry and accepts only `dev`, `test`, or `prod`. API, Worker, migration and reset read optional common `app.toml` from the directory selected by `--config-dir <directory>`, then recursively merge the mandatory `app.<environment>.toml`. If the common file is genuinely absent, the environment file must supply every required field; an existing malformed, unreadable or dangling-link common file still fails. The default directory is `config`, and relative paths resolve from the current working directory. Production connection, secret strength, TLS, instance ID and resource isolation validation remain in force. Localization uses TOML field `app.locales_dir`, defaulting to `locales`, resolved from the process current working directory for ordinary applications; `APP_LOCALES_DIR` is not read. Missing resources or loading failures fail strictly in production, while development can use an embedded fallback.

Set database, Redis, object storage, JWT, monitoring token and initial administrator credentials directly in their plaintext fields. Keep real configuration only in ignored local directories, excluding it from logs, public receipts and version control. The root `snowflake_worker_id` must differ for each instance in the same deployment; `[bootstrap] admin_password` explicitly supplies the initial administrator password. The initial ordinary user `user`, newly created ordinary users and imported ordinary users still use `123456` and must change it on first login. Arrays are replaced as a whole in environment files: `cors.allow_origins` and `proxy.trusted_cidrs` use TOML string arrays; `database.replicas`, `database.sources` and `tenant_data.targets` use complete TOML table arrays, without patching individual entries.

The generator's `--config-dir <directory>` and Builder `.configuration_directory(path)` resolve relative paths from the backend Workspace root; running from a business crate's `src` does not change the `workspace/config` default. Priority is explicit CLI/Builder, local `.local-tests/tools.toml`, common `config/tools.toml`, then `workspace/config`; `[generator] config_dir` in tool files is optional. Shared MySQL tests read fixed `app.toml` and `app.test.toml` files under the backend root at `.local-tests/mysql/config`; all connection and TLS parameters come from `database.primary`, and Disabled is rejected. A missing directory only skips tests without acceptance; existing invalid or linked paths, missing files and invalid configuration fail. File `scope_id` participates in unique schema names; see [integration verification](development.en.md#integration-verification) for the opt-in and evidence boundaries. Real protocol acceptance is pending. Structure checks and CI tool tests for deployment templates and private configuration directories have passed. Real containers, images and the complete flow remain unaccepted; partial tests do not replace a complete gate.

## Tenant Routing

`database.primary` is the control database's sole writable database and the source for strongly consistent reads and `shared-control`. `database.replicas` contains read-only replicas of that same control database. `database.sources` contains named MySQL sources explicitly selected by business code. None of these replaces tenant placement. The following is a complete connection topology. Replace password placeholders, and note that certificate paths are resolved according to the platform of the running process:

```toml
scope_id = "orders-production"
snowflake_worker_id = 1

[multi_tenancy]
enabled = true

[database]
migration_mode = "verify"

[database.primary]
host = "mysql-control.internal"
port = 3306
database = "ryframe_control"
username = "ryframe_app"
password = "replace-with-primary-password"
max_connections = 20
min_connections = 2
tls_mode = "verify_identity"
tls_ca = "/run/secrets/mysql-ca.pem"

[[database.replicas]]
name = "control-read-east"
host = "mysql-control-read.internal"
port = 3306
database = "ryframe_control"
username = "ryframe_reader"
password = "replace-with-replica-password"
max_connections = 8
min_connections = 1
tls_mode = "verify_identity"
tls_ca = "/run/secrets/mysql-ca.pem"

[[database.sources]]
name = "legacy"
host = "mysql-legacy.internal"
port = 3306
database = "legacy_orders"
username = "legacy_reader"
password = "replace-with-source-password"
max_connections = 4
min_connections = 0
tls_mode = "verify_identity"
tls_ca = "/run/secrets/mysql-ca.pem"

[tenant_data]
default_target = "business-shared"
max_open_targets = 4
max_total_connections = 24
idle_pool_secs = 300

[[tenant_data.targets]]
key = "business-shared"
kind = "mysql"
mode = "shared"
host = "mysql-business.internal"
port = 3306
database = "ryframe_business"
username = "business_app"
password = "replace-with-shared-target-password"
max_connections = 8
tls_mode = "verify_identity"
tls_ca = "/run/secrets/mysql-ca.pem"

[[tenant_data.targets]]
key = "tenant-acme"
kind = "mysql"
mode = "dedicated"
host = "mysql-acme.internal"
port = 3306
database = "ryframe_tenant_acme"
username = "acme_app"
password = "replace-with-dedicated-target-password"
max_connections = 6
tls_mode = "verify_identity"
tls_ca = "/run/secrets/mysql-ca.pem"
```

Primary, replicas, sources and MySQL tenant targets all use the `password` field directly. Tenant target passwords may be empty, but the field must exist; DSNs are not accepted. Connection names must be unique, sources cannot use `primary`, and tenant targets must use schemas different from those of other control databases, sources and targets. Remote production connections and tenant targets must use `verify_identity` with a configured CA. `verify_ca` also requires a CA, and mTLS certificates and private keys must be configured as a pair.

Replicas serve only explicit `ReadConsistency::Eventual` reads. Healthy replicas are selected round-robin, falling back to primary when all are unavailable. `Strong`, read-after-write, authorization, tasks, exports, and state transitions use primary. RyFrame does not establish MySQL primary/replica replication or wait for replicas to catch up; operations staff are responsible for lag monitoring and failover. `database.sources` is accessed through `control_database.source("<name>")`, which returns `Option`. Each source has an independent connection pool and transactions; databases are not sharded automatically, and there are no distributed transactions across databases. A schema source used by the generator does not automatically become a runtime target.

| Layout | placement target | Boundary |
| --- | --- | --- |
| Shared control database | Implicit `shared-control`, `kind="control"`, `mode="shared"` | Default single-tenant mode and the system tenant; reuses the primary pool, while tenant business data still has an independent migration ledger |
| Shared business database | `kind="mysql"`, `mode="shared"` | Multiple tenants share `biz_*` tables; queries remain isolated by `tenant_id` |
| Dedicated business database | `kind="mysql"`, `mode="dedicated"` | One active tenant at a time, with separate accounts, capacity, and backups available |

New configurations default to `multi_tenancy.enabled=false`. Tenant selection and management are exposed only when explicitly set to `true` in the configuration file. `tenant_data.targets[].key` is the stable identity used by migrations, health checks, the API, and placement; a database name cannot replace it. The generator's `source="local"` is shorthand: when `database=control`, it reads primary; when the storage scope is tenant, it reads `tenant_data.default_target`. That key must therefore refer to a MySQL target, and implicit `shared-control` cannot be used as a local schema source. Specify `primary` or `tenant_data.targets.<key>` directly to remove ambiguity. Provisioning a new tenant still explicitly submits `data_target_key`, and runtime routing uses only placement in the control database. Missing placement or an unknown target causes failure closed.

When evolving from single-tenant `shared-control`, first add a shared or dedicated target to the configuration, create an empty database, set its target `password` field, run `cargo migrate -- tenant-data up --target <target-key>` and `verify`, and complete a live probe in “Platform Management → Data Targets.” Select the target key when provisioning a new tenant. Existing non-system tenants use the tenant data placement/migration flow: first preview and validate the placement generation, plan hash, and idempotency key, then copy the data and switch placement. Changing `default_target`, DNS, or copying a schema does not migrate existing data. The system tenant cannot migrate business data, and an occupied dedicated target must first be migrated or cleaned up.

Dedicated target pools are created on demand. The default limit is `max_total_connections / max_open_targets`, overridden by the target's `max_connections`; the API and Worker count separately. `tenant-data up/verify/status` uses target keys, and `--all` processes only registered targets. For multiple targets, check physical instances and schemas first. Duplicate aliases are rejected before DDL. Check status after a failure; completed targets are not replayed automatically.
