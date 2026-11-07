[简体中文](architecture.md)

# Architecture and Extension Points

Plugins such as MQTT, email, and optimistic locking are on the future roadmap. The preferred approach is on-demand Rust crates and compile-time assembly, with no additional service requirements when they are not introduced or enabled. MQTT and email are designed as integration capabilities, while optimistic locking is designed as a persistence extension. Their introduction will also address dependencies, configuration, lifecycle, permissions and menus, tenant isolation, the capability catalog, and plan synchronization. The current version does not implement a plugin system or add empty interfaces, dynamic loading switches, or placeholder dependencies.

This document helps business developers understand how a request passes through RyFrame and choose suitable extension points.

## Runtime Structure

```text diagram
Browser / Client
       │ HTTP / WebSocket
       ▼
ryframe-api (routing, DTOs, authentication extraction, OpenAPI)
       │
       ▼
ryframe-application (use cases, transactions, state machines, ports)
       │
       ├── ryframe-db / ryframe-tenant-db (control and tenant databases)
       └── ryframe-adapters (Redis, object storage, tables, rate limiting, telemetry)
       │
       ▼
ryframe (API, Worker, migrations, dependency assembly)
```

The HTTP layer parses a request into an explicit DTO and calls an application use case. Use cases access databases, Redis, and object storage through ports, allowing business flows to be tested independently of concrete connection implementations. The API and Worker use the same set of application services. The composition root uses `APP_ENV=dev|test|prod` to select mandatory `config/app.<environment>.toml`, merged with common `config/app.toml`. Passwords, JWT, monitoring, Redis and storage credentials use plaintext fields, and the initial administrator password comes from `[bootstrap] admin_password`. The root `snowflake_worker_id` must differ across instances in the same deployment. Application `--config-dir <directory>` defaults to `config` and resolves relative paths from the current working directory; generator CLI, Builder `.configuration_directory(path)` and optional `[generator] config_dir` resolve them from the backend Workspace root. Generator priority is explicit CLI/Builder, local `.local-tests/tools.toml`, common `config/tools.toml`, then `workspace/config`; tool files do not accept environment overrides. Localization uses TOML field `app.locales_dir`, defaulting to `locales`, resolved from the process current working directory for ordinary applications; `APP_LOCALES_DIR` is not read. Resource loading failures strictly reject startup in production, while development allows an embedded fallback. The development supervisor implements complete private file configuration snapshots per generation, switches after configuration saves, and rolls back on failure. Port and instance identifier changes reuse existing binaries; rollback restores the previous generation's configuration, ports, and identifiers. An uncertain stop retains the scene and refuses to start a second Worker. Real-service acceptance of the complete flow remains pending and cannot be replaced by targeted tests; see the development guide for usage. Four-role deployment templates and private configuration directories are integrated and have passed asset structure checks and CI tool tests. Real containers, images and the complete flow remain unaccepted; partial checks do not establish a complete gate pass.

## Module Responsibilities

| Module | Development purpose |
|---|---|
| `ryframe-kernel` | General-purpose IDs, Snowflake, pagination, errors, and value objects |
| `ryframe-config` | Configuration structures, plaintext TOML file merging, validation, and version phase determination shared by tools |
| `ryframe-auth` | Passwords, JWT, and RBAC decisions |
| `ryframe-application` | Business use cases, transactions, state machines, and outbound ports |
| `ryframe-db` | Control database queries, writes, and migrations |
| `ryframe-tenant-db` | Tenant target routing, queries, writes, and migrations |
| `ryframe-adapters` | Redis, object storage, spreadsheets, rate limiting, localization, and telemetry |
| `ryframe-api` | Axum routes, DTOs, OpenAPI, extractors, and transport middleware |
| `ryframe` | Startup assembly for the API, Worker, migrations, and rebuilds |
| `ryframe-sdk` | Stable resource, routing, persistence, and migration extension interfaces for developers' business crates |
| `ryframe-generator` | Offline generation of standard resources; the default build excludes database drivers |

## Per-Process Compilation Scope

By default, the root composition crate compiles only the API and Swagger UI. Standalone processes must disable default features and select only their corresponding entry point to avoid bringing HTTP, Redis, image processing, and telemetry into migration tools:

| Process | feature |
|---|---|
| API | `bin-api`, with `runtime-swagger-ui` added as needed |
| Worker | `bin-worker` |
| Control database migration | `bin-migrate` |
| Tenant data maintenance | `bin-tenant-data` |
| Non-production rebuild | `bin-reset` |

