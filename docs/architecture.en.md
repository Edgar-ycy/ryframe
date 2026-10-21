[简体中文](architecture.md)

# Architecture and Extension Points

Plugins such as MQTT, email, and optimistic locking are on the future roadmap. The preferred approach is on-demand Rust crates and compile-time assembly, with no additional service requirements when they are not introduced or enabled. MQTT and email are designed as integration capabilities, while optimistic locking is designed as a persistence extension. Their introduction will also address dependencies, configuration, lifecycle, permissions and menus, tenant isolation, the capability catalog, and plan synchronization. The current version does not implement a plugin system or add empty interfaces, dynamic loading switches, or placeholder dependencies.

This document helps business developers understand how a request passes through RyFrame and choose suitable extension points.

## Runtime Structure

```text
浏览器 / 客户端
       │ HTTP / WebSocket
       ▼
ryframe-api（路由、DTO、认证提取、OpenAPI）
       │
       ▼
ryframe-application（业务用例、事务、状态机、端口）
       │
       ├── ryframe-db / ryframe-tenant-db（控制库与租户库）
       └── ryframe-adapters（Redis、对象存储、表格、限流、遥测）
       │
       ▼
ryframe（API、Worker、迁移与依赖装配）
```

The HTTP layer parses a request into an explicit DTO and calls an application use case. Use cases access databases, Redis, and object storage through ports, allowing business flows to be tested independently of concrete connection implementations. The API and Worker use the same set of application services.

## Module Responsibilities

| Module | Development purpose |
|---|---|
| `ryframe-kernel` | General-purpose IDs, Snowflake, pagination, errors, and value objects |
| `ryframe-config` | Configuration structures, environment overrides, validation, and version phase determination shared by tools |
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

`ryframe-application::system` exposes four business domain entry points:

- `identity`: users, roles, permissions, departments, profiles, imports, CAPTCHA, and WebSocket tickets;
- `platform`: tenants, products, and authorization diagnostics;
- `content`: configuration, dictionaries, notices, files, options, and standard content resources;
- `operations`: messages, exports, audit logs, login logs, online users, monitoring, backup and recovery validation, and retention policies.

When looking for an existing capability, start with the public services in its business domain, then proceed to the specific use case.

## Choosing a Development Approach

Cross-repository OpenAPI-derived files use `scripts/api-artifact-manifest.json` in the developer's frontend repository as the authoritative manifest, read jointly by the frontend generator and backend contract transaction. The version must be 1, and `files` must be a nonempty collection of unique, safe paths relative to `src/`. The manifest, renderer, and other inputs are all included in snapshot checks before and after the transaction; changes cause installation of the candidate result to be rejected.

RyFrame's official source does not include built-in business examples. Developers use `cargo generate -- new-business <业务包名> --write` to create a business crate under `crates/`, such as `order-business` or `inventory-business`, and access framework extension interfaces through `ryframe-sdk`. Omit `--write` to preview. The scaffold atomically writes the workspace member, composition root dependencies, and the single registry. By default, a business crate contains its model, use cases, Repository, Handler, routes, and migrations together; split it along actual boundaries as the business becomes more complex.

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

External operations tools produce consistent database and object backups. The application backup use cases own registration, the recovery state machine, and transactions. Control database Repositories store manifests and drill records; the tenant database implements target and data validation; adapters implement file digests, object digests, and readiness probes. The maintenance CLI reads explicit configuration, assembles ports, and prints structured results. The API background collector reads only aggregate status and exports metrics with fixed dimensions to Prometheus.

Recovery proceeds through start, data validation, and business and runtime validation; failure is terminal. The target must have independent scope and ownership. Data validation compares explicitly registered physical databases, schemas, tenant relationships, table digests, and object digests. Business proof is bound to the drill record and exact frontend and backend source. Success is registered only when data validation, complete browser scenarios, API/Worker readiness, and recovery time objectives are all satisfied. For detailed operations, see the [Operations Guide](operations.en.md#failed-or-expired-backups).
