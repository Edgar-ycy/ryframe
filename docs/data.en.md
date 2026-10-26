[简体中文](data.md)

# Data

## Initializing a New Environment

First create an isolated empty MySQL database, then select `dev`, `test`, or `prod` through `APP_ENV`. `config/app.toml` is optional, and the selected `config/app.<environment>.toml` must exist; without the common file, the environment file must supply every required setting. Set database passwords, JWT and `[bootstrap] admin_password` directly in plaintext configuration; see [README](../README.en.md#first-run-after-downloading). Use `--config-dir <directory>` to select another directory; application-relative paths resolve from the current working directory. Default single tenancy still has separate control and `shared-control` ledgers. Run the following in order from the backend root: When continuing with the ignored directory from README, add `--config-dir .local-tests/dev-config` after `--` in this guide's Cargo alias examples that otherwise use default `config`, for example `cargo migrate -- --config-dir .local-tests/dev-config control verify` or `cargo serve -- --config-dir .local-tests/dev-config`. Keep selecting the same directory for verification, export and runtime commands to avoid connecting through a different, uninitialized configuration.

Failures from `cargo migrate` go to stderr with the actual phase, selected configuration files, configuration keys, and a redacted cause; successful output keeps its existing contract. Connection or configuration failures precede migration execution. After entering `up` or `adopt --apply`, DDL or ledger changes may be partially committed; this does not imply an overall rollback. Preserve the evidence, fix the cause, and use the same `--config-dir` and `APP_ENV` for `status` and `verify` before deciding on writes; never replay automatically. Export failures also require checking any created directories, temporary files, or final file. Independent MySQL targets retain redacted connection, health, and schema causes for internal diagnostics; public API responses exclude these causes, and unavailability without an underlying error does not establish an authentication error. Every suggested command explicitly includes a directory placeholder for `--config-dir`. Replace it with the parent directory of the displayed configuration files and quote it for the current shell; double any single quote inside a PowerShell single-quoted path. Do not execute the placeholder or omit the directory and inspect a different default database.

```powershell
cargo migrate -- control up
cargo migrate -- control verify
cargo migrate -- tenant-data up --target shared-control
cargo migrate -- tenant-data verify --target shared-control
cargo serve
```

The migration path uses `control up` to create or upgrade the control database; a new empty physical database may instead use the complete SQL initialization below. `control verify` only validates the ledger, structure, and compile-time business catalog; it cannot replace initialization on a new empty database. First-time initialization requires `[bootstrap] admin_password` in configuration. The API and Worker only validate at startup and do not execute DDL. Non-production configurations default to `jobs.mode="embedded"`, so `cargo serve` already consumes background tasks. To validate separate processes, follow the [development guide](development.en.md#starting-the-backend) to create private API and Worker configurations, both using `jobs.mode="external"`, with different instance IDs and explicitly selected configuration directories. For every MySQL tenant target, first create an empty database, register its configuration, and run `tenant-data up/verify` separately.

## Control Database Initialization SQL

```powershell
cargo migrate -- control export-sql --output .local-tests/control-init.sql --plan
cargo migrate -- control export-sql --output .local-tests/control-init.sql --write
```

`control export-sql --plan` reads no passwords, application configuration or database and writes no files. Actual export with `--write` loads file configuration selected by `APP_ENV` and `--config-dir <directory>` and reads `[bootstrap] admin_password` to generate hashes, without connecting to a database. The actual export covers the control plane and `shared-control` tenant data plane in one physical database: control structures, post-creation constraints, system data, fence/slot/projection tables and initial rows, both framework migration ledgers, and registered Control and Tenant business initialization migrations with one business ledger, without plaintext passwords. Handwritten business migrations must provide schema and initialization SQL from the same source as `up`. The export contains password hashes and must be protected as credential material; the output must be a new file and cannot overwrite an existing one. Select an empty database and configure the client to stop on errors; do not use `--force`. The following example connects to a local database. Client host, port, user and database must match the selected configuration; for remote production connections, explicitly enable hostname verification with the matching CA instead of relying on client TLS defaults. In PowerShell, use:

```powershell
mysql --default-character-set=utf8mb4 -h 127.0.0.1 -P 3306 -u ryframe_app -p ryframe_dev --execute="source .local-tests/control-init.sql"
```

bash can use `< .local-tests/control-init.sql`. PowerShell does not support this redirection, so do not use the bash command unchanged in PowerShell. After importing, run:

```powershell
cargo migrate -- control verify
cargo migrate -- tenant-data verify --target shared-control
```

After successful import, run both verifications above directly without an additional `up`. Independent MySQL tenant targets are outside this physical-database export and still use their own `tenant-data up/verify`. `sql/ryframe_config.sql` is a review snapshot containing placeholder hashes and no ledger; it cannot be deployed. After an interrupted import, do not guess how to complete it by repeating `up` or automatically delete objects. Confirm their purpose, then create another isolated database. MySQL DDL cannot rely on an outer transaction to roll back the entire operation. Imports run actual metadata assertions before and after each migration, covering column order, defaults, collations, indexes, foreign keys, and `CHECK` expressions and `enforced`; only successful verification permits its business ledger entry. Final verification rechecks framework and business structures and ownership/ledgers, then records both framework completion markers in a transaction holding metadata locks. Previously completed migrations and DDL may remain after failure. Inspect the reported migration, phase, category and actual state before taking action; do not replay automatically.

## Data Boundaries and Migrations

The control database stores identity, authorization, products, the tenant directory, tasks, file metadata, and placement. By default, control database business data without `tenant_id` is accessible only to the system tenant. Tenant business data is routed through placement to `shared-control`, a shared business target, or a dedicated tenant target. The control database and tenant data plane use different ledgers. The compile-time catalog records tables, columns, cursors, and schemas. Startup, `verify`, and the generator fail closed when they find missing items, drift, unknown ledgers, or ownership conflicts; they do not automatically create tables or rewrite authorization. `shared-control` verification excludes only business tables formally registered as control tables by the composition root; `control verify` still validates their structure. They do not enter the tenant copy catalog or the dedicated MySQL target allowlist. Unknown or missing business tables are listed by name. If verification fails after `tenant-data up` has completed DDL, preserve the database and use read-only `control verify` and `tenant-data verify` to check ownership and ledgers; do not delete the database or automatically replay migrations.

The current `0.x` versions support only new isolated databases. Old structures or invalid capability codes are not converted automatically. After `v1.0.0`, frozen migrations permit only additions and roll-forward. Migration accounts need access to business table metadata. DDL may commit implicitly, and ledger transactions do not guarantee rollback of the entire migration. If validation fails after execution, preserve the state; do not assume rollback or replay automatically. Common checks are:

```powershell
cargo migrate -- control status
cargo migrate -- control verify
cargo migrate -- tenant-data status --all
cargo migrate -- tenant-data verify --all
```

Framework maintainers can use `cargo xtask data migrate check` to validate the migration catalog, registration order, generated migration wiring, and digests in `catalog/migrations.lock.toml` without writing. Use `cargo xtask data migrate check --require-frozen` for pre-commit checks. Explicit baseline consolidation in a development version uses `cargo xtask data migrate baseline --write`. Stable versions freeze only new migrations in the current working tree, using `cargo xtask data migrate freeze`. `cargo xtask data migrate reconcile --write` selects baseline or freeze according to the current version phase, and the full-stack fixture reuses it. These commands do not connect to or modify databases.

## Adopting Existing Tables

Existing tables require explicit adoption. If unregistered business tables are added to an initialized control database, startup rejection of unknown tables is expected. Preserve the tables and data; generating models and immediately running `serve` does not establish adoption. Stop producers including API/Worker, prepare registered modules with matching models and migrations, and use a current migration binary containing that registration. Adoption prechecks read-only verify the control framework ledger, complete physical schema and target boundary, without requiring new business permissions/menus to be seeded first. The adoption plan still checks business schema, physical identity, source digests and other ledger constraints at each operation. Saving a preview and inspecting do not write the database; apply registers only verified adopted ledger metadata without historical business DDL or business-row changes. Inspect the original plan after application. When new access entries or reviewed subsequent migrations are needed, explicitly run `up` for the corresponding scope, followed by full verification. The new-module example below uses `control up` to initialize access entries before verification; API/Worker startup still requires complete business permissions and menus, with no relaxed startup path:

```powershell
cargo migrate -- control adopt --module order --output .local-tests/order-adopt.json --write
cargo migrate -- control adopt --inspect .local-tests/order-adopt.json
cargo migrate -- control adopt --apply .local-tests/order-adopt.json --write
cargo migrate -- control adopt --inspect .local-tests/order-adopt.json
cargo migrate -- control up
cargo migrate -- control verify
```

For tenant tables, adoption, inspection, target migrations and verification use `tenant-data` with explicit `--target business-shared`; `--all` is not accepted. Business permissions/menus still require `control up` and `control verify` on the control database; migrating only the tenant target does not replace that step. `--output` must be used with `--write`, while `--inspect` prohibits writes. Plans may use only new paths within `.local-tests`. `adopt --plan` does not read application configuration, connect to databases, or write; it differs from `adopt --output ... --write`, which reads actual structure and saves an adoption plan. Adoption only registers the existing structure; it does not execute historical DDL or change business rows. For partial registration, structural drift, or unknown outcomes, inspect the original plan first rather than replaying with a different plan. When the target is empty, create tables through normal `up` execution.

Authorization, task claiming, export requests, deletion acceptance, and configuration transfers use the primary database and explicit transactions. Eventual consistency is used only for list reads that can tolerate stale data; pre-execution validation, downloads, and state transitions use strong consistency. Tenant data scope is driven by `sys_tenant.data_scope_version`: a control database transaction writes to the outbox, and the Worker publishes and atomically switches the projection at the placement target. If the authenticated principal's version is behind or the projection is unavailable, the operation fails closed. User creation, updates, deletion, and import batches that actually insert users advance the tenant authorization epoch in the same transaction that records the data-scope change, writing the existing mirror outbox when caching is enabled. After commit, the tenant authorization cache is synchronized to invalidate old snapshots for other subjects and rebuild them on the next request. Updates and deletion still advance authorization versions for target users and reject old tokens; import batches without inserts and rolled-back transactions do not advance the epoch. Mirror synchronization failures after commit follow the existing cache policy; committed writes are not replayed, and the persistent outbox provides recovery. There is no distributed transaction between the two databases, and the control database cannot roll back writes that have already entered the tenant database concurrently.

## Backup, Recovery, and Retention

Stop the API, Worker, scheduler, and message producers before a formal backup. The inventory uses `--quiesced-at` to indicate that the external system has been quiesced. `--observed-at` records only the start of observation and does not establish eligibility for a formal backup. Backup registration validates file sizes and SHA-256. Recovery data validation compares the current schema, complete table digests, placement, ownership, and object manifests; resources and registration metadata are excluded from business digests. An isolated recovery must use a new scope, databases, and object prefix while retaining target ownership. Success is registered only after data validation, API/Worker readiness, and real business proof pass. The reference objectives are a 24-hour recovery point and a 60-minute RTO; actual results cannot be inferred from a plan or a single inventory. The parent directories of the following outputs must already exist, and the target files must not exist. For detailed recovery commands, see the [Operations Guide](operations.en.md#failed-or-expired-backups).

```powershell
cargo xtask data backup inventory --output .local-tests/backup/inventory.json --source-sha <40-character-sha> --quiesced-at <RFC3339>
cargo xtask data target inventory --target <target-key> --output .local-tests/backup/target-inventory.json
```

For development copying, first freeze a plan with `cargo xtask check recovery clone plan --input <manifest> --output <new-plan> --write`, then perform a read-only recheck with `cargo xtask check recovery clone verify --plan <plan>`. Both entry points execute natively in Rust, without starting Python or accessing databases, networks, or shared services. They strictly validate input object fields, the 16 MiB limit, schemas and the business catalog, SQL data dumps, object sizes and SHA-256, tenant placement/fences, task relationships, and deterministic plan hashes. `bridge --build <receipt> --inventory <manifest> --output <new-bridge-file> --write` registers the reuse boundary only after Rust validates the explicit build receipt, complete source snapshot, file set, artifact bytes, build context, and current product inputs. It does not compile, modify old receipts, or establish recovery eligibility; the output must use create-new. `maintenance --operation build --output <new-directory> --write` uses the unified process layer and Cargo offline mode to build the three maintenance CLIs `reset`, `migrate`, and `tenant-data` individually, freezing the binaries, Cargo JSON events, logs, source, toolchain, and target receipts. Failures leave `failed.json` and original logs; successful receipts do not conceal partial writes. `maintenance --operation verify --output <directory-or-build.json>` rechecks frozen receipts, current source, and the toolchain without writing. Rebuilding into an existing output is rejected before Cargo starts. `storage` is also handled by Rust: fixed RustFS requests, data directory identity, original startup and actual process generations, listening ports, service logs, failed and unknown startup receipts, service identity changes, and precise process cleanup are validated at native entry points. `storage status` does not create attempts or evidence files; other operations require explicit `--write`. The successor and previous-tool replacement paths for `cache` have not been fully migrated, so this version continues to use the original state machine and does not retain a partially completed Rust entry point.

For `source_to_seed`, Rust handles run directory records, kernel mutual exclusion, controller identity, monotonic attempts, final source checks, and result publication for `init`, read-only `status`, and `stage`; `init/status` do not start Python. `stage export` has also moved to Rust. Preflight reads the fixed source request, environment, build, runtime, and producer receipts. Execution performs read-only validation of MySQL/object ownership, migrations, complete schemas, inventories before and after execution, SQL dumps, object bytes, scheduling relationships, and generation/storage drift. All local evidence is published with create-new inside `.local-tests`. `stage target-verify` uses Rust to orchestrate complete read-only database, object, and Redis rechecks, protecting initialization files, configuration, and actual process identity before and after execution. Complete history and recovery lineage are still validated through a private read-only Python protocol. Raw MySQL output retains the same file handle; a success receipt is first written completely and synchronized to a temporary file, then published exclusively. `copy` continues to use a single private Python subprocess that produces only the business results for the current stage and does not hold a second lifecycle state machine. `seed_to_arm` still requires deep validation of the target file tree, external storage generations, and published sources, so its `init/status/stage` continue to use the original Python state machine as a whole, without leaving a partial migration in Rust. Run directories, evidence, and outputs must reside within the current backend's `.local-tests`. Duplicate fields, links, paths outside the boundary, unknown writes, overlapping sources and targets, and unregistered resources are rejected. Plans, bridges, and registry directories are written only with create-new; status receipts are published atomically. The remaining `runtime`, `recover`, post-copy, seed, and cache stages continue to use existing private protocols. After an interruption, first use read-only `clone status` or `storage status` and complete before/after images to determine the next action. Only `reconcile`, `resume`, or `recover` bound to the creation identity may be executed according to the original ledger; unknown writes must not be replayed. API, Worker, Redis, and storage restarts separately register configuration, credential references, ports, process identity, and data directories. After a source generation start/stop failure, check `source-generation-status` first. Ordinary recovery cleans up only registered processes of the same generation and does not retroactively attest historical before images or zero data drift. The same bound Rust tool validates frozen source-generation requests, paired build equivalence, complete ordinary-recovery before images, and C69 receipt types without writes. Only the established normalization of environment digests and maintenance build-log content is allowed. A successful guard grants neither startup nor replay permission; failures preserve the original stage evidence. Private clone, fresh-target, seed source, and clone stage requests use a single bounded UTF-8 JSON standard-input message, without business arguments in argv or requests in the child environment. Long-running recovery stages gain no overall timeout, and interruption or exit does not trigger an automatic retry.

Rust validates migration initialization resume state and successful confirmation without writing resources. The Python controller submits the fixed request, prepare snapshot, and lock state through a private standard-input protocol. Validation binds the continuous migration prefix, complete stage file set, trusted service restart generation, Redis marker rebind, and the single permitted predecessor with a failed read-only operation. Failure preserves the original evidence and does not replay unknown writes. External runtime generation sources continue to use the independent trusted-source validator rather than embedded started values; native evidence is checked again after source validation.

Before reading credentials or starting business operations, native identity, quota, and department producers validate the latest passed seed registration and its original file digest within the same run directory. A failed or unfinished latest registration prevents fallback. Before startup authorization finishes, the fixed manifest, complete stage records, and registration result are checked again. Source drift stops execution and preserves the original evidence. Registration file validation and publication also run in the bound Rust tool. The outer controller retains the original run/target locks while the native entry verifies the current attempt, live controller, target lock identity, latest post-copy generation, and explicitly bound configuration files. An identical registration can be rechecked and reused; conflicting or partial files are never overwritten. A registration written before the outer stage finishes still grants no startup eligibility: preserve the evidence and inspect the stage and locks first. Existing orchestration continues to validate the complete copy history and resource guards for `prepare/start`; successful registration does not replace those checks. Identity producers recheck the fixed post-copy registration, plan, and live controller before execution, at identity publication boundaries, and at completion; quota and department protocols verify the same generation before and after each business operation. Truncated input, duplicate fields, oversized requests, or a broken output channel still close held sessions. Explicit logout does not require later sources to remain valid, and simultaneous business and cleanup failures never produce success.

Rust validates read-only commands, native S3 receipts, owner contents, MySQL output, and the closed guard artifact set for department plan adoption, publishing the manifest with create-new. Unregistered files, source drift, or partial evidence reject adoption and preserve the original state. Identity prerequisites reuse the same Rust ordered ledger projection, checking all 622 writes, 11 roles, and 200 users. Rust also performs read-only validation of the ten-tenant plan, complete department paths in bounded XLSX workbooks, directory and template before/after images, idempotent intents, individual reconciliations, unique confirmations, and identity authorization evidence. The existing stage selector still selects the latest trusted stage. Model requests use digest-bound managed temporary input files, rechecked after processing, without increasing the stdin limit or adding environment inputs. Session cleanup budgets cover ten administrators, ten template sessions, and pacing cleanup, so later source drift does not block logout. Existing locked stages still orchestrate remote department operations and failed-plan adoption; passing evidence validation grants no write, Worker, or recovery authorization.

Export tasks freeze canonical filters, the authorization fingerprint, `snapshot_at`, and `upper_id`. Specified IDs remain subject to tenant and data permissions. Default limits are 500,000 rows, 1000 rows per batch, 1800 seconds, 512 MiB, and two running tasks per tenant. Deletion first writes `delete_pending_at`, then idempotently removes objects and records. A missing object is treated as success; a storage failure retains the tombstone for retry. Each deployment uses a stable, unique `scope_id`, and Redis, objects, and databases all validate ownership. `FLUSHDB`, `KEYS`, broad bucket deletion, and scanning shared servers are prohibited.

## File Configuration and Credentials

`APP_ENV` is the only project configuration environment entry and accepts exactly `dev`, `test`, or `prod`, without case variants or whitespace. Ordinary API, Worker and migration processes default to `dev` when it is unset; reset requires it explicitly. Operational examples always select the environment explicitly. API, Worker, migration and reset read optional common `app.toml` from the directory selected by `--config-dir <directory>`, then recursively merge the mandatory `app.<environment>.toml`. If the common file is genuinely absent, the environment file must supply every required field; an existing malformed, unreadable or dangling-link common file still fails. The default directory is `config`, and relative paths resolve from the current working directory. Production connection, secret strength, TLS, instance ID and resource isolation validation remain in force. Localization uses TOML field `app.locales_dir`, defaulting to `locales`, resolved from the process current working directory for ordinary applications; `APP_LOCALES_DIR` is not read. Missing resources or loading failures fail strictly in production, while development can use an embedded fallback.

Set database, Redis, object storage, JWT, monitoring token and initial administrator credentials directly in their plaintext fields. Keep real configuration only in ignored local directories, excluding it from logs, public receipts and version control. The root `snowflake_worker_id` must differ for each instance in the same deployment; `[bootstrap] admin_password` explicitly supplies the initial administrator password. The initial ordinary user `user`, newly created ordinary users and imported ordinary users still use `123456` and must change it on first login. Arrays are replaced as a whole in environment files: `cors.allow_origins` and `proxy.trusted_cidrs` use TOML string arrays; `database.replicas`, `database.sources` and `tenant_data.targets` use complete TOML table arrays, without patching individual entries.

The generator's `--config-dir <directory>` and Builder `.configuration_directory(path)` resolve relative paths from the backend Workspace root; running from a business crate's `src` does not change the `workspace/config` default. Priority is explicit CLI/Builder, local `.local-tests/tools.toml`, common `config/tools.toml`, then `workspace/config`; `[generator] config_dir` in tool files is optional. Shared MySQL tests use the fixed backend-root directory `.local-tests/mysql/config`, where common `app.toml` is optional and `app.test.toml` is mandatory. All connection and TLS parameters come from `database.primary`, and Disabled is rejected. A missing directory only skips tests without acceptance; existing invalid or linked paths, a missing environment file and invalid configuration fail. File `scope_id` participates in unique schema names; see [integration verification](development.en.md#integration-verification) for the opt-in and evidence boundaries. Only cases actually run provide evidence for the corresponding service; configuration or gate entry points alone do not establish complete real-service acceptance. Structure checks and CI tool tests for deployment templates and private configuration directories have passed. Real containers, images and the complete flow remain unaccepted; partial tests do not replace a complete gate.

## Tenant Routing

`database.primary` is the control database's sole writable database and the source for strongly consistent reads and `shared-control`. `database.replicas` contains read-only replicas of that same control database. `database.sources` contains named MySQL sources explicitly selected by business code. None of these replaces tenant placement. The following database topology fragment must be merged into a complete application configuration; it is not a standalone configuration file. Replace password placeholders, and note that certificate paths are resolved according to the platform of the running process:

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

Primary, replicas, sources and MySQL tenant targets use the `password` field directly, without credential environment overrides or DSNs. Empty passwords must still be explicit; authentication depends on the server. Replica and source names must be unique without cross-category conflicts, and sources cannot use `primary`. A MySQL tenant target’s host, port and database tuple must not overlap the control database, replicas, sources or other targets. Different servers may use the same database name; names alone do not establish identity. A named source may explicitly alias primary, subject to tenant-target independence checks. TLS defaults to `required`, which requires encryption only. `verify_ca` validates the certificate chain, while `verify_identity` also checks the hostname; both require a CA. All such remote production connections and targets require `verify_identity`. mTLS certificates and private keys must be configured together. Isolated development may explicitly disable TLS, but shared real MySQL tests reject Disabled.

Replicas serve only explicit `ReadConsistency::Eventual` reads. Healthy replicas are selected round-robin, falling back to primary during pool selection when all are unavailable; failed SQL is not automatically replayed on primary. `Strong`, read-after-write, authorization, tasks, exports, and state transitions use primary. Startup does not wait for replicas: background reconnection and checks of health, framework ledger/schema, access catalog and currently registered control-business table structures precede routing; control-business schema drift removes a replica immediately, and recovery still requires two consecutive complete successful checks. Failure to connect primary or a configured named source blocks startup. RyFrame does not establish MySQL replication or wait for replicas to catch up; operations staff manage lag and failover. `database.sources` is accessed through `control_database.source("<name>")`, which returns `Option`, and does not participate in automatic read routing. Each source has its own connection pool and transactions; its commits and rollbacks are independent of primary. Databases are not sharded automatically, and there are no distributed transactions across databases. MySQL tenant targets have no replica configuration; even eventual reads use the target’s own pool. A schema source used by the generator does not automatically become a runtime target.

| Layout | placement target | Boundary |
| --- | --- | --- |
| Shared control database | Implicit `shared-control`, `kind="control"`, `mode="shared"` | Default single-tenant mode and the system tenant; reuses control pools and the explicit read-consistency policy, while tenant business data still has an independent migration ledger |
| Shared business database | `kind="mysql"`, `mode="shared"` | Multiple tenants share `biz_*` tables; queries remain isolated by `tenant_id` |
| Dedicated business database | `kind="mysql"`, `mode="dedicated"` | One active tenant at a time, with separate accounts, capacity, and backups available |

New configurations default to `multi_tenancy.enabled=false`. Tenant selection and management are exposed only when explicitly set to `true` in the configuration file. `tenant_data.targets[].key` is the stable identity used by migrations, health checks, the API, and placement; a database name cannot replace it. The generator's `source="local"` is shorthand: when `database=control`, it reads primary; when the storage scope is tenant, it reads `tenant_data.default_target`. That key must therefore refer to a MySQL target, and implicit `shared-control` cannot be used as a local schema source. Specify `primary` or `tenant_data.targets.<key>` directly to remove ambiguity. Provisioning a new tenant still explicitly submits `data_target_key`, and runtime routing uses only placement in the control database. Missing placement, unknown or unreachable targets, and fence or generation mismatches fail closed without falling back to `default_target` or `shared-control`.

When evolving from single-tenant `shared-control`, first add a shared or dedicated target to the configuration, create an empty database, set its target `password` field, run `cargo migrate -- tenant-data up --target <target-key>` and `verify`, and complete a live probe in “Platform Management → Data Targets.” Select the target key when provisioning a new tenant. Existing non-system tenants use the tenant data placement/migration flow: first preview and validate the placement generation, plan hash, and idempotency key, then copy the data and switch placement. Changing `default_target`, DNS, or copying a schema does not migrate existing data. The system tenant cannot migrate business data, and an occupied dedicated target must first be migrated or cleaned up.

Independent target pools are created on demand. Each pool defaults to the integer quotient `max_total_connections / max_open_targets`, overridden by target `max_connections`. Both budgets apply only to MySQL tenant targets, excluding primary, replicas, sources and `shared-control`, which reuses control pools. API and Worker count independently. When a budget is reached, only idle pools without active leases can be reclaimed; if capacity remains insufficient, the new pool is refused without displacing active sessions. `tenant-data up/verify/status` uses target keys, and `--all` processes only registered targets. For multiple targets, check physical instances and schemas first. Duplicate aliases are rejected before DDL. Check status after a failure; completed targets are not replayed automatically.