`ryframe-adapters` does not enable expensive capabilities by default. The API explicitly combines `image-processing`, `monitoring`, `otel`, `redis-api`, and `spreadsheet`; the Worker uses `monitoring`, `otel`, `redis-client`, and `spreadsheet`. CAPTCHA enables only the PNG codec, and general image processing codecs are compiled only with `image-processing`. Generated fakes are available only under `ryframe-application/test-support` and are excluded from product processes.

The Worker's health status model and database monitoring port reside in application and do not depend on `ryframe-api`. The HTTP health endpoint is assembled only in the Worker composition root. All binaries install the AWS-LC provider before creating MySQL, Redis, HTTP, object storage, or OTLP clients. Startup is rejected if a different provider is already installed. Security Audit parses the actual Cargo feature tree: the API, including Swagger UI, and the Worker require AWS-LC on the JWT, HTTP, Rustls, and SQLx paths and reject the actual `ring` package/provider. The Worker additionally prohibits a dependency on `ryframe-api`. Migration processes retain only the AWS-LC TLS path required by the database and prohibit API, Adapter, Web, Redis, image processing, telemetry, and HTTP client dependencies. The generator's default runtime graph prohibits database and network runtimes and limits the unique package closure to 90; schema import dependencies are introduced only by the corresponding feature. The `ring-io` and `ring-sig-verify` compatibility features within `aws-lc-rs` are not ring providers. The unified provider describes only the project's choice of cryptographic implementation and does not mean that the project automatically obtains FIPS certification. The OTLP HTTP protobuf outbound path compiles only trace, the Tokio runtime, and the reqwest client, excluding logs, metrics, and the blocking client. The generator and xtask share the tools capability of `ryframe-config` to determine the stable release phase, reading only the Workspace manifest, Git tags, and historical commits. Migration history freezing tools do not call Python or a database.

Telemetry uses file configuration exclusively: headers, resource attributes, span limits, and batch processing parameters all take effect from the merged configuration, while `OTEL_*` environment variables have no effect. The [default configuration](../config/app.toml) and the `TelemetryConfig` and `TelemetrySpanLimits` types define the fields and defaults. The framework determines SDK metadata, service name, version, and build commit identity in the resource; custom resource attributes cannot override them. Files store explicit configuration values directly; `Debug` fully hides endpoint, header, and resource attribute values, and validation errors do not echo credentials. Sampling, AWS-LC TLS, exporter failure metrics, and graceful shutdown behavior are retained. Real OTLP HTTPS acceptance must still be performed separately and cannot be replaced by configuration checks.

The local `opentelemetry_sdk` and `opentelemetry-otlp` patches provide explicit builder entry points that do not read environment variables, preventing SDK or exporter environment defaults from overriding telemetry configuration while preserving the original third-party default entry points. Both remain at `0.33.0`. The SDK crate release package does not include the original license file. The local SDK and OTLP Apache-2.0 licenses both use the license source from `opentelemetry-proto 0.33.0`, verified against the same upstream commit; file digests and provenance are recorded in the [supply chain policy](../tools/policies/supply_chain_policy.json).

`ryframe-application::system` exposes four business domain entry points:

- `identity`: users, roles, permissions, departments, profiles, imports, CAPTCHA, and WebSocket tickets;
- `platform`: tenants, products, and authorization diagnostics;
- `content`: configuration, dictionaries, notices, files, options, and standard content resources;
- `operations`: messages, exports, audit logs, login logs, online users, monitoring, backup and recovery validation, and retention policies.

When looking for an existing capability, start with the public services in its business domain, then proceed to the specific use case.

## Choosing a Development Approach

Cross-repository OpenAPI-derived files use `scripts/api-artifact-manifest.json` in the developer's frontend repository as the authoritative manifest, read jointly by the frontend generator and backend contract transaction. The version must be 1, and `files` must be a nonempty collection of unique, safe paths relative to `src/`. The manifest, renderer, and other inputs are all included in snapshot checks before and after the transaction; changes cause installation of the candidate result to be rejected.

RyFrame's official source does not include built-in business examples. Developers use `cargo generate -- new-business <business-package-name> --write` to create a business crate under `crates/`, such as `order-business` or `inventory-business`, and access framework extension interfaces through `ryframe-sdk`. Omit `--write` to preview. The scaffold atomically writes the workspace member, composition root dependencies, and the single registry. By default, a business crate contains its model, use cases, Repository, Handler, routes, and migrations together; split it along actual boundaries as the business becomes more complex.

Business modules are explicitly connected through ordinary Cargo dependencies. `crates/ryframe` is the sole composition root, and `business_modules()` is the sole registry. The API, Worker, and migration programs share this list. This structure does not use runtime directory scanning, automatic link registration, or build scripts that rewrite source code. Both the Cargo dependency graph and module dependency graph can be checked statically for cycles.

Standard CRUD is generated from a handwritten Rust `ResourceModel` into the business crate's own `src/generated/` and `migrations/`. Directories are organized into `entities`, `dto`, `repositories`, `services`, `handlers`, and `openapi` for readability; `resources/` and `extensions/` are always maintained by the developer. Default catalog compilation depends only on the lightweight SDK and does not introduce a database or network runtime.

For framework resources, `render_resources` generates only backend assets offline. When assets for both repositories are required, use `render_resources_with_frontend` to explicitly specify the frontend template source. `cargo xtask generate resource` retains cross-repository checks and writes, while `--explain` does not require the frontend. Backend-only planning and writing do not read or validate frontend paths and preserve all previous frontend ownership entries. Explicit cross-repository operations revalidate managed files across the entire requested scope. Named generation is bound to the actual rendering source and registers only already managed resources and the current resource; unrequested frontend outputs are not treated as deletions.

Use custom use cases when transaction orchestration, external connections, asynchronous tasks, or specialized state machines are needed:

1. Define requests, results, and business flows in the business crate's `extensions/`.
2. When external capabilities are needed, define ports and implement them in that business crate. Access framework capabilities through `ryframe-sdk`.
3. Assemble routes, OpenAPI, and migrations in `module()`. For background execution, the Worker obtains capabilities from the same module registry.
4. Synchronize the frontend contract and complete integration testing.

Standard resources may also retain one strongly typed extension, such as Post export or Notice message publishing. Other ordinary CRUD continues to be generated from the resource manifest.

## Data and Transactions

The control database stores identity, authorization, the tenant directory, placement, and platform tasks. Tenant business data is routed through placement in the control database to shared-control, a shared business target, or a dedicated tenant target. For operations involving multiple writes, the application use case starts and commits the transaction, and Repository calls in the same flow receive the same transaction context. `database.sources` and each tenant target have independent connection pools and transactions. The framework does not automatically shard databases or simulate distributed transactions across the control, source, and target databases.

List displays may choose eventual consistency according to the scenario. Permission checks, task claiming, downloads, and state transitions use strong consistency. Tenant switches and background tasks should continue to pass explicit tenant and scope information.

## Access Control and Contracts

Routes use the `Public`, `Authenticated`, `Permission`, or `Capability` access policy. Framework menus, permissions, page keys, and capability come from `catalog/access.toml`. For standard business modules, the Rust model generates the module's `access.toml`, and initialization, SQL export, and OpenAPI assembly share the parsed result. Business routes are associated with their policies at the API layer. The composition root installs the system-tenant-only business boundary once; tenant provisioning, permission management, and session projections use the same boundary.

API requests and responses are included in the OpenAPI snapshot, from which the frontend generates operation descriptors. For synchronization steps after an API change, see the [Development Guide](development.en.md).

## External Backup and Recovery Validation

Rust performs historical process stop verification centrally; other callers pass the complete creation identity and consume a strict, small result. On Windows, a managed native child queries CIM through the safe `wmi` API enabled only for the tool crate. The third-party COM implementation is outside RyFrame's self-maintained Rust. A CIM observation and a stop conclusion remain separate: query failure cannot substitute for stop evidence. See the [Operations Guide](operations.en.md#collection-and-registration) for boundaries and troubleshooting.

The unified Rust stage controller executes seed source-register directly. It actually holds the target Generation and copy-ledger guard, verifies complete initialization, copy, seed-close, and producer histories, and publishes six local evidence files in a fixed order, completing two rounds of source-anchor, request, and generation comparisons before writing the final source registration. Normal finalization verifies and releases the locks held by this invocation. Failure preserves partial evidence; abnormal residual locks require reconciliation against their actual state rather than deletion followed by replay. A Windows native CIM query failure rejects registration without falling back to PowerShell/PDH. Stage success returns the unified wrapper containing the nested registration result; see the [Operations Guide](operations.en.md#external-acceptance-driver-for-the-reference-environment) for invocation and result consumption. This local registration authorizes no remote writes and cannot replace full end-to-end recovery acceptance.

External operations tools produce consistent database and object backups. The application backup use cases own registration, the recovery state machine, and transactions. Control database Repositories store manifests and drill records; the tenant database implements target and data validation; adapters implement file digests, object digests, and readiness probes. The maintenance CLI reads explicit configuration, assembles ports, and prints structured results. The API background collector reads only aggregate status and exports metrics with fixed dimensions to Prometheus.

Recovery proceeds through start, data validation, and business and runtime validation; failure is terminal. The target must have independent scope and ownership. Data validation compares explicitly registered physical databases, schemas, tenant relationships, table digests, and object digests. Business proof is bound to the drill record and exact frontend and backend source. Success is registered only when data validation, complete browser scenarios, API/Worker readiness, and recovery time objectives are all satisfied. For detailed operations, see the [Operations Guide](operations.en.md#failed-or-expired-backups).

## Non-production Rebuilds

`ryframe-reset` is only compiled with the explicit `bin-reset` feature, and production images do not copy this binary. First inspect the full commit with `git rev-parse HEAD`, then fill the 40-character Git SHA into `reset.source_sha` in the selected plaintext configuration. `plan` requires this field and the non-secret rotation declaration `reset.credential_version`: 1–64 ASCII letters, digits, dots, underscores or hyphens, updated whenever any reset-related password or private key rotates. `reset.services_stopped` defaults to `false`; `plan` requires neither a `true` value nor `reset.state_directory`. Only `execute` requires API, Worker and scheduler to be stopped first, an explicit `reset.services_stopped=true`, an explicitly supplied absolute `reset.state_directory`, and all read-only preflights. Supply the administrator password explicitly as plaintext in `[bootstrap] admin_password`, meeting the complexity policy. The ordinary seed account `user` uses the shared initial password `123456` and requires no separately supplied user password. Rebuild verification checks `admin` and `user` for both their password hashes and `password_change_required=true`, failing closed on any mismatch; both accounts must change their passwords after the first login.

```powershell
git rev-parse HEAD
$env:APP_ENV = "test"
cargo reset -- plan --config-dir ../ryframe-reset-config
cargo reset -- execute --config-dir ../ryframe-reset-config --plan-hash <sha256> --confirm-reset <exact-confirmation-phrase>
```

`reset plan` reads and validates application configuration and generates a manifest and hash binding source and resource scope, without executing the destructive rebuild or replacing the ownership preflight performed by `execute`. It differs from offline `control export-sql --plan` or `adopt --plan`, which do not read application configuration. Internal physical-alias deduplication in reset only avoids processing the same physical database twice; it does not relax restrictions on dedicated tenant targets sharing a database. The schema isolation constraints in the [Data Guide](data.en.md#tenant-routing) still apply. The execution order is fixed: object prefix, Redis namespace, physical databases, control baseline, tenant baseline, verification, then lock release. Manifests, ledgers, and reports contain no secrets. When an ordinary stage fails and lock release succeeds, resumption is only permitted with the same manifest and ledger. Production environments are permanently rejected before configuration is read or external resources are accessed. `cargo reset` is the direct user entry point; the framework maintenance tool `cargo xtask data reset` consumes the same task graph and requires explicit `--write`. During troubleshooting, do not mix state directories between these entry points or delete a ledger to start a new rebuild.

Report `status` distinguishes `completed` (completed by this invocation), `reused` (reuses the original completion from the same ledger), and `failed`. `completed_at` records the original completion time persisted after lock release; repeated execution neither updates it nor reruns preflights or destructive stages. `reported_at` only represents report generation time. When lock release fails, execution is interrupted, or completion evidence is missing, repeated execution still fails. Manually check service shutdown, MySQL environment locks, and the exact resources in the manifest, retaining the original failure records. Selecting a new absolute state directory in `reset.state_directory` represents a new maintenance operation. The directory path must not contain dot traversal, links or reparse points; generate and verify a new plan, ownership and resource scope rather than triggering reset by deleting or overwriting an old ledger. The current development version uses manifest v5, ledger v5 and report v2. Manifests use non-secret configuration references `password_config_key`, `access_key_config_key` and `secret_key_config_key`, retaining the rotation binding through `credential_version`. Old formats are explicitly rejected without automatic conversion or replay, and old local evidence is retained.

## Cargo tool source verification

Native export verification compares the API/Worker product domains and current build context separately, and revalidates explicitly registered source audits. Changes to tools or documentation never relabel an old artifact as built from new source; successful reuse retains the original build snapshot and fingerprint. The end of the execution window rechecks the complete execution source and frozen artifacts. Producer registries deduplicate by PID and creation time, so different names cannot register the same generation more than once. Complete source capture anchors the canonical root only for the current observation, retaining ordinary-file, link, canonical-path and digest checks for every file. Success and failure both recheck the root path and directory state, without reuse across observations or requests.

For direct Cargo build, check, and test commands, the xtask build script verifies the current Cargo workspace root and complete source file set on every run, including added and deleted files. The compiled snapshot contains the canonical root path and file hashes. Identical contents in different worktrees still represent different compilation origins. The tool's describe operation reads only the frozen compilation snapshot and never recomputes it at runtime.

Unchanged generated contents retain their bytes and modification time. Changed contents are fully written to a temporary file before safe replacement. The script retains all file and directory watches and watches an OUT_DIR path that is never created, causing Cargo to perform verification on every run. If that check path exists, including a link, the build fails and requests verification followed by precise cleanup of that cache object or cleanup of xtask only; it never deletes unknown objects automatically. With the current toolchain, measured same-root, unchanged-content build, check, and test commands still run the build script and recompile xtask even though the generated snapshot hash and modification time remain unchanged. This does not guarantee compilation-free incremental builds. Switching worktrees with a shared target updates the root identity and recompiles. Timings collected during concurrent regression runs are reference values only; isolated performance requires a separate measurement.

The build script also freezes and verifies the compilation input bytes of itself and the source traversal helper. If a cached script differs from either file in the current root, the build fails and requests cleanup of xtask only before rebuilding. Reading current files never substitutes for proof of the script's compilation inputs.

The same protected binary performs complete managed origin-tool source verification through the private `__frontend-origin verify-sources` operation. Standard input accepts exactly `format_version` as integer `1`, `root`, and `sources`, within 4 MiB. The root must be the same real directory frozen at compilation, with no links or reparse points. Bound sources must equal both the compiled list and the complete currently discovered set, with no additional directory exclusions. Each regular file is read twice in full within 16 MiB, checking actual device/file identifiers, size, and mtime/ctime before reading, on the open handle, and after close. Windows ctime is creation time; Unix ctime retains nanoseconds. The response contains only a fixed version, kind, file count, and SHA-256 aggregate token covering the ordered binding and file/root identities, never source bytes. The existing Python loader retains strict binding reads, executable hashes, and binary protection. Inside that protection it obtains the initial token, verifies again before yielding the tool, and compares the final token in finally. Cancellation reaps the current read-only probe without retry and still checks binding and executable identities. URL checks retain their 10-second timeout; complete source scans gain no fixed total deadline. Both retain bounded pipes and fail closed. Source or binding changes require rebuilding and regenerating the managed binding through `__frontend-origin describe`; neither the old source scan nor temporary compilation is a fallback.

The Rust private `__frontend-build-contract --frontend-dir <abs>` entry is the authority for frontend build proofs. Standard input contains exactly the `frontend_inventories` and `backend_inventories` arrays within 16 MiB; responses contain only the three domains and public configuration, identity and complete artifact descriptions. Node obtains the fixed tool binding from the formal consumer context instead of invoking a Python source-contract entry. The authority verifies the existing origin binding, compiled root and complete source closure read-only, retaining source, configuration and artifact protection through response publication; failures publish no success JSON. Recovery Python interfaces only adapt the same Rust models and file verification. An IPC snapshot does not retain file protection, so callers must revalidate at the end of their use window. The actual HTTP server, build orchestration and generic JSON/path helpers still used by the origin loader remain. Configuration bodies never enter the protocol, and large dist artifacts retain streaming hashes without the document-only 16 MiB limit.

The shared-cache regression derives its isolated Workspace package properties, required dependencies, lints, profiles, and patches from the real TOML, resolving source paths to absolute paths in the original Workspace. It first resolves the seed lock offline and rejects any package version, source, or checksum changes; subsequent builds use `--offline --locked`. Only isolated macro copies and the existing `patch_route` consumer test receive a literal probe, executing A, identical B emitting A, changed B emitting B, and return to A emitting A. Production macros gain no test exports.

On each Cargo invocation, `ryframe-macro` rechecks relative paths and byte digests for its `src/**/*.rs`, `Cargo.toml`, and `build.rs`; adding, editing, or deleting source changes its compilation input. Identical bytes across worktrees do not change the fingerprint merely because absolute paths differ. Unchanged generated content preserves its mtime, but this does not guarantee that Cargo skips compilation. An occupied missing-watch sentinel rejects the build without deleting the object; a mismatch with the script's own compiled source also fails explicitly. With exclusive ownership of the shared target, run `cargo test -p ryframe-macro --test source_cache -- --ignored --exact shared_target_rechecks_macro_sources_across_roots`, explicitly setting the process's standard Cargo `CARGO_TARGET_DIR` to that same absolute cache directory. The regression retains A/B sources, Cargo JSON, errors, and timings under `.local-tests`, checking build/check/test, return to A, unchanged-content mtime, and source additions, edits, and deletions. Timings support performance comparisons only in an exclusive environment.
