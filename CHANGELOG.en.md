# Changelog

[简体中文](CHANGELOG.md)

## [Unreleased]

- Move restore-runtime readiness probes and complete frontend resource response verification to Rust. Managed requests bind the current coordinator tool, preserving strict readiness, empty Worker responses, size/redirect rejection, per-file digests, and cancellation/failure semantics. Lifecycle registration, supervision, and recovery orchestration retain their existing entry points.
- Move cache-recovery request structure, frozen receipts, read-only Redis authentication/configuration validation, and exact process calls to the bound Rust tool; retain full directory-identity observations, startup authorization, and Linux supervision primitives, with no automatic replay of unknown results.

- Resource-gate replay now passes driver context and fake-tool log requests as bounded JSON through managed stdin. The corresponding custom environment inputs are removed, while Cargo target isolation, the 20-case comparison, and failure-log evidence are retained.
- Moved department adoption read-only receipts and closed guard evidence validation to Rust, reusing the native 622-entry ordered ledger projection for identity prerequisites. Removed duplicate Python validation and object receipt tests while preserving unknown-write, failure, cancellation, drift, and recovery boundaries.

- Migrate reference business-fixture browser binding, stage orchestration, evidence and static-response validation, atomic tool-configuration publication, and read-only consumers to Rust. Retain the existing runtime and full-stack process-engine interfaces, with tests for complete tree cleanup on timeout, unknown outcomes, and credential redaction.

- Native seed producers bind execution and publication boundaries to the fixed registration, post-copy evidence, plan, and live controller; quota and department protocols recheck the current generation for each operation. Interrupted input or output still closes held sessions, logout remains possible after later source drift, and simultaneous business and cleanup failures retain a failed result.

- Seed registration file validation and create-new publication now run in the bound Rust tool, checking the current attempt, live controller, target lock, and latest post-copy generation; the Python registration writer is removed. Native evidence validation covers identity publication, fixed handoffs, failed starts, and exact stops. Existing orchestration still owns the complete runtime resource guards, and successful registration grants neither Worker startup nor recovery eligibility.

- Native source-object capture and target-object revalidation now use a Rust S3 read port with file-bound credentials and SigV4 signatures, without launching AWS CLI or passing AWS credentials to subprocesses. Public request and export evidence excludes credential values while retaining the private request digest and public-projection verification. Conditional ETags/versions, complete pagination, metadata and ownership checks, streaming exclusive files, and interruption evidence are preserved; redirects, partial responses, and unknown object policies are rejected. Offline capture verification requires callers to specify the transport provenance explicitly; Python capture and copy entry points that have not yet migrated continue to verify their actual tool provenance independently.

- Native recovery-target static configuration validation now reads plaintext TOML, retaining only `APP_ENV=test` for environment selection. Requests may explicitly bind a configuration directory inside the execution backend; `app.test.toml` is required and common `app.toml` is optional. Physical connections, Redis, Worker, initial administrator passwords, and configuration drift remain strictly checked; former credential variable names are no longer configuration sources for this static entry point.

- Python checks install the fixed native-tool descriptor through explicit arguments, removing the rate-limit tool's environment binding. The binding can be installed only once and cannot be replaced; binary identity and digests remain verified for each invocation. The check entry preserves all unittest arguments, UTF-8 mode, and failing test exit statuses.

- Database connections now share an options factory so controlled callers can append validation for new physical connections while preserving existing pool, TLS, SQL logging, and trace-recording policies.

- Python and Node check interpreters now use `[verification]` in `config/tools.toml` and local `.local-tests/tools.toml`, with local fields taking precedence and operating-system PATH as the default. Relative entries resolve from the backend root, preserve venv entries, and reject empty values, control characters, and parent traversal. Removed former `RYFRAME_PYTHON` and `RYFRAME_NODE` selection and the private CI export; interpreter preflight and Rust AST checks reuse file selection. Configuration failures retain the file, specific safe reason, and field repair instructions without echoing raw values; invalid local configuration never falls back.

- Fix resource-gate replay joining multiple arguments into one, preserving Unicode, paths with spaces, and individual option boundaries. Tools first locate the backend workspace through ancestors of the command working directory, retain the compiled-manifest fallback, and remove legacy environment overrides for worktree selection.

- Rust now validates identity pools, tenant distribution, configuration, the operation catalog and actual rate-limit pacing read-only before starting the measurement driver. Duplicate identities, incorrect digests and unresolvable rate-limit rules fail with specific safe reasons.

- Non-production reset ordinary seed users now use the shared initial password `123456`, while administrators still require an explicitly supplied complex password; removes separate ordinary-user password credentials and dependencies in fixtures, source digests, and current callers. Rebuild verification checks both accounts' password hashes and mandatory password-change flags, retaining ownership and unknown-write protection.

- All self-maintained guides and complete change history now have paired Chinese and English versions. Checks validate language links, section references, commands, and code examples; documentation changes run a lightweight check while retaining complete provenance digests.

- The OpenAPI derived-file manifest now reads the frontend's versioned JSON and rejects duplicate fields, duplicate paths, and unsafe paths; removed JavaScript manifest parsing and the corresponding executable-source test fixtures.

- Task measurement correlation, attempt timing, and completion observation are now collected in Rust, using persistent read-only connections, a fresh snapshot for each round, and complete attempt records; automatically binds the current controller artifact and verifies digests before and after measurement, and removes the replaced Python collector. MySQL warnings remain available; connection, protocol, timeout, and cleanup failures retain fixed diagnostics without exposing credentials or raw SQL errors.

- Framework resources generate the backend offline by default, with cross-repository assets supplied by an explicit frontend rendering source; standalone backend checks and writes ignore stale frontend paths and retain the original ownership, while explicit cross-repository operations still verify manual changes, missing files, and path boundaries. Pure frontend templates, UI assertions, and resource consumer contracts move to the frontend; managed previews, naming choices, deletion, and transaction protection remain verified by Rust.

- Business frontend generation calls the pure renderer maintained in the frontend repository through validated IR; Rust then verifies the returned version, asset completeness, ownership, and paths and includes them in candidate installation across both repositories. Backend TypeScript validation now uses the frontend's strict typing and runtime test entry points, removing the duplicate JS harness. The renderer is not called when frontend synchronization is disabled.

- `source_to_seed` `target-verify` now uses Rust to orchestrate complete read-only verification of the database, objects, and Redis, retaining initialization history as the read-only authority, input and actual-file identity protection, a single controller, and failure evidence; raw MySQL output reuses held handles, and complete results are published exclusively after synchronization. `seed_to_arm` continues to use the complete existing state machine; successful verification does not mean that copy or recovery acceptance is complete.

- When Windows process exit and image queries occur concurrently, exit status is verified through the same held handle; query errors for processes that are still running or whose exit cannot be proven continue to fail closed, retaining the specific operation reason.

- The Chinese and English READMEs use the same document-source classification; changes limited to English documentation no longer trigger API, Worker, or frontend product rebuilds. The full source manifest and document digests still retain the change, and unknown paths continue to be treated conservatively as product inputs.

- Configuration tools continue to use the executable explicitly bound by the coordinator after the service environment is cleaned; each read-only invocation holds its own protection handle, allowing concurrent HEAD validation to reuse the tool while retaining stage-controller protection, file integrity, and failure-release checks.

- Copy-source manifests reject noncanonical paths such as duplicate slashes, dot paths, and trailing slashes during fingerprint calculation and manifest parsing, preventing the same file from contributing to different source digests through path aliases; valid Chinese characters, spaces, and filenames containing dots remain supported.

- Python compatibility tests in framework checks reuse the current Rust tool explicitly bound by the coordinator, reducing repeated Cargo launches; file identity, bytes, and digests are verified before and after invocation, and a read-only protection handle is held during execution on Windows. Configuration is still reloaded on every invocation; invalid bindings are rejected without automatically falling back to other tools.

- Reference-fixture `fixture review` review renewal moves to Rust, retaining isolated resource derivation for all three sides, port-conflict rejection, the unexecuted state, final checks of template and business worktrees, and publication of new files; adds real Git fixtures and field-by-field comparisons with the existing implementation, and normalizes Windows credential path separators.

- Business-fixture `fixture source-pair` source-receipt publication moves to Rust, retaining source digests, controlled paths, create-only semantics, and command-result structure; other fixture operations continue to use verified private stage entry points, without prematurely switching commands that have not yet been migrated.

- Formal-recovery `inputs reference|product|bindings` moves to Rust xtask, verifying shared exports, arm, fresh ownership, backups, and product sources in-process; retains strict JSON, independent paths, read-only previews, create-only atomic publication, drift checks before and after publication, and concurrent-write rejection. Explicitly specified backups must match the target plan. Removes the replaced Python input protocol; browser evidence continues to reuse the retained JSON publication tool.

- Formal-recovery monitoring bindings, input staging, loopback HTTP/webhooks, Prometheus/Alertmanager process trees, observation, and shutdown move to Rust xtask; `status/result` continues to execute natively. Static formal-recovery sources and live generations at all three endpoints still reuse the existing Python verifier through a fixed read-only protocol, without substituting field checks. Retains credential-permission checks, file protection during execution, fixed tool versions, isolated environments, exact probes, failure cleanup, and non-overwritable receipts; final publication verifies file identity, hard-link counts, and content. Failed pending cleanup retains an unknown state and blocks replay. Removes the replaced Python monitoring execution modules and tests and migrates their scenarios to Rust; live integration of private DACLs under restricted Windows tokens still awaits verification in an unrestricted environment.
- In `cargo xtask check recovery clone stage`, `source_to_seed` export resource orchestration moves to Rust xtask. The native implementation reuses lifecycle locks, attempts, and controllers, strictly verifies source requests, builds, maintenance CLI, runtime, and producer receipts, collects before/after inventories, complete schemas, SQL dumps, and object bytes through read-only MySQL/object protocols, and checks tenant placement/fence, scheduling relationships, generation/storage drift, file digests, create-new evidence, and failure scenes; adds fake-external-tool tests covering read-only preflight, linked paths, object-byte drift, and successful collection. `target-verify` and `copy` continue to use the original Python resource subprocesses; the Python object-collection receipt format remains available for later copy stages.

- The RustFS resource stage of `cargo xtask check recovery clone storage` moves to Rust xtask. The native entry point retains fixed requests and registered digests, data directories and credential references, readiness of both ports, original startup and actual process identities, failure and unknown-startup receipts, rejection of service-identity changes, explicit `--write`, read-only `status`, and exact process-tree cleanup; adds isolated tests covering successful binding, arguments/paths/links, occupied ports, request-digest drift, stop races, unknown-startup receipts, and read-only status. The `cache` successor and predecessor-tool replacement paths are not yet fully implemented, so they revert to the original state machine and the incomplete Rust code is removed; the Python implementation is still called by seed and the remaining copy stages and is therefore retained.

- `cargo xtask check recovery clone bridge` and `maintenance build|verify` move to Rust xtask: bridge strictly verifies API/Worker or maintenance CLI build receipts, complete Git source manifests, file sets, artifact bytes, build contexts, current product inputs, and link boundaries in native code, and publishes reuse audit files field-by-field compatible with the original Python using create-new semantics. Maintenance builds execute three fixed targeted builds in Cargo offline mode through the unified process layer, freezing binaries, JSON events, logs, source, toolchain, and target receipts, and fail closed on source or toolchain drift, stale outputs, build failures, and unknown partial writes. `maintenance verify` reads existing receipts compatibly in read-only mode; the command entry point no longer starts Python. Adds real Git/Python output comparisons and regressions for input and artifact drift, links, incorrect sources, repeated execution, failure scenes, and receipt fields.

- In `cargo xtask check recovery clone init|status|stage`, `source_to_seed` lifecycle state moves to Rust: natively handles strict manifest binding, create-new run directories, read-only status, process mutual exclusion, controller identity, monotonic attempts, failure/cancellation/unknown-result receipts, atomic stage-result publication, and final source checks. `init/status` does not start Python; stage export, target-verify, and copy resource orchestration that has not yet migrated runs through a single private subprocess, without sharing the Python lifecycle state machine. Deep verification of the `seed_to_arm` target file tree, external-storage generations, and published sources has not yet migrated, so all three commands revert to the original Python state machine to avoid a partial migration. Retains `.local-tests` path, link, and manifest-digest checks, and adds field-by-field comparisons with Python init/status output, controller observation, and failure scenes.

- Rate-limit configuration reading for full-stack, identity-environment, and business-browser fixtures moves to Rust xtask. The native implementation unifies recursive table merging of `app.toml` and environment files, validation of rate-limit booleans, positive integers, and window limits, login endpoint budgets, and authority generation, and outputs results through the `__rate-limit` stdin JSON protocol, which is not included in public help. Node `pacing.mjs` and the single thin Python client no longer start the old configuration script; removes the replaced Python implementation and tests while retaining error redaction, path boundaries, output limits, and existing authority fields.

- Offline pure models and evidence validation for `cargo xtask check recovery clone plan|verify` move to Rust xtask: retains strict JSON field sets and duplicate-key rejection, the 16 MiB limit, schema/business catalog and SQL dump parsing, five object buckets and safe metadata, tenant placement/fence/slot, task and scheduling relationships, deterministic plan hashes, create-new output, and post-write verification. The native entry point does not start Python or access databases or networks; other clone resource stages continue to use existing private protocols. Also completes the tenant-baseline allowlist for data-scope projection tables and adds regressions for Windows long paths, links, input drift, and policy drift.

- User-import performance, browser, and identity-template fixtures move to Rust xtask. The performance entry point retains the original stdin manifest protocol, the browser entry point generates three rows of controlled samples, and the template entry point returns department paths and digests. The ZIP/XML implementation uniformly limits templates to 16 MiB, output to 10 MiB, and the uncompressed large-file sample to 3 MiB; rejects path traversal, duplicate, encrypted, symbolic-link, and excess members; retains other members' original bytes; and creates new output using temporary files and hard links. Node and the identity environment no longer start Python, `bindings.import_samples` only requires an absolute template path, and the replaced Python implementation and tests are removed.

- Combined verification of release sources, exact-source CI, GitHub jobs/artifacts, remote tags, and full-stack sources migrates to Rust xtask, retaining fixed private receipt fields, overall wait deadlines, failure receipts, path and digest bindings, and read-only `--plan` semantics; Release and Extended CI no longer install or invoke release-verification Python.

- The Python environment gate moves to Rust xtask. Rust strictly parses `scripts/requirements-ci.txt`, verifies interpreter identity, the minimum Python 3.11 version, and pinned distribution versions through a single fixed stdin/UTF-8 protocol, then calls `tools/python/rust_function_size.py --native-probe` for 64 AST probe rounds. Retains diagnostics for missing interpreters, version or dependency mismatches, nonzero/no-output/timed-out probes, and native Windows crashes; `RYFRAME_PYTHON`, unified-process-layer UTF-8 cleanup, and the behavior that `--plan` does not start Python remain consistent. Removes the replaced Python environment-check script and tests; recovery monitoring and tenant-data proof now use the retained AST probe entry point.

- The Linux DevEx cgroup memory gate moves to Rust xtask: retains exact CI run/attempt, nonce, source SHA, non-root uid/gid, and kernel cgroup identity checks. A separate Rust subprocess permanently drops privileges before starting Cargo, and another root stage performs exact `cgroup.kill` and directory cleanup after freezing; process registration, failure receipts, and cleanup observations now only create or append files. Regeneration cases still accept only exactly one specified ignored test passing. Extended CI removes Python installation for this job and deletes the replaced Python gate and tests.

- Resource-gate replay moves from a Python script to Rust xtask. The native implementation retains strict manifest validation, exact commits in both repositories, isolated worktrees, ABBA dual branches, a shared Cargo target, dedicated sccache, more than 20 cases and zero-divergence criteria, create-only reports, and activation budgets; CI runs Rust replay tests directly in the resource-gate job, and the old Python source and tests are removed.

- Supply-chain source checks move to Rust xtask. Policy schemas, local patch licenses and registry originals, pinned Action/reusable-workflow/container references, service image digests, tool versions, workflow commands, and Cargo feature trees now use the same read-only implementation; workflow parsing retains anchors, aliases, duplicate-key rejection, and YAML 1.2 boolean semantics. Removes the old Python script and tests used only by that script; the security job no longer installs Python dependencies for this purpose.

- The AWS-LC TLS outbound integration gate moves to Rust xtask: natively generates a temporary loopback CA, runs a Redis TLS proxy and HTTPS fixture, and retains three exact test categories, a 900-second timeout per test, full process-tree termination, non-overwritable logs, and failure summaries. CI no longer installs Python for TLS fixtures; removes the replaced Python gate, HTTPS helper, and tests.

- CI companion-frontend source selection moves to Rust xtask, natively handling event parsing, OpenAPI baseline classification, candidate-contract generation and verification, release-tag mapping, and GitHub job output; retains explicit markers, invalid-baseline fallback, and command-failure diagnostics, and removes the replaced Python implementation and tests.

- Architecture checks move to Rust xtask, unifying verification of crate dependency facts, the public surface of system business domains, document and test layouts, source size, persistence and async-port constraints, Workspace unsafe lints, and static tenant-data boundaries; function size and unsafe syntax continue to be parsed by the pinned tree-sitter helper, with Rust responsible for policy, change scope, and failure diagnostics.

- Deployment-asset and permission-route checks move to Rust xtask. Deployment static, image-archive, and CI stages retain file, link, Markdown-anchor, pinned-image, health-check, configuration, Action, and online-generator boundaries; permission checks continue to enforce the `ryframe-api` compilation policy. Removes the replaced Python scripts and tests used only by the deployment script.

- Migration-history freeze verification, development-version baseline reconciliation, and generator version-phase detection move to Rust xtask and shared tool modules, retaining historical order, uniqueness, digests, stable-version downgrade protection, explicit `--write`, and read-only check semantics; removes the corresponding Python implementations, tests, and embedded generator scripts. Full-stack fixtures now use `cargo xtask data migrate reconcile --write`.

- Standard resource forms gain textarea, boolean switch, date, datetime, exact Decimal, and JSON controls; JSON uses a strictly parsed string form and is converted before requests, while JSON filters that cannot be expressed through standard query parameters are rejected.

- The OpenAPI menu-route contract upgrades to v3 and includes access-catalog icons, keeping generated assets, session menus, and sidebar projections consistent for menus such as permission diagnostics.

- Trivy vulnerability-report validation moves to Rust xtask, retaining severity filtering, exact vulnerability-exception matching, and unused-exception checks; the report entry point becomes a single read-only atomic task and no longer depends on a Python environment or repeated source supply-chain checks.

- Read-only import of existing tables now includes update/delete actions and match rules in foreign-key metadata, preserving composite-column order and rejecting unknown rules; source records retain constraint information, and the generator still rejects foreign-key writes that have not been expressed.

- Tenants gain independent data-scope versions and minimal authorization projections; user, department, configuration-migration, and tenant-provisioning changes atomically record Outbox entries. After verifying fences, the Worker publishes Owner and department facts by build token, rejecting access when projections are not ready or versions lag behind.

- Business ResourceModel supports `owner_field` declarations; after verifying that the field exists and is `i64`/`Option<i64>`, it generates descriptors, OpenAPI access metadata, and CRUD data scopes together. Control-database and tenant business resources use the same declaration rules.

- Authentication principals retain the original user authorization version; cache reads and writes verify that principal and snapshot versions match and reject overwriting an old principal with a newer version. Message tickets also verify token versions, using independently versioned snapshot keys to avoid reusing old structures.

- CycloneDX report validation moves to Rust xtask, retaining specification-version, component, top-level metadata, and reproducible-identifier checks; the report entry point runs read-only and no longer depends on a Python environment or repeated source supply-chain checks.

- Fixes missing data-scope filtering in ID lookup, editing, and deletion for standard resources with a configured owner; scope-sensitive reads use the primary database, writes lock rows by scope within the same transaction, and test doubles align with null-owner and deleted-record semantics.

- Standard business models uniformly generate database and OpenAPI access catalogs; initialization and complete SQL include business menu permissions, and read-only validation rejects missing or drifted bindings. New tenant administrators receive tenant business CRUD by default, while ordinary users and extension operations require manual authorization; business resources exclusive to the system tenant are excluded from ordinary tenants.

- The prerelease-dependency exception-manifest schema only accepts the JSON integer 1, rejecting booleans, fractions, and other types or version values.

- Prerelease-dependency checks move to Rust xtask, reading current Cargo lockfiles, manifests, and deployment references directly, retaining variable resolution, unresolved-reference rejection, invalidated exceptions, and expiry-review checks. Full checks and CI preflight use the same read-only implementation, and diagnostics do not expose credentials in references.

- Password reset and self-service password changes expire the user's other pending reset requests within the same transaction, preventing old links from rewriting passwords; adds explicit recovery instructions for abnormal development accounts and verification for concurrency, isolation from other users, and failure rollback.

- Business generation compiles models in isolated candidates, exports OpenAPI for the current run, validates the frontend, and then installs artifacts for both repositories together; new business creation also locks Cargo.lock, and repeated generation produces zero differences. Multi-table import uses workspace configuration from business subdirectories, range metadata for 64-bit string fields preserves precision, and wide-model pages are split by responsibility.

- Create-only numeric and enum fields now generate the same edit-disabled flags as text fields; test modules move into their owning crates' test directories.

- Adds a pure-Rust report of language and file-byte composition, including tests, templates, and generated code, with vendor reported separately; read-only previews, explicit thresholds, and report writing share the task graph. Checks for removed identity capabilities move to Rust, deleting the corresponding Python implementation and forwarding calls.

- Business-menu route segments use hyphens, and title keys are normalized from snake-case resource names to lower camel case, fixing frontend-contract rejection of underscored resources; invalid routes and title-key conflicts fail closed during backend composition.

- Business-model structural changes generate appended migrations and preserve historical files, digests, and the before/after structure chain; confirmed widening and new columns generate DDL from the same source, while changes requiring backfill or risking data loss stop the entire batch and output review reasons. Fixes full-replacement mapping of nullable business fields.

- Before executing DDL, tenant migrations check all selected targets against actual MySQL instances and database names, reject repeated execution through aliases of the same database, and verify identity again before execution on each target; named read sources remain independent.

- Migration commands support previewing, saving, executing, and read-only verification of existing-business-table adoption plans by module; tenant adoption specifies a single target. Planning and execution share the task graph, while SQL exports and plans share private file paths and non-overwriting publication rules.

- The SDK provides read-only adoption plans, atomic registration, and result verification for existing business tables; binds the physical database, source, and complete ledger before-image, rejects drift and partial registration, requires complete before/after-image verification before handling unknown commits, and confirms completed plans read-only on repeat execution.

- Business-ledger registration uses serializable transactions, holding metadata locks on explicit business tables before commit and verifying structure and ownership within the same transaction to prevent DDL drift between validation and registration; cancellation releases locks through transaction rollback, while DDL itself is still treated as potentially partially committed.

- Business-migration ledgers register sources and table ownership separately for control/tenant; scope preflight rejects unknown records and duplicate ownership. Status outputs migration identifiers and sources, and SQL initialization and execution entry points use consistent metadata.

- The control-database business catalog registers complete current structures; API and Worker startup perform read-only checks for missing tables and drift. The catalog is replaced only after batch preflight, invalid registration retains the old catalog, and runtime verification introduces neither migration execution nor API dependencies.

- Standard CRUD supports business tables without audit timestamp columns, generating timestamp assignments according to declarations; soft deletion no longer requires updated_at. Request-level operation auditing and strict validation of declared audit fields remain.

- Model descriptors explicitly retain primary-key order; reverse import reads PRIMARY indexes and maps physical primary keys other than id. Entity primary-key tuples are generated in index order, DDL retains physical column order, and inconsistent metadata or name conflicts reject writes.

- Business routes are assembled directly from normalized resource paths, fixing runtime paths for hyphenated module names that differed from OpenAPI.

- Business scaffolding adopts unified file transactions, rejects overwriting concurrent registration changes or newly created files, and preserves rollback-failure evidence; fixes Windows CRLF manifests, CLI and Builder package-name parsing, and plan display.

- Business migrations separately verify source and ledger metadata, structures before and after execution, and the latest actual table structures for the entire migration group; structural-drift diagnostics identify column, index, constraint, and other differences without echoing default values. Existing tables and failed post-verification are not presented as completed initialization.

- Global business models retain structures without `tenant_id`; Service, persistence, and page assets are restricted to the system tenant by default. Global structure reading supports ordinary tables without the `biz_` prefix, while tenant catalogs retain their naming boundaries.

- CLI, tool configuration, and Rust Builder share generation plans, supporting batch table or model selection, automatic registration of imported descriptors, prefix mapping, and batch preflight writes; retain original structural metadata and reject partial generation when unselected resources have drifted.

- Business auto-increment primary keys read back actual IDs within the same transaction, retaining tenant composite primary keys and auto-increment indexes; auto-increment and rollback semantics are verified consistently in Fake and real databases.

- Descriptor reading uses the project's locked dependencies and patches; scaffolding, business artifacts, and assembly are formatted uniformly, supporting consecutive module registration. Generated code for empty filters and numeric-only filters passes compilation and Clippy.

- Formal initialization SQL includes structures and ledgers from the same source as registered control-database business migrations, with business writes preceding the control-database completion marker; missing, empty, or duplicate descriptions are rejected before export.

- Business generation reuses standard-resource transaction primitives, checking each file's before-image and installing without overwriting; failures restore installed files. Incomplete rollback retains the backup directory and reports the reason, and zero-difference writes do not rewrite files.

- MySQL structure reading preserves full column types, defaults, and additional attributes, rejecting unknown types, views, and unreadable columns; integer mappings retain signedness ranges, date and JSON models use SDK-qualified paths, and SDK offline models can use JSON.

- Adds Chinese and English READMEs and rewrites first-time initialization and startup steps; completes documentation for read replicas, named sources, shared and dedicated tenant databases, secret overrides, connection budgets, and TLS configuration, and records the future direction of on-demand plugins.

- The generator is unified under `cargo generate`, excluding database import from default builds; business crate creation previews by default and writes explicitly, supports business package names, and adds strict argument validation and plans without execution. Resource generation reads tool defaults, and frontend synchronization uses the current candidate contract.

- Removes the MD5 backfill and old upload-reservation maintenance programs that referenced the old file structure, along with their dedicated features, dependencies, and command entry points; current files continue to use SHA-256 and existing cleanup flows.

- The permission-diagnostics menu declares its icon centrally in the access catalog; initialization and menu synchronization use the same catalog.

- New configurations default to single-tenant mode, exposing tenant and package management according to mode while retaining configuration migration and data maintenance for the system tenant; real multi-tenant acceptance explicitly enables the mode.

- Routine backend checks run independently by default, with explicit scope selection for cross-repository checks; frontend paths support shared and local tool configuration. Task graphs identify backend policies and Rust gates separately, and plans read locked, offline Cargo metadata.

- The initial administrator password is injected explicitly; ordinary new and imported users use an initial password and must change it at first login, with restricted sessions prevented by the backend from accessing business resources or message connections. Adds complete SQL export, including deferred constraints and migration ledgers; real MySQL verification confirms that SQL import and migration agree on structures, indexes, foreign keys, and identity/permission/menu semantics.

- When an authorization mirror lags behind a new token, the primary database is verified before rebuilding the snapshot, while old tokens remain rejected; diagnostics distinguish lagging, ahead, disabled, and unavailable caches.

- Fixes a same-table subquery in menu updates in the control-database review SQL and adds the user-avatar foreign key; first-start documentation explicitly requires creating an empty database and running migration initialization first, and external services are not restricted to a particular operating system.

- Registered reference fixtures can renew current tool digests through an immutable `ready → ready` preflight chain; renewal still requires resources, scope, ports, databases, and service-directory semantics to remain exactly unchanged. This resolves the inability to safely verify or restart the original isolated services after local tools such as WSL/Python update during continuous recovery.

- User-import sample generation supports result directories beyond MAX_PATH through extended-path reading, exclusive publication, and hard links; after completing all generation and source checks, fixtures warm maintenance targets through the same `cargo xtask` alias actually used by the browser, avoiding inline compilation, timeouts, and CPU contention on the first history-status poll.

- Reference-fixture test configuration increases request-rate-limit capacity while retaining base-configuration windows and `api_limits` such as login: the full browser suite's request volume from a single IP exceeds the product's default limit, previously resulting in 429 responses during long flows such as capability toggling.

- The browser-evidence stage restarts only the supervised Worker and retains the original API instance; core suites running inside business-fixture worktrees use the fixture source model, while non-fixture worktrees continue to use the core source branch.

- Business-browser suite cleanup also reclaims Workers within its process tree: Worker control records external termination when the supervisor, member supervisor, and product process are all confirmed exited, ports are released, and no normal-shutdown proof exists. The evidence stage restarts the Worker outside the suite's process tree; any surviving identity, PID reuse, or inconsistent record still fails closed.

- During source verification, nested artifact bridges determine product inputs and build context from the bridge root: fixture product trees differ from the current run root, and verifying against the run root would reject audited bridged builds.

- Post-copy handoff holds the target generation lock during registration, preparation, scheduling, and reading: the initialized file-tree snapshot includes control mutex files, and initialization-history verification fails closed without `initialize.lock` and guard ownership.

- Fixture environments generate a separate test configuration directory within their own output directory: copy the base `app.toml` and write `app.test.toml` according to reviewed tenant targets, consistent with the CI full-stack flow. Tools such as the migration gate can read test targets while the frozen execution tree remains clean.

- Fixture Workers now start through the formal full-stack supervisor and register supervisor/monitor process trees: frozen business-browser cases stop, crash, or restart Workers through `full_stack_worker`, which previously could not control runs without process-tree receipts.

- Business-browser environments carry exactly the same `APP_*` set as the runtime environment: full-stack Worker control verifies run receipts by process-configuration digest, and missing `APP_RESET_*` rejects Worker stop or restart.

- Fresh-target request service lineage uses the same executable-digest definition as the ledger, allowing restarted service generations to reconcile with registration.

- The frozen WSL Redis helper set no longer depends on Windows-side modules: extended-path semantics in process receipts now branch explicitly by platform, with a new regression ensuring the copied set is self-contained.

- Failed service restarts can reconcile back to the original scene after verifying external-termination evidence, then restart from the original predecessor; unknown or unfinished scenes still fail closed.

- Reference fixtures explicitly disable `api_docs` during preparation, matching fixture API builds and runtime environments without `runtime-swagger-ui`; target generations, run registration, and copy runtime environments receive the same configuration digest.

- Copy-evidence link regressions cover both normal and extended path representations and add a native real-symbolic-link rejection case; simulated and actual filesystem calls address the same resource.

- Windows read-only object collection and process-supervision receipts uniformly use extended filesystem paths, retaining original source paths and exclusive-write constraints and supporting deeply nested acceptance directories.

- Windows business-browser maintenance entry points launch commands through the bound Node and formal Corepack script, fixing batch-file quoting for installation paths containing spaces; builds and browsers share an explicit offline Corepack cache.

- Business-fixture browser bindings reuse the complete build source verified during runtime-environment registration, avoiding false source-change detection caused by repeating verification outside that environment.

- Copy-source exports verify registered empty dedicated targets through formal per-target read-only inventories and bind original inventory evidence; inconsistent targets, omitted nonempty targets, or changed evidence fail closed.

- Isolated reference-data preparation strictly consumes reviewed tenant topology, record counts, object sizes, and request intervals, removing hardcoded samples that differed from the reviewed plan.

- Recovery reference data uses packages with no capabilities, allowing deployments to provide an optional capability catalog; invalid catalog responses still fail closed before package writes.

- Isolated-fixture data preparation uses the current controller's maintenance tool and independently registers its source; source changes during execution fail closed, and tool fixes no longer require recompiling frozen products.

- Acceptance process-identity checks distinguish actual exit, image deletion, and access denial; a living process whose image cannot be verified fails closed and is no longer incorrectly recorded as exited.

- The business generator outputs complete tenant-table descriptions, which the composition root assembles centrally into a frozen runtime catalog; copy and recovery tools verify managed business-column structures and retain business-migration ledgers, rejecting missing, duplicate, or changed catalogs.

- Data-maintenance processes use the unified business-module registry; formal scaffolding also integrates the maintenance process runtime feature so enabled modules are no longer omitted when business tables are verified.

- Formal business scaffolding declares optional dependencies actually required by derive macros according to API and persistence features, fixing compilation failures in generated business modules.

- Business acceptance fixtures reuse formal business crate scaffolding and process-feature assembly, maintaining only resource models and removing duplicate business manifests and module-assembly templates.

- Repositories for physically deleted resources no longer generate unused soft-delete imports, keeping business scaffolding builds free of warnings.

- Business-module OpenAPI composition completes permission and menu catalogs from generated resource metadata, retaining framework catalogs and rejecting duplicate business menus; business frontend contracts can synchronize through the unified generation entry point.

- Before capturing generated source, isolated business fixtures synchronize OpenAPI, frontend derived files, and source markers through the formal candidate entry point, avoiding inherited source states inconsistent with the contract body.

- Generated routes consume state at the last actual capability guard, avoiding redundant clones caused by trailing basic resources; fixture-control failures retain redacted validation reasons, and development documentation completes the steps from business scaffolding to frontend integration.

- Optional package capabilities are narrowed to notices/announcements and tenant-initiated message publication; basic management continues to follow RBAC, and empty packages retain basic resources. The capability catalog provides strict empty-object configuration Schemas; announcement delivery validates both capabilities, while historical data and system notifications are neither deleted nor disabled by toggles.
- Reference-fixture runtime service generations resolve review and bootstrap environments from the unique passed restart receipt for the active generation: non-initial generations after tool renewal can continue registering seed runtime identities, while ambiguous sources or generation changes during resolution fail closed.
- Seed runtime, scheduling, write critical sections, and drain guards use post-copy target-storage resolution: source_to_seed fixtures no longer treat the current run as the storage run, allowing storage generations after service restarts to continue quota, department, identity, and shutdown stages.
- Redis tool fingerprints for fixture service-restart generations can be renewed according to the review bound to that stage: physical configuration other than WSL, Python, and Redis server tools must still reuse the first-generation request byte for byte, and tool changes not recorded in the review continue to fail closed.
- Original fresh-target storage receipts may belong to a fixture's registered restart generation: runtime renewal checks the complete ledger to establish that the receipt belongs to the first, current, or any verified restart generation; unregistered generations still fail closed.
- Fixture service observation records identity mismatches, where a PID has been reused by an unrelated process, as disappearance of the recorded identity: external-termination verification no longer fails permanently because of reuse. Any termination, wait, or signal still verifies the recorded identity and never operates on the reused PID.
- After fixture services restart according to a renewed review, fresh-target cache generations allow WSL tool fingerprints to update with the historically verified request for that generation; configuration, artifacts, ports, distribution, and Redis/Python entry points must still match the target's original receipt.
- Fresh-target request issuance allows the service ledger to retain the original review while the request binds a tool-renewal review whose lineage is proven through the same seed private environment; mismatched reviews or failed renewal validation still reject issuance.

## [v0.13.1] - 2026-10-08

- Realigns release versions and formal OpenAPI across both repositories after companion-frontend export browser acceptance fixes.
- Seven list-export categories support selected records; request statistics and Worker batches jointly constrain selected primary keys, filters, and data permissions.
- The Vue SFC source-line policy changes to 600 lines, matching the maintenance rules that include templates, scripts, and embedded styles.
## v0.13.0 Consolidated Release - 2026-10-08

- Consolidates previous 0.13.x features and release fixes into v0.13.0, while retaining historical change records.
- Release continues waiting for job snapshots not yet fully returned by GitHub Actions; missing or incomplete required jobs are not treated as successful, while explicit failures still reject release immediately.
- Tag pushes no longer use stale annotated tag objects from events as the commit baseline; migration history is verified against the release commit, and resource and deployment checks use complete gates.

## [v0.13.6] - 2026-10-07

- Fixes backend pre-Release validation incorrectly mapping tag evidence for two backend CI runs to the backend and frontend repositories, and adds repository-identity regression tests.

## [v0.13.5] - 2026-10-07

- Corrects the mismatch between v0.13.4 backend release metadata and the frontend OpenAPI source commit, realigning frontend and backend v0.13.5 release sources.

## [v0.13.4] - 2026-10-07

- Fixes joint Release validation treating the backend extended-CI SHA as the frontend SHA; full-stack receipts bind backend and frontend commits separately and verify against the business crate's actual current generated artifacts.
- Fixes Linux full-stack process monitoring incorrectly reporting Worker startup failure during exit and PID-reuse races; stale snapshots are now safely ignored.

## [v0.13.3] - 2026-10-07

- Fixes out-of-bounds indexing caused by backend Release receipts still being read at old array positions, and adds regression tests for different CI run identities; extended CI uses a free Worker health port selected at runtime to avoid fixed-port conflicts.

## [v0.13.2] - 2026-10-07

- Fixes out-of-bounds indexing in the backend Release verifier, which still read full-stack receipts from a third item after switching to two backend CI evidence items; adds a regression test confirming that receipts come from backend extended CI.

## [v0.13.1] - 2026-10-07

- Backend Release verifies only backend CI, extended acceptance, and formal OpenAPI consumer contracts; the frontend repository verifies its own source through independent CI and Release, avoiding waits for nonexistent or unrelated workflows in the other repository.

## [v0.13.0] - 2026-10-07

- Users can create independent business crates through `cargo ryframe new-business <module-name>`; the command integrates Workspace, composition roots, API/Worker/migration features, and the sole module registry together using staging and rollback. Standard resources continue to generate SeaORM, application services, protected APIs, tenant migrations, OpenAPI typed callers, and frontend pages within business crates; product-package capability catalogs exclude platform tenant management, login now selects tenant names, and configuration migration moves under the platform-management menu with an icon.
- Continuation boundaries after host-restart finalization: the finalization receipt becomes the sole authorization for registering new service generations, and the one restart each of target storage and cache must be adjacent and bind the same generation. Segmented verification accepts this restart segment and, after normalizing storage and cache identities to the new generation, permits only declared session-write increments; all other databases, objects, schemas, and ownership must show zero drift. Cache keys lost during restart count only as the empty state of the new generation, not as successful cleanup.
- Host-restart finalization stage: after restart facts are verified, publishes a unique finalization receipt binding the C70 frozen request, failed-acceptance prefix, host boot proof, exit evidence for the original dual roles and storage/cache, the never-started baseline, and exact session-write increments. Only registration of new service generations is allowed; it does not restart services, collect new images, or replay source stages, and explicitly marks acceptance as failed and replay as prohibited.
- Pre-finalization observation of source-generation host-restart interruption: accepts only the unique finalized successful C70 startup and the exact failed-acceptance prefix without a collected after-image. Kernel host boot proof establishes that the original API/Worker and object-storage generations must have exited and that exact endpoints are explicitly closed; reuses read-only cache status to verify the pre-restart Redis generation. Build-environment drift, surviving processes, and occupied ports are listed only as blockers; observation itself does not start, stop, or replay any resources.
- Source-run acceptance distinguishes naturally expired authorization snapshots from persistent version keys that must exist, atomically verifying exact keys and content before cleanup; cache-stage failures can continue within the same living generation using the original producer, audit, and complete before-image, separately binding a clean repair source while retaining unknown-response reconciliation and source constraints for stopping and export.
- Object-storage stop and cleanup first verify and reclaim all registered historical processes one by one, then check shared-port release together, preventing a dead old generation from being blocked by a legitimate successor listener; unknown processes and residual foreign listeners still reject successful publication.
- In never-started recovery, an inventory-argument parsing failure immediately following an audited structure-verification failure can preserve history and continue after full verification of local evidence, original product and coordinator sources, and actual binaries; this does not permit skipping additional failed stages or treating partial collection as a complete baseline.
- Audited collection failures in never-started source generations can recollect complete baselines through the original recovery entry point after preserving the original failure, partial files, and maintenance-binary proof; other failures, unknown files, and cleanup after actual startup do not qualify for retry.
- Audited pre-start source-generation failures can archive original source, failure, and controller evidence through the original recovery entry point and collect the current complete baseline; only one subsequent actual startup is allowed. Ordinary missing trees, unknown writes, and failures after startup still prohibit replay.

- Source-successor startup, verification, stop, and export retain control environments and restricted service environments separately; build sources and toolchains remain strictly verified against the control environment. Inventory resources and business processes do not inherit build variables, and unregistered environment drift is rejected both before and after switching.
- Successor source-generation requests verify `--source-backend` and companion `--product-backend` according to source-directory responsibilities, allowing registered main repositories or worktrees; parsing and private-protocol publication reject invalid paths and links, while evidence and output remain restricted to controlled acceptance directories.
- Fixes the cache recovery adapter failing before its first Redis observation because of a missing checkpoint; connection, authentication, and marker commands reuse the current stage guard, and failures before marker writes begin can explicitly continue along the original process generation.
- Recovery of published seeds verifies the original initialization stage, inventory, and reset before/after images through the full publication chain, rather than incorrectly applying archival requirements for a new fresh workspace; new targets still strictly require complete workspace archival, and historical records remain unchanged.
- Windows copy-command receipts support controlled paths longer than 260 characters; if the first continuation fails during read-only receipt publication, one strict successor can be appended within the same service generation and complete before-image, without replaying confirmed migrations or cache markers.
- Cache recovery of published seeds can explicitly adopt WSL/Python verified in a successor review through the existing `restart --request`, retaining original registration, Redis resource identity, and unknown-write restrictions; recovery of object storage from the same source no longer depends on unrelated old product-build bridges.
- Formal-recovery target-plan verification, preview, and explicit publication converge on a single target module, removing standalone scripts that only contained duplicate imports and argument forwarding; reference-recovery entry points and tests directly reuse the same implementation.
- Device reference-data `plan` and `prepare` now use strongly typed Rust requests; environment, runtime, working directory, plan files, and base/candidate target sides are verified before starting the fixed Python implementation. Both write stages require explicit authorization and pass through the unified fixture-control private protocol without argv. Python loads runtime and recovery business dependencies only after protocol validation and removal of environment fields; the original semantics for 100,000 records, 1 GiB of objects, source binding, and unknown-write failure receipts remain unchanged.
- The five operations of the 168-hour retention fixture now use strongly typed Rust requests; run directories, tenants, migration IDs, plan digests, and read-only/resource-write boundaries are verified before starting the fixed Python implementation, then passed through the unified fixture-control private protocol without argv. Python loads MySQL, runtime, and export dependencies only after protocol validation and removal of environment fields; original history shifting, backup registration, ownership, and complete before/after-image semantics remain unchanged.
- The `snapshot` and `verify-deleted` acceptance entry points for full-stack export physical objects now use strongly typed Rust requests; run directories, task IDs, receipt paths, and read-only/receipt-write boundaries are verified before starting the fixed Python implementation, then passed through the unified fixture-control private protocol without argv. Original database, object-storage, and ownership verification remains unchanged.
- Offline plans, ledger stages, running services, post-copy processing, and non-source seed operations under `cargo xtask check recovery clone` now use strongly typed Rust requests; the first layer uniformly verifies `.local-tests` absolute paths, roles, enums, and read-only/evidence-write/business-write contracts, then calls the original state machine through versioned private JSON without argv. Python removes the protocol environment before loading copy business modules, no longer maintains a second argparse manifest, and rejects direct script argv. Runtime status uses observation without writing files; invalid target-verify continuation is rejected before ledger creation, and original ownership, before/after-image, and unknown-result verification remains unchanged.
- Formal-recovery Runtime and Source commands now parse strongly typed arguments entirely in Rust, rejecting duplicate, unknown, missing-value, conflicting write authorization, and unsafe-path arguments before starting the fixed Python implementation; private stages accept only a versioned single-environment JSON protocol, remove it before business dispatch, and no longer support old argv entry points.
- The Device-fixture source-pair issuance entry point now uses strongly typed requests; new receipt paths are restricted before Python starts to existing real parent directories within the current backend's `.local-tests`. Writes require explicit authorization; the fixed implementation reconstructs arguments only from the versioned private protocol, while direct internal runtime function calls remain unchanged.
- The Device isolated-fixture worktree-preparation entry point now uses strongly typed Rust requests; output is restricted before Python starts to a nonexistent real directory within the current backend's `.local-tests`. Formal candidate commits must be supplied as a pair, and fixed frontend paths and requests are passed only through the versioned private protocol, which is not inherited by Git, Corepack, or Cargo descendants.
- Formal-recovery monitoring bind, start, observe, close, result, and status now use strongly typed Rust requests; xtask verifies read-only/write boundaries, run IDs, three ports, `.local-tests` evidence, and link-free external tools before starting the fixed Python implementation, then passes them through a versioned protocol without argv. Internal staged lifecycle and failure-receipt semantics remain unchanged.
- `cargo xtask check recovery dataset-prepare` now parses mutually exclusive preparation/existing-data-verification modes, verification sides, and explicit write authorization at the first layer, accepting only existing evidence within the current backend's `.local-tests`; the fixed Node implementation uses a versioned single-environment protocol without argv, and the original recovery coordinator calls it through the same protocol. Plans, preflight, and ownership are still reverified before login, object downloads, and data generation.
- R24-fixture environment, review, fresh-target request, successor, and service-control entry points now use strongly typed requests grouped by domain; Rust rejects unknown, duplicate, missing, mutually exclusive write, and out-of-bound/linked-path arguments before starting fixed Python implementations, passing them only through a single versioned private JSON protocol. The five Python implementations no longer accept direct argv.
- Formal-recovery reference plans, data verification, backup, restore, replica corruption, and derivation of three input categories are uniformly parsed into structured requests by `cargo xtask check recovery`; unknown, duplicate, missing, and cross-stage arguments are rejected as argument errors before private implementations start. Evidence paths are restricted to the current backend's `.local-tests`, rejecting traversal, links, and overwriting; read-only previews reject `--write`, and evidence publication or resource operations require explicit authorization. Fixed Python implementations use a versioned single-environment private JSON protocol and reject direct argv; original ledger, ownership, complete before/after-image, and unknown-result verification remains unchanged.
- The nine Device-fixture runtime stages are now parsed into structured requests by `cargo xtask check recovery fixture runtime`; Rust verifies explicit writes, read-only mutual exclusion, absolute paths, current-backend `.local-tests` boundaries, and links/reparse points before starting the fixed Python implementation, passing them through a versioned single-environment private JSON protocol. Direct Python execution no longer accepts argv.
- Performance-identity plan, apply, and verify writes are consolidated under cargo xtask data performance-identities; Rust verifies explicit writes, absolute paths, and .local-tests boundaries before starting the fixed Node implementation and passes requests through a versioned single-environment private JSON protocol, rejecting argv, duplicate or unknown fields, and inherited contamination.
- The backup, restore, target, file, and reset operations of `cargo xtask data` are parsed into structured requests at the first layer; unknown, duplicate, missing, or mutually exclusive arguments are rejected as argument errors before maintenance binaries start. Evidence inputs and outputs are restricted to absolute real paths within the current backend's `.local-tests`; registered backup roots, isolated configurations, and runners retain support for external absolute directories, and all paths reject links. Record publication and business writes gain explicit `--write`, while underlying ownership, plan, confirmation, and resource verification remains unchanged.
- Recovery build receipts upgrade to strict v2, separately recording API/Worker product inputs, acceptance-tool inputs, and complete source manifests, and binding Cargo commands, profile, target, jobs, actual cargo/rustc versions, and effective environment-variable names and value digests without recording values. Tool-source changes are no longer mistaken for product changes; changes to build arguments or product inputs still reject reuse.
- The unified copy-recovery entry point gains explicit maintenance-tool build and read-only verification stages; controlled run directories, fresh-target working directories, and their evidence arguments may use relative paths under the current backend's `.local-tests`, while links and out-of-bound paths remain rejected.
- Copy verification combines read-only collection in the same batch across the full database vector and classifies results together after source, payload, and ledger verification; database writes still receive independent before/after observations. Object-binding checks omit unused complete-model parsing while retaining catalog-content drift checks.
- Fixes copy-object verification failing to recognize AWS CLI HEAD 404 responses with zero-retry markers; missing-object detection continues to reject permission errors, timeouts, and multiple diagnostics.
- Windows copy stages reuse registered binary digests protected by kernel handles preventing writes and deletion, verifying actual paths and file identities each time; complete source, credential, process, and business-data verification remains, and other stages do not enable reuse.
- Copy verification and final object readback use four-way bounded reads, rechecking source, selected-object proofs, payloads, and ledgers at the end of each batch before accounting according to plan. They reuse the same complete collection stage and verify actual row counts in the same SQL-state scan, retaining full table declarations, empty tables, relationships, and digest checks before and after scanning. Writes remain serial; failures retain original evidence and wait for in-flight reads to exit. Single-object verification is limited to its bucket, while complete five-bucket ownership is still verified before stage writes and at finalization, and original source-export proofs remain fully verified.
- The unified copy entry point gains seed API-environment registration, preparation, scheduling-change verification, and existing-data verification, integrating fixed-identity preparation and independent API/Worker runtime handoff; failed starts can explicitly continue after exact dual-role stopping and historical reconciliation. Identity-environment validation correctly recognizes implicit shared control databases; Node sessions register actual creating identities and unique runtime arguments, support explicit cleanup after abnormal controller exit, and retain unknown-write and failure evidence.
- Seed identity preparation gains quota plans for explicit tenants, one-time application, and unknown-response reconciliation; capacity includes actual users, fixed measurement identities, and import-growth headroom, and authoritative permissions are refreshed and reverified after updates. Identity preparation consumes only complete capacity proofs, without automatically replaying unknown writes or changing product default quotas.
- Development copying gains a unified staged entry point that verifies the scene in a fixed run directory and continues the original ledger, retaining failure records, completed steps, and unknown-write states; environment startup and shutdown bind the latest kernel process identities. Product-build inputs and acceptance-tool fingerprints are recorded separately, allowing existing artifacts to be reused through explicit source audits after tool changes, while formal recovery still requires exact clean source. Duplicate source scans are merged within the same generation check, object steps verify only the original proofs they depend on, and stage beginnings and ends retain full checks. Semantic results of complete plans are reused within a process while still verifying inputs, source policies, all evidence, and pre-write payloads; copy SQL physical-reference checks reuse a bounded rule cache, preserving logical-tenant and physical-field validation boundaries.
- The unified copy entry point gains explicit same-directory RustFS restart, read-only status, and exact cleanup, fixing the original service artifacts, credentials, and data directories while retaining original initialization, old receipts, and copy ledgers. Restart is limited to before post-copy business-handoff registration and verifies stopped producers and actual process identities; unknown startup or publication results retain evidence, and original data steps still require verification after runtime recovery.
- The unified copy entry point gains same-configuration recovery and process-identity binding for isolated Redis, atomically rebuilding only the two originally registered acceptance markers; unknown responses are reconciled first and continuation requires an explicit before-image, retaining original initialization and copy ledgers without restoring old sessions or locks.
- Adds read-only complete table inventories for explicit database targets, covering targets with no assigned tenants and separately verifying ownership, migration ledgers, and backup/restore registration data; development copying connects source capture, explicit new-target initialization, and persistent-step execution, with final verification of full row images and object identities. Passing data steps does not mean business or recovery success. Extended CI adds separate execution and cleanup steps for real Linux cgroup cases and includes them in required release checks.

### Fixed

- Strict fresh-target continuation now binds complete prepare/registration/predecessor lineage, exact migration and observation receipts, the direct before-image of the final reset ledger, and the complete target tree under lock; continuation is allowed only from the first missing read-only migration boundary or a registered inventory failure. Unknown files, malformed failure evidence, and unconfirmed writes continue to fail closed.
- Node orchestrators pin UTF-8 arguments and output environments for Python subprocesses through a single internal invocation plan, clearing inherited encoding variables according to Windows case semantics before writing controlled values; recovery-data and performance collection retain their respective bytecode, stdio, protocol, timeout, and exit-code behavior.
- xtask pins UTF-8 mode and output encoding for Python subprocesses through the unified Rust process layer, clearing inherited and explicitly supplied case variants according to platform semantics before writing controlled values; check, generation, CI, migration, and recovery entry points no longer maintain separate encoding arguments, keeping behavior consistent for fixed `RYFRAME_PYTHON` and Chinese paths.
- Extended CI runs real development-server and production-preview browser flows for both core and generated Device isolated sources; the four-cell matrix no longer omits Device/dev.
- Create-new publication of Device browser intents is included in standard failure-evidence boundaries; pre-publication failures explicitly record absence, and synchronization errors after a file becomes visible bind its digest. Both retain a non-replayable scene and failure receipt before business requests begin.
- Formal Device-fixture generation binds exact, clean backend and frontend candidate commits before creating output directories; development snapshots can still explicitly retain uncommitted inputs, while formal recovery no longer waits until generation completes to reject incorrect sources.
- Extended CI real-browser development and production-preview steps explicitly inherit the current MySQL service-container identity, ensuring physical export-object verification connects to the same isolated database.
- Read-only fresh-target status changes to reconciliation and removes continuation suggestions when the fixed request no longer satisfies current executable conditions, retaining original registration and zero-write semantics and avoiding guidance to replay an invalid request.
- Development copying verifies producer shutdown against the registered creation generation, allowing an explicitly newer process to reuse an old PID; when Windows queries are restricted, only the exact PID's creation time can prove the old generation ended. Unclear timing or permissions continue to reject, and process-termination rules remain unchanged.
- Database-inventory failure diagnostics distinguish physical-identity and ownership verification, retaining logical targets, response counts, and command-input digests. Two fixed read-only verification queries use command arguments with stdin closed, exclusively saving and verifying raw stdout, file identities, and cross-layer digests; business SQL retains its original transport. Empty responses, output collection failures, and finalization failures continue to fail closed without replaying unknown writes.
- Development acceptance controllers can recover through persistent identities even if interrupted after directory-lock cleanup, and stage success waits for final source verification and cleanup; target startup rejects internal success files when the outer copy stage has failed or remains incomplete. Stage failures gain controller-bound code-location evidence without recording exception bodies or local variables; diagnostic-write failures still retain the original failure.
- Development-source exports verify control databases by 16-character fingerprints, tenant databases by SHA-256, and each by its own migration ledger; correctly associate global parent tasks for three platform-cleanup scheduling categories and check SQL state and relationships before object capture. Object capture is limited to four concurrent operations, retaining every per-object validation and waiting for in-flight operations to exit after failures. Windows stop observations directly verify the complete kernel listener table; other platforms accept only explicit connection refusal within bounded probes, and unknown states do not pass. Object observation distinguishes complete downloads, explicit HEAD 404s, permission errors, and unknown results, retaining original failure evidence; logs explicitly bind file and configuration credentials for redaction.
- Reference backups bind the clean API/Worker builds actually running before shutdown and reverify existing source-side data, rejecting stale artifacts, process-generation changes, evidence changes, and incorrect collection timing.
- Rebuild reports distinguish completion in the current run from historical reuse and retain the original completion time; lock-release failures, interruptions, or missing durable evidence still reject execution without falsely reporting success.
- Upload entry points enforce configured file and request-body limits, removing Multipart's implicit 2 MiB limit; malformed forms return 400, oversized requests return 413, and underlying read failures retain 500. Chunked requests are subject to the same limits.
- Corrects the TOML hierarchy of the default endpoint rate-limit window so window settings are not treated as endpoint rules; capacity and window values remain unchanged.
- Corrects the direction of DevEx P95 regression percentages so improvements display as negative values; existing regression-gate thresholds remain unchanged.
- DevEx runtime reports distinguish HTTP requests from business cycles, avoid counting failures twice in shared home-page metrics, and record cycle failure rates and session failures separately while continuing to block passage.
- Database-resource measurement verifies both connection and global_status collection health, rejecting residual connection counts and duplicate or invalid metrics after failed collection as valid samples.
- Routine CI, extended acceptance, and release use unified companion-repository sources; Release asset cleanup and publication occur after final CI and tag verification pass.
- Reference-recovery data preparation generates cyclic values within the current post sort-order range, continuing to use valid data beyond 1,000 records without relaxing business-field validation.
- Recovery reference-data preparation uses explicit stage deadlines, avoiding generic-command timeout termination while 100,000 records are still being prepared; preparation time is excluded from recovery time.
- Redis optimistic transactions reuse independent connection generations; failed, timed-out, or cancelled connections are discarded, and connections return only after successful confirmation that WATCH is cleared, avoiding ordinary-command interleaving and a new connection for every transaction.
- Ordinary users read themes through the fixed tenant UI-settings endpoint, avoiding 403 responses caused by incorrectly using the configuration-management endpoint during page initialization; arbitrary configuration queries retain their original permission requirements.
- Tenant-resource generation also registers copy catalogs, column types, and full schema descriptions, fixing validation failure after Device table creation because the catalog was empty; DDL and catalogs share the field-type model.
- Resource manifests verify route-key and menu-key consistency before writes, preventing initialization-seed failure after generation.
- Fixes standard-resource updates reporting success without writing to the database; generated Repositories explicitly mark fields for update, with real MySQL persistence and tenant-isolation regressions added.

### Removed

- Completely removes backend implementations of Agent queries, user delegation, and service-account management, 23 operations, dedicated configuration, key assembly, and six data tables; retains all other business capabilities and complete historical records.

### Changed

- Seed source-register, storage rebinding, source-generation start/stop/status/cleanup, and the sole export chain now have operations, paths, and write policies strictly parsed by the first xtask layer; run directories and requests are restricted to the current backend's `.local-tests` and reject links before Python starts. Private stages receive only a versioned JSON protocol without credential contents and no longer accept direct argv for these formal operations.
- Fresh-target recovery entry points now strictly parse stages, paths, and explicit-write arguments through xtask and call the internal state machine through a versioned private JSON protocol; paths are restricted to the current backend's `.local-tests` before Python starts or locks are created, and private programs no longer receive argv for this public command.
- Backend Node regression tests are centrally discovered recursively by the unified task graph under `scripts/tests/**/*.test.mjs`; complete local checks and CI preflight execute the same set only once in a fixed order. Removes three scattered Python test bridges and adds department-copy and existing-data-copy tests previously absent from gates.
- CI companion-frontend source selection converges on `cargo xtask check ci frontend-source`; xtask validates events, absolute paths, baseline SHAs, and Git refs before starting the private Python implementation. PR candidate OpenAPI explicitly declares and executes API compilation with a fixed CI target, and workflows no longer pass an overridable backend worktree.
- Release-source and remote-CI evidence verification respectively converge on the unified task graph of `cargo xtask check release source|ci`; arguments undergo strict type validation before Python starts or GitHub is accessed, and workflows no longer directly call private scripts to record and verify full-stack source combinations. Both Python implementations accept only the versioned private environment protocol injected by xtask and reject command-line arguments, eliminating duplicate public-option parsing.
- Extended CI cgroup v2 memory acceptance now runs through the same registered task under `cargo xtask check perf cgroup run|cleanup`; the Rust entry point verifies arguments and evidence directories before starting private cross-privilege programs, and `--plan` only displays tasks, write scopes, and resource scopes.
- CI deployment-source and image gates now use the same task graph under `cargo xtask check ci security deployment source|image`; deployment-change detection fails closed, static/Compose/Nginx/Prometheus validation is no longer called repeatedly by preflight or workflows, and workflows retain only fixed tool environments and necessary production-image builds.
- The CI supply-chain source gate now sequentially runs policy, RustSec, and dependency-source checks through the same task graph under `cargo xtask check ci security source`; the security job covers push and all pull request actions, and preflight no longer repeats the same supply-chain policy.
- The Required aggregate gate now reuses the same CI job and plan-output definitions through `cargo xtask check ci required`, strictly parsing GitHub `needs` before checking dynamic gate results; removes the duplicate Python rules implementation.
- Resource-gate replay, export physical objects, and the 168-hour retention fixture all move into CI or recovery groups under `cargo xtask check`; xtask fixes sources for both repositories, retention writes require explicit `--write`, and export receipts are restricted to the current backend's `.local-tests`.
- `cargo xtask build` builds API, Worker, and frontend production directories separately from the same plan and reports all three artifact digests; `--plan` only displays the same task graph. Actual builds verify both worktree fingerprints before and after execution and reject artifacts with mixed sources.
- Full-stack tests manage API and external Workers through a private persistent supervisor for each startup; Windows creates and joins a private Job Object before product code executes, while Unix uses a separate session and process group. Receipts bind scope, startup generation, and two levels of creation identity, distinguish normal stops from fault injection, retain actual product exit codes, and wait for complete process-tree and port cleanup.
- Recovery business proofs bind actual API, Worker, and frontend processes, Cargo build artifacts, frontend production files, target plans, and test runners; final registration generates authoritative context from the database, and the clean current backend independently verifies both product and execution sources, adapter contracts, test details, and all evidence digests.
- External recovery reference drivers gain explicit resource plans, dataset preparation for eleven tenants, exact backup/restore, and independent replica fault injection; recovery business acceptance must read existing data and download existing objects.
- DevEx general timings gain P99, record process-tree peak memory from Windows Job/Linux cgroup and reasons for unavailability, and reject comparisons that mix collection definitions.
- Runtime performance collection separately verifies API/Worker process identities and metrics authentication and preserves failed cycles and session events; home-page and recovery-data preparation use explicit request intervals and fixed identities.
- Runtime loads verify and use only actually selected independent identities and tenants; import measurement generates independently sampled, entirely valid XLSX files offline from fixed real templates, verifies manifests and file digests, and reports preparation time separately. Command process-tree memory still includes preparation.
- Runtime measurement verifies actual builds, processes, listeners, and frontend-asset sources before and after measurement; loads use a fixed identity pool, distinguish ten-tenant imports from system-tenant scheduling, and continue to record capacity rejection as failure.
- Runtime loads follow fixed request pacing and sample-preparation windows based on real rate-limit digests; business waiting counts toward cycle time and throughput. Preparation waiting is reported separately, and rate-limit failures are not retried.
- Reconciles the development-version database initialization baseline and adds explicit `cargo xtask data migrate baseline --write`, while ordinary checks remain read-only; stable-version protection takes effect from v1.0.0 and checks published history to prevent bypass by downgrading.
- Packages support empty capability catalogs while retaining version publication, tenant binding, quotas, and capability extensions; frontend and backend contracts synchronize to the four existing domains.
- Splits long configuration, migration, task, and generator functions, clears registered exceptions, and enables checks for all functions.
- Architecture gates directly verify current resources and tenant-copy catalogs, removing old template paths and transitional dependency graphs while retaining strict tenant-session and database-structure validation.
- Extends full-stack CI with external Workers, S3-compatible object storage, and production-frontend browser flows; release verification binds exact source combinations, the latest run attempt, and success evidence for required jobs.

### Added

- Formal recovery gains an isolated monitoring-delivery lifecycle, using immutable staging to manage local webhook, Alertmanager, and Prometheus process trees and verifying API/Worker bearer metrics, targets, rules, 23/24-hour boundaries, and firing/resolved deliveries; explicitly progresses, fully cleans up, and safely publishes terminal evidence through `cargo xtask check recovery monitoring`.
- Formal-recovery comparisons gain strict B0/B1 source manifests, separately binding exact sources from both repositories, v2 product-build receipts, and B0 tool-adaptation rebuild evidence, and requiring both sides to consume the same published source-export. Generation requires explicit writes; read-only verification neither creates recovery state nor accesses live storage credentials.
- Development performance-data copying gains offline planning and read-only verification, checking complete table catalogs, logical tenant relationships, and object mappings and rejecting active tasks and unknown resources; planning does not copy data or register successful recovery.
- The development-copy maintenance CLI binds actual Cargo artifacts, source differences, and toolchains; object helpers support atomic conditional creation and complete metadata collection, separately recording storage attributes and GET-response differences and retaining exact diagnostics on failure.
- Independent performance environments gain an ordinary-user preparation CLI with planning, explicit execution, and verification; fixes identity pools, minimal permissions, import templates, and message audiences. Unknown-write results retain ledgers and reject replay.
- Background tasks persist the time and result of every claim and retain separate attempt records on retries; the performance helper collector verifies complete execution facts against explicit business IDs, tenants, and resource ownership.
- Adds external backup registration, streaming file/object validation, isolated-recovery data and business validation CLIs, and metrics/alerts for backup age, missing or invalid backups, and recovery results. RPO/RTO targets require measurements in the reference environment and do not indicate a completed drill.
- Runtime status gains backup-collection caches and recovery summaries, distinguishing unknown, available, failed, and stale states; collection timeouts and stale successful timestamps alert independently without affecting API readiness.
- Adds real home-page, API, background-task, and multi-tenant load drivers and reports for DevEx, retaining existing budgets and checking a 10% P95 regression; adds comparison tools for preserving endpoints, menus, permissions, and pages.

## [v0.12.1] - 2026-09-02

### Changed

- Upgrades the Rust toolchain uniformly to 1.98.0, raises the minimum Rust version to 1.98, and continues to use Rust 2024 edition; updates production builder images and pins their official digests.
- Deployment gates verify that the pinned toolchain, workspace minimum version, and production builder image version agree, preventing build-version drift between local environments and CI.
- Authentication, permission, and tenant-context middleware use unified strongly typed HTTP errors, while rate-limit middleware returns responses directly; error observation uses `inspect_err` to accommodate the new Clippy while retaining existing HTTP behavior.
- Removes the obsolete deprecation-warning exemption from application task-interface exports, retaining no old compatibility entry points.

### Fixed

- Removes the sccache remote backend in GitHub Actions that continually had zero hits and produced write errors; only Rust Gate and Windows Smoke persist size-limited local compilation caches, and other jobs no longer install wrappers without benefit. Changes to production images and full-stack verification scripts selectively trigger Extended CI.
- Production-image content gates now export and read filesystems offline instead of requiring a shell in Distroless images, while verifying fixed numeric non-root users and minimal runtime-layer tools.
- The production runtime layer switches to digest-pinned Distroless Debian 13, removing the shell, package manager, and `curl`; API and Worker gain built-in `--healthcheck` to avoid unpatched base-image system packages blocking high-severity vulnerability gates.
- Fixes real full-stack CI still enabling API documentation with a minimal `bin-api` build, causing the application to exit before readiness; this flow no longer compiles unrelated Swagger UI.
- Fixes Cargo CycloneDX custom filenames appending `.json` twice, preventing extended gates from collecting the backend SBOM.
- Upgrades the production runtime image to digest-pinned Debian 13.6 slim, eliminating high and critical vulnerability-gate failures triggered by old Debian 12.15 system packages.

## [v0.12.0] - 2026-09-02

### Added

- Adds a unified DevEx measurement entry point covering Rust cold builds, incremental builds, save feedback, resource generation, resource gates, sccache, frontend fast checks, and production builds, saving reviewable P50/P95 evidence.
- Adds intelligent resource gates based on resource ownership and reverse dependency closure; completes replay of 20 categories of addition, field, permission, relationship, SQL, rename, and deletion cases, automatically falling back to complete gates when necessary.
- Adds AWS-LC outbound TLS integration validation covering JWT, MySQL, Redis, S3, and OTLP links, with explicit validation of native C/C++ sccache launchers.

### Changed

- The root composition crate uses explicit bin features for API, Worker, migration, and maintenance programs; Worker no longer depends on API, and migration and runtime dependency surfaces are isolated by target.
- `ryframe-generator` uses pure offline dependencies by default, enabling database-schema import on demand through the `schema-import` feature; check and preview modes prohibit file writes.
- TLS and JWT cryptographic paths uniformly use the AWS-LC provider; startup fails when a process finds a different provider, with no silent fallback. The project does not claim FIPS certification on this basis.
- `cargo dev` plans API, Worker, migration, and resource actions from complete path sets; configuration changes reuse the last known-good artifacts, stale cycles terminate complete build process trees, and failed candidates restore the previous artifact generation.
- Crate boundaries, function lengths, file sizes, change propagation, and temporary architecture edges are maintained through machine-verifiable policies; application no longer exposes SQL entities or query builders to callers.
- Routine CI merges duplicate initialization and checks, retaining six backend business jobs under a single `Required` aggregate; full-stack, compatibility, bills of materials, and image scans move to scheduled, tag, or manual extended gates.
- README now describes only the current project state and reviewable metrics, no longer retaining historical optimization comparisons.

### Fixed

- Fixes unstable snapshot slots, workspace roots, target directories, and source hashes during parallel resource-gate execution, preventing incorrect reuse or unnecessary recompilation.
- Fixes development-state-machine races involving Windows build-process-tree cleanup, configuration-candidate validation, control-database migration isolation, and late sccache startup.
- Fixes CI runner temporary-directory expressions, deployment directories in merged jobs, and references to withdrawn cryptographic dependencies, restoring complete main-branch gates.

### Validation

- Frontend fast-check P95 is 9.506 seconds, resource-gate P95 is 58.661 seconds, and stale-build cancellation P95 is 369.7 milliseconds; the generator's default dependency closure is 37.
- Targeted API, Worker, and migrate builds, default and `schema-import` generators, complete Rust gates, real MySQL/Redis/AWS-LC TLS integration, and Windows smoke all pass.
- Removes the retired AWS-LC sccache evaluation script and its dedicated tests, retaining scripts still called by CI, xtask, release, or test entry points.

## [v0.11.3] - 2026-08-27

### Changed

- Consolidates application, adapters, and composition-root startup assembly boundaries, retaining an acyclic dependency graph across twelve crates.
- Optimizes local validation, standard-resource generation, CI orchestration, and test builds, unifying development and automation entry points.
- Improves multi-tenant configuration application, data targets, migrations, Repositories, and real full-stack quality gates.
- Streamlines user-facing project documentation, unrelated templates, Qodana remnants, and development-tool configuration.

## [v0.11.2] - 2026-08-23

### Changed

- Consolidates responsibilities and dependency boundaries across twelve crates, with automated gates preventing reverse dependencies across layers.
- Completes asynchronous export based on applied filters, terminal-state deletion, and safe non-production rebuild flows.
- Synchronizes permission catalogs, menu internationalization, standard-resource artifacts, and companion-frontend contracts.

## [v0.11.1] - 2026-08-21

### Fixed

- Pairs the release with the frontend HTTP `202` success-envelope fix, preventing terminal-state deletion from being misclassified as failure.

### Validation

- Keeps the twelve crates, OpenAPI, backend version, and companion-frontend version consistent.

## [v0.11.0] - 2026-08-21

### Added

- Adds bounded asynchronous export based on the last applied filters, idempotent downloads, cancellation, and terminal-state cleanup.
- Adds scope-isolated non-production rebuild plans, ownership checks, and paired frontend/backend release gates.

### Changed

- Consolidates the backend into ten product crates and two tool crates, fixing directed acyclic dependency boundaries.
- Unifies responsibilities for resource generation, Repositories, transactions, configuration, authentication/authorization, and API transport layers.

## [v0.10.0] - 2026-08-19

### Added

- Adds product packages, immutable published versions, a compile-time Capability Catalog, full tenant overrides, and runtime epochs; the platform can preview and atomically apply package changes, with `system.service_accounts` as the first controlled capability.
- Adds persistent MySQL Saga idempotency records for tenant creation, requiring `plan_version_id`, `data_target_key`, and `Idempotency-Key`, and safely resuming in the order provisioning, target fence, capability-resource synchronization, and active/enabled.
- Adds tenant configuration packages without database IDs or sensitive credentials, supporting cross-environment or same-environment upload, preview, merge application, pre-application snapshots, seven-day rollback windows, and configuration-write leases.
- Parameter configuration gains an explicit `portable` flag; only parameters reviewed by administrators and not matching sensitive-key rules may enter configuration packages.
- Adds tiered data-retention services, system-tenant preview and manual-run endpoints, and a safe cleanup schedule at `03:30 UTC` daily; successful tasks, published Outbox entries, scheduling history, export history, logs, import history, and import files are cleaned in batches using independent windows, while background-task and Outbox dead letters are retained permanently.
- Adds resumable, cancellable asynchronous user-import tasks with idempotent creation, private source files, batch cursors, permission and data-scope rechecks, partial success, paginated exception rows, and Excel reports.
- User-import templates now use full department paths and available-department lists generated for the current data scope, no longer requiring or accepting any department database-ID column; old templates are explicitly rejected.
- Adds read-only effective-permission diagnostics, centrally resolving roles, permission sources, menu visibility, data scopes, authorization versions, and cache states from the primary database; database diagnostics still work when Redis is unavailable.
- Adds operational snapshots and 6-hour, 24-hour, and 7-day trend endpoints aggregated strictly by current tenant, covering dependency states, process resources, connection pools, background tasks, scheduling, login, and operation activity.

### Changed

- Database runtime and tenant data-target version requirements are unified at MySQL 8.0.16+; MariaDB remains rejected, retaining the minimum capabilities required for enforced CHECK, `SKIP LOCKED`, and full schema validation.
- Login, refresh, and `GET /auth/context` uniformly return atomic `session_context`; menus are filtered by Capability before RBAC, and protected responses return four snapshot headers for authorization, runtime, data generation, and status.
- Login, request principals, long-running user operations, and permission diagnostics share an authorization resolver, unifying enabled-role, super-role, permission-union, and data-scope semantics.
- API, Embedded Worker, External Worker, and `ryframe-worker --once` share background-task handler assembly, preventing missing import or maintenance-task handlers in different runtime modes.

### Removed

- Removes the synchronous user-import endpoint `POST /api/v1/system/users/import`; callers must migrate to `POST /api/v1/system/user-imports` and query progress and results through import-task resources.

### Fixed

- Login captchas use higher-resolution glyphs, low-interference rendering, and an unambiguous character set to reduce manual recognition errors.
- Service-account seed synchronization accepts historical menus and permissions put into controlled dormancy by tenant Capability, avoiding misclassifying legitimate dormant states as definition conflicts during product-capability model upgrades.
- Uses forward data migration to fix system-administrator and ordinary-user hashes and login states in fresh databases that did not match documented default passwords; updates only records retaining old invalid hashes without overwriting passwords already changed.
- Frontend session refresh invalidates the current CSRF challenge on both success and failure, preventing login from continuing to reuse a mismatched in-memory challenge token after the backend clears authentication Cookies.
- Service accounts, delegation, access auditing, and Agent API uniformly integrate product Capability: unavailable deployment dependencies return stable `501 capability_unavailable`, tenants without the capability return `403 tenant_capability_denied`, and clients read effective capabilities from session context rather than feature-toggle fields in the version endpoint.
- Online-user last-activity updates treat concurrent CAS contention (`Skipped`) or concurrently revoked sessions (`Deleted`) as success, no longer emitting the WARN that login-device metadata is temporarily unavailable or counting these events toward Redis degradation monitoring.
- Tenant request-rate-limit window snapshots support Redis integers, bulk strings, and nil/negative TTL replies when no window is active, preventing partial capacity-endpoint degradation caused by serialization-type mismatches.

### Security

- `/platform` no longer depends on the short-lived Redis idempotency layer; tenant creation and data migration use control-database request digests, unique keys, and state machines as authority, so Redis failures neither bypass nor block persistent Sagas.
- MySQL connections require TLS by default and remove the SQLx RSA feature used only for password authentication without TLS, avoiding a dependency with no fixed version for `RUSTSEC-2023-0071`; remote production databases continue to require `verify_identity`.

## [v0.9.0] - 2026-08-08

### Added

- Background tasks and Outbox gain exponential idle backoff, positive and negative jitter, local/Redis wake hints, and independent lease-recovery cycles; maximum idle polling and lease-recovery intervals are configurable, reducing empty-queue database pressure while retaining low latency for new tasks.
- Adds metrics for task claims, wake transport, listener states, and protocol errors, combining queue depth and oldest-pending-task statistics into batch queries and completing production observability for standalone Workers.

### Changed

- Shared DTOs, authentication Handlers, application configuration, background-task repositories, authorization caches, and export services are split into internal modules by domain responsibility; existing public import paths and HTTP contracts remain unchanged, reducing large-file coupling and future change risk.
- Background-task enqueueing, Outbox delivery, and cross-process Workers uniformly use reliable wake channels; wakeups only shorten waiting, while the persistent database queue remains the sole source of truth.
- Production Compose binds API startup conditions to standalone Worker health and tightens container build contexts, health probes, and deployment documentation.

### Fixed

- Fixes OpenAPI path macros not being explicitly registered from new modules after authentication Handler splitting, restoring complete contract exports for password reset, CSRF, logout, refresh, current user, and WebSocket tickets.
- Fixes potential numeric overflow in background retry backoff at extreme failure counts and improves repeat-execution safety of message-recipient soft-delete migrations and initialization data.
- Fixes inconsistent waiting strategies in Worker idle polling, lease recovery, and dependency-recovery paths, avoiding continuous high-frequency queries when idle and recovery delays amplified by fixed polling intervals.

### Validation

- Joint gates pass the Cargo feature registry, formatting, dependency policy, permission routes, production-target Clippy across the entire workspace, deterministic OpenAPI/MySQL snapshots, and complete frontend quality checks.
- Removes unmaintained Qodana, source-comment-language, and old architecture-scanning scripts and their CI entry points; the unified local entry point becomes `cargo xtask check --scope all`, while frontend `pnpm check` runs current effective gates.
- This version is jointly released with frontend `v0.9.0` using annotated tags with the same name; stable Releases retain only GitHub-generated source ZIP/TAR archives.

## [v0.8.0] - 2026-08-07

### Added

- Message inboxes gain recipient-level soft deletion and batch-delete endpoints; deletion is isolated by tenant and current user, supports idempotency, and excludes deleted messages from lists, unread counts, catch-up, replay, delivery, and read updates.
- SQL logging gains `off`, `slow`, `summary`, and `full` modes and configurable slow-query thresholds; structured fields enter the tracing output pipeline uniformly, and development environments log only slow queries reaching 200 ms by default.
- Server information now uses a process-wide shared sampler that continuously publishes snapshots after startup; monitoring endpoints only read the latest samples, no longer recreating `sysinfo::System` or waiting for CPU sampling on each request.

### Changed

- Delivery, read, and deletion have separate semantics: clients acknowledge delivery after catch-up or receiving WebSocket messages, mark messages read only when opening details, and reading also guarantees delivery.
- Message WebSocket, inbox repositories and services, background tasks, routing, SQL logs, and database tracing are split into internal modules by responsibility, preserving compatibility of existing public import paths.
- The system retains only `/api/v1/ws` with one-time tickets, Redis wakeups, and inbox catch-up; removes the deprecated generic WebSocket implementation and unused email crate.
- Readiness probes, background empty polling, and logs without state changes are lowered to `DEBUG`; dependency deterioration, recovery, and completed work are logged only at state boundaries, avoiding continuous output during normal operation.
- Test, benchmark, and acceptance assets remain only in the maintainer's local ignored directory `.local-tests/backend/`; remote repositories, production source, Cargo manifests, and CI no longer contain or invoke test entry points.

### Fixed

- Fixes SQLx short statements missing SQL bodies, complete statements being truncated, and the same query bypassing the unified writer or being logged twice, while keeping bind parameters, connection strings, and sensitive information out of logs.
- Fixes technical write requests such as WebSocket tickets being misclassified as missing business-transaction audits and continuously producing warnings; warnings remain only on routes requiring business-audit binding.
- Fixes overly broad development CORS origins and quality issues found by Rust static analysis; development origins are restricted to local ports used by Vite.

### Security

- SQL logs do not record bind-parameter values by default, and OpenTelemetry database Spans do not send raw SQL; production SQL logging defaults to disabled and emits a safety notice when summary or full mode is explicitly enabled.

### Validation

- Release gates cover formatting, source hygiene, dependency policy, permission routes, architecture boundaries, production-target Clippy across the entire workspace, OpenAPI snapshot consistency, and unified local behavior tests.
- Stable releases continue to contain only GitHub-generated source ZIP/TAR archives, jointly defined by immutable backend and frontend commits with the same tag name, without custom attachments.

## [v0.7.0] - 2026-08-05

### Added

- Adds a single explicitly confirmed v0.7 runtime-acceptance entry point, strictly serially verifying dual-instance messaging, read-replica removal after three consecutive failures and recovery after two successes, and complete parent-child traces and failure recovery with a real OpenTelemetry Collector.
- Runtime acceptance generates a separate Docker project and ownership token for each run; containers, networks, and volumes must match both project and token before fault injection or cleanup, and source commits, lockfiles, binaries, and immutable image identities are recorded.
- Runtime database-topology responses gain `consecutive_failures` and `consecutive_successes` to directly observe replica-removal and recovery thresholds.

### Changed

- HTTP success, error, message-protocol, and background-task semantics uniformly use complete Chinese and English resources; language negotiation, `Content-Language`, and `Vary: Accept-Language` form a closed loop within the current contract, without old fixed-Chinese response branches.
- Message WebSocket connections can deliver and trigger catch-up only after the hello frame successfully enters the bounded send queue; delivery is at least once before ACK persistence, and clients logically merge by message ID. New connections after ACK persistence must maintain zero delivery across a complete catch-up cycle, no longer assuming raw-frame exactly-once behavior as part of the protocol.
- OTel cross-process propagation persists and restores both `traceparent` and `tracestate`; API, standalone Workers, and Outbox use the same trace context, with bounded failure and resumed export during Collector interruptions.
- Read-replica health selection explicitly uses consecutive-probe thresholds: routing continues through the first two failures, removes the replica on the third, keeps it removed after the first recovery success, and rejoins only after the second, without the old immediate-switch pattern.
- The idempotency Redis key namespace upgrades one-way to `ryframe:v0.7:idempotency:`, without reading or dual-writing v0.6 keys.
- OTel acceptance reuses binaries built during the messaging stage; the three runtime stages no longer repeat the same Cargo build.
- Completes staged upgrades of `rust_xlsxwriter`, `validator`, `tower-http`, `jsonwebtoken`, and `syn`; `base64` and `password-hash` remain in the audited stable dependency families.

### Fixed

- Fixes database events failing to form real OTel child Spans during reentry into `tracing::Layer::on_event`; directly exports client Spans with actual durations from the current context, uniformly using current database semantic attributes without recording SQL bodies.
- Fixes async tasks losing API→Worker/Outbox parent links by querying context only from the current tracing Span; uniformly generates and persists W3C `traceparent/tracestate` from the current OTel Context.
- Fixes incorrect acceptance-evidence judgments caused by Windows PowerShell 5.1 handling generic lists and binary readiness responses inconsistently, retaining Collector logs and trace evidence when OTel fails.
- Fixes microsecond database timestamps being rounded up to the next second when written into `DATETIME` without fractional precision, briefly preventing committed messages from entering inboxes; all eight message and inbox timestamp columns migrate uniformly to `DATETIME(6)`, with test fixtures, baselines, and SQL snapshots using the same precision.
- Fixes Windows reusing temporary ports released by acceptance scripts for outbound connections, causing intermittent subsequent API binding failures; removes duplicate port-allocation implementations across the three stages, uniformly choosing non-ephemeral ports and performing real loopback-bind verification.

### Security

- Substages accept only fixed internal confirmation tokens, exact loopback URLs, fixed in-repository evidence roots, and exclusive evidence writes; process termination verifies both start time and executable to prevent killing unrelated processes after PID reuse.
- Every substage restores the complete process environment before exit; Docker context must point to the local daemon, existing resources in a same-named project reject takeover, and any ownership mismatch safely stops cleanup.

### Validation

- Local gates cover workspace-wide formatting, architecture, Clippy, `cargo check --workspace --all-targets`, workspace library and binary tests, OpenAPI snapshots, and v0.7 runtime-acceptance policy tests; behavior requiring real MySQL/Redis is covered by isolated Docker acceptance.
- Formal v0.7 Docker acceptance passes the messaging, read-replica, and OTel stages serially from a clean commit; all atomic evidence is `passed`, with zero failed assertions, evidence-collection errors, cleanup errors, or residual isolated Docker resources.
- Stable releases continue to contain only GitHub-generated source ZIP/TAR archives, without building or signing platform images or uploading SBOMs or other custom attachments.

## [v0.6.0] - 2026-08-03

### Changed

- Adds two local isolated acceptance entry points: the general script uses a unique Docker project for MySQL, Redis, RustFS, workspace-wide tests, and standalone Worker/API smoke; the FILE-A script uses fixed legacy-database fixtures and real MD5 collision objects to verify backfill, draining, migration lockout, and final structures. Both reject remote Docker contexts and clean temporary resources within restricted scopes.
- API and standalone Workers use the same logging initialization; log levels, formats, output targets, and retention counts become strongly typed startup configuration. File mode rotates daily and actually limits historical file counts; production configuration and container orchestration default to stdout to avoid creating local log directories in read-only containers.
- API and standalone Worker `/readyz` now read time-limited in-memory snapshots generated by background dependency probes, with no SQL, Redis, or object-storage network calls on request paths; expired snapshots mean not ready, and Worker explicitly does not use object storage as a readiness condition.
- User authorization-version fields are unified as `authorization_version`, with JWT and WebSocket tickets using the new name; adds tenant-level `authorization_epoch`. New installations use new columns directly, while incremental migrations only rename historical columns losslessly and explicitly prohibit rollback.
- HTTP error boundaries remove duplicate error types and bidirectional conversions; domains uniformly use `ryframe-kernel::AppError/AppResult`, and Handlers use only the types in `ryframe-api::http`, `HttpAppError/HttpResult`, for one-way adaptation to HTTP responses. Architecture gates prevent old symbols from returning.
- Messaging gains a single strongly typed `messaging` configuration explicitly injected by the composition root; ticket lifetime, retention, per-user connection count, outbound queues, and recipients per message are no longer hardcoded. Publication transactions use `INSERT … SELECT` to fix recipients within MySQL, rolling back and rejecting when exceeding `max_recipients_per_message + 1`; disabling messaging explicitly makes REST, tickets, WebSocket, and message tasks unavailable.
- Removes `LoggedRepo`, which produced no logs, with Services and code-generation templates storing concrete Repositories directly; also removes old `DatabaseCluster::new/with_sources/with_sources_and_replica_health` constructors. Architecture gates prevent these empty wrappers and implicit construction logic from returning.
- Swagger UI uses a Rust crate matching the utoipa version to embed all HTML, CSS, JavaScript, fonts, and icons at compile time; removes CDNs, external validators, external-resource fallbacks, and compatibility redirects. API documentation remains forcibly disabled in production.
- Tightens document and global content-security policies: scripts allow only same-origin resources without `unsafe-eval`; only Swagger UI pages relax inline styles because runtime components require them, and all document static assets are served from the same origin.
- Rate-limit configuration removes the old `refill_per_sec` parameter; Redis and memory implementations uniformly use `window_secs` fixed windows, preventing runtime-mode changes from altering rate-limit semantics.
- OTel export failures uniformly record fixed-stage logs and metrics without dynamic labels; adds deterministic acceptance using nonresponding loopback endpoints, ensuring business operations and readiness are unaffected and exit flush waits at most five seconds.
- Download responses emit only RFC 5987 `filename*` UTF-8 filenames, removing the ASCII `filename` fallback for old clients.
- JWTs must carry a nonempty `sid` to decode, removing rejection branches dedicated to old tokens; also removes unused old numeric error codes and compatibility defaults in refresh-session serialization.
- The database migration library removes the `run` compatibility alias, and callers uniformly use `up/status/verify`; architecture gates prohibit both migration aliases and Serde field aliases from returning.
- Operation-log and login-log export filters uniformly accept only `name`; old `oper_name/user_name` request fields explicitly fail.
- Production environment-variable examples now use internal image digests built and audited by deployers from stable-tag sources, without references to GHCR or release manifests.
- Announcement write and response fields are unified as `content_markdown`, removing the old `content` contract; content limits become 1–60,000 UTF-8 bytes, with the sole runtime policy published to the frontend through OpenAPI extensions.
- All configuration errors and unknown internal errors return generic `500` messages, with internal details logged only; removes the deprecated `X-XSS-Protection` header and misleading claims about global XSS filtering.
- Response envelopes cover all `/api` paths, with unknown-version and unversioned paths uniformly returning JSON `404`; removed top-level `msg/rows/total` response fields are explicitly rejected.
- Response envelopes process JSON only through a bounded 16 MiB buffer; WebSocket upgrades, successful non-JSON responses such as files/images, and enabled OpenAPI/Swagger documentation retain original streaming responses. Successful non-envelope JSON fails closed, while error normalization retains `Retry-After`, CORS headers, and response extensions.
- `/api/v1/version` becomes a strongly typed unified response included in OpenAPI; architecture gates also verify bounded production-response buffering, reuse of original response Parts, and that every public API JSON 2xx Schema uses only the current unified envelope.
- Oversized request bodies and request timeouts return standard error envelopes; online-user pagination passes `ValidatedPageQuery` directly to Service, with new architecture guards prohibiting Service or Repository from again accepting raw `page/page_size/offset` values.
- Removes 11 unbounded lists and associated configuration, services, OpenAPI, and tests, without old-interface compatibility; role and user selectors uniformly use the bounded `/options?q&limit` contract, performing stable prefix queries by tenant, permissions, and data scope.
- Idempotency storage keys isolate only tenant, user, and original client key; request fingerprints uniformly bind method, actual normalized path, sorted query, and body SHA-256, returning `409` for the same key with different fingerprints.
- Stable releases converge on immutable identity verification across both repositories and source-only GitHub Releases; joint release workflows no longer repeat compilation, testing, or snapshot generation, instead verifying successful push CI for exact commits in both repositories, matching checked-in OpenAPI, and unmoved tags. Removes duplicate snapshot jobs and `stable-release` manual environment approval.
- Removes Nightly tags and prerelease workflows, allowing only the backend stable joint-release controller to create Releases; also removes multi-architecture container builds, GHCR pushes, SBOMs, Cosign signatures, OCI release manifests, and all custom attachments. Stable releases retain only GitHub-generated source ZIP/TAR archives.
- Backend CI retains only Linux static quality gates and dependency-security audits, removing Windows nextest and scheduled coverage jobs that compiled repeatedly; Rust tests, coverage, and OpenAPI/MySQL snapshot generation all run locally, and CI no longer recompiles through `cargo run/check/build/test/nextest/llvm-cov`.
- Production secrets are uniformly injected through `APP_*` environment variables or external secret managers, with configuration files allowing only empty placeholders; removes `ENC[...]`, `CONFIG_MASTER_KEY`, and dual-format AES configuration encryption/decryption, without old-configuration fallback.
- Public API prefixes are generated uniformly from `ryframe-api::http` using `API_PREFIX` and `api_path`; OpenAPI publishes the same prefix to the frontend through `x-ryframe-api-prefix`, and deployment environments configure only the API origin rather than maintaining version paths.
- The API layer owns all public response DTOs and performs exhaustive owned conversions of Service models; Utoipa is removed from the runtime dependency graphs of Service, Generator, and Utils. File Service returns only storage identity, and private download URLs are constructed only at the API DTO boundary.
- Minimum and maximum Cargo feature combinations are centrally registered in `config/feature-matrix.json`; CI reuses Cargo metadata for lightweight integrity guards, while complete combination compilation runs only through explicit local `cargo xtask feature-matrix`.
- Configuration caches now use database-persisted tenant-namespace `BIGINT` authoritative versions; configuration writes, version increments, and Outbox entries commit in one transaction. Redis uses fixed same-slot version/values keys and exact decimal version comparisons without floating point.
- File-upload runtime uses only SHA-256 and the `upload_status` state machine; old MD5 verification and `del_flag = '3'` cleanup are isolated in one-time maintenance binaries. The final one-way migration fails closed if prerequisite data has not converged and removes old columns and indexes on success.
- All audited business write commands commit business changes, authorization versions, and `audit.operation` Outbox entries within the same MySQL transaction; failure auditing uses separate short transactions without overwriting original business errors, and Service-layer architecture guards prohibit bare commits from returning.
- Outbox and background tasks gain complete deduplication, lease heartbeats, crash recovery, exponential backoff, dead states, and manual retries; export tasks use stable primary-key cursors, creator-permission and data-scope rechecks, deterministic object identifiers, and cleanup after 24-hour expiry.
- Idempotency fingerprints bind tenant, user, method, actual canonical path, sorted query parameters, and body SHA-256; the same key with a different fingerprint returns `409`, and local acceptance with 50 concurrent requests confirms side effects execute only once.
- File-maintenance tools use real collision samples to verify that MD5 cannot serve as the basis for reuse, and complete acceptance for dual digests, CAS, compensation ownership, two-stage cleanup, and downloads using original filenames and Content-Type from the database.

## [v0.5.1] - 2026-08-02

### Changed

- GitHub Actions uniformly upgrade to Node.js 24-compatible versions, eliminating Node.js 20 deprecation warnings on hosted runners.
- GitHub Actions no longer invokes `cargo test`; Linux handles formatting, architecture, and Clippy, Windows runs library tests once through `cargo nextest`, and doctests run only locally.
- Simplifies Cargo cache keys and removes low-benefit `target` caches, avoiding large build-artifact uploads that slow routine CI.
- OpenAPI and MySQL snapshot generation moves from every push to manual checks and stable-release gates; routine commits run only necessary consistency checks.
- Removes low-benefit runtime and Snowflake-algorithm smoke checks from routine CI, shortening backend feedback time.

### Fixed

- Fixes `setup-node` automatic caching, pinned SBOM Action versions, and dual-repository gate reuse in Release, preventing release failures due to invalid cache configuration or Action references.
- Fixes duplicate jobs, missing Linux test jobs, and expired Action runtimes in CI workflows.
- Upgrades `event-listener 5.4.1`, which had a concurrent memory-safety defect, to the fixed `5.4.2`, eliminating the `RUSTSEC-2026-0221` audit warning.

### Validation

- Routine backend gates cover formatting, architecture, Clippy, Windows `nextest` library tests, and dependency-security audits; `cargo test` remains only a local development command.
- Stable releases continue to verify dual-repository versions, fixed commits, OpenAPI contracts, MySQL snapshots, and exact CHANGELOG tag descriptions.

## [v0.5.0] - 2026-07-18

### Added

- Adds scheduled public-internet observation explicitly bound to deployed RCs and automatic stable-promotion workflows, continuously verifying the HTTPS home page, frontend/backend build SHAs, `/livez`, `/readyz`, `/api/v1/version`, scheduled heartbeats, and 48-hour Deployment evidence.
- Adds `/livez` and `/readyz`, providing pure process-liveness checks and readiness checks for MySQL, required Redis, and necessary object storage respectively.
- Adds `GET /api/v1/auth/csrf`, HttpOnly refresh Cookies, stable `sid`, Redis refresh families, optimistic-transaction atomic rotation, a five-second concurrency grace period, and replay revocation.
- Adds a Docker Compose test stack for MySQL 8.4, Redis 7, and RustFS, along with tests for empty-database baselines, old-structure upgrades, repeated migrations, seed idempotency, and partial-structure rejection.
- Adds trusted-proxy CIDRs, scoped idempotency, dependency-degradation and rejection metrics, unified upload limits, and stable joint-release gates.

### Changed

- Automatic stable promotion allows only explicitly enabled dedicated release accounts to create annotated tags in both repositories; the default path retains protected Environment manual approval, while the automatic path still runs complete joint-release gates.
- Backend builds embed the full source commit through `RYFRAME_BUILD_COMMIT`, and `/api/v1/version` returns `source_commit`, preventing RCs not actually deployed from being recorded as observed versions.
- The pnpm initialization Action in release workflows upgrades to `pnpm/action-setup@v6` (Node.js 24 runtime), with architecture and unit-test gates preventing fallback to old versions.
- Backend and frontend uniformly upgrade to `v0.5.0`, using the same Git tag and release window; the old JSON refresh protocol is incompatible.
- Rust Migrator and the idempotent Seeder become the sole executable schema sources, while complete SQL becomes a read-only snapshot generated from migrations and verified by CI.
- Access tokens and CSRF challenges are stored only in page memory; refresh, logout, and multi-tab recovery now use Cookies, CSRF, and Redis session coordination.
- Production Redis is fixed as required and must use persistence and `noeviction`; refresh, revocation, idempotency, and distributed-lock failures fail closed.
- Tightens rate-limit keys, trusted client IPs, CORS credentials, Nginx TLS/security headers, private upload/download, log redaction, and process metrics according to production boundaries.
- Ordinary files, avatars, and request timeouts are unified at 10 MiB, 5 MiB, 120 seconds for uploads, and 30 seconds for ordinary APIs, with stable `413` responses when limits are exceeded.
- Request authorization resolves roles and permissions from MySQL on every request, no longer using permission caches that could retain stale authorization after failed Redis deletion.
- Runtime unavailability of MySQL, Redis, captcha storage, or object storage uniformly returns `503`; object downloads return `404` only when metadata or objects are actually absent.
- Development/test database reset drops and recreates the database after target-database confirmation, then runs Migrator and Seeder, preventing removed old tables from blocking empty-database baselines.
- Runtime smoke in CI and Release explicitly uses the primary database as the generator data source and verifies that the named business-data-source set is empty, without relying on extra test databases.
- RC, stable, and frontend/backend Nightly GitHub Releases uniformly become general source releases, retaining only GitHub-provided Source code (zip) and Source code (tar.gz); Nightly updates only after successful `main` CI in the corresponding repository, and release gates no longer depend on production API URLs.
- Annotated-tag descriptions and GitHub Release bodies for RC, stable, and Nightly are generated exactly from the current-version CHANGELOG in the corresponding repository; project-level Releases list both backend and frontend updates and no longer allow version-only titles or empty descriptions.

### Removed

- Removes Cargo features, SQLx drivers, runtime branches, configuration, code-generator mappings, tests, and current documentation support for PostgreSQL and SQLite.
- Removes the database `driver` configuration field and environment variable; primary databases, replicas, and named business data sources are fixed to MySQL.
- Removes `HotConfig`, configuration watchers, `reload_hot`, public exports, and related tests; configuration changes require process restarts.
- Removes plaintext `refresh_token` from responses and requests, `RefreshRequest`, frontend token localStorage APIs, and old session-compatibility logic.
- Removes the old `/health` route, Nginx `/uploads/` alias, global idempotency layer, and public direct file/avatar links.
- Removes dedicated Rust integration tests, Runtime Smoke generator checks, and release-gate configuration for the local `ryframe_device` database.
- Removes custom backend executable archives, frontend dist, OCI/GHCR images, CycloneDX SBOMs, SHA-256 manifests, and RC-observation proof attachments; Release reruns clean existing custom attachments for the target tag.

### Fixed

- Development configuration explicitly allows `http://localhost` and `http://127.0.0.1`, preventing authentication Origin guards from incorrectly rejecting login through local Vite proxies.
- Fixes false schema-validation reports after full-database resets caused by MySQL 8.4 information-schema unsigned-field decoding, column-type parsing with parentheses in comments, and foreign-key action parsing across blank lines.
- Fixes non-root RustFS users being unable to write to temporary `/data` volumes in the Docker Compose test stack, preventing object-storage smoke from starting.
- Fixes isolated MySQL test-database destructors waiting across Tokio runtimes for pool closure, causing integration tests to hang indefinitely after completing assertions.
- Fixes trigger and foreign-key alteration syntax in MySQL 8.4 integration tests and serializes Redis global fault injection to prevent parallel-test interference.
- Fixes the Linux-only process-thread metric collection branch triggering strict Clippy `collapsible_if`, causing remote CI to fail early.
- Migrates Nginx TLS templates to standalone `http2 on` syntax, with deployment gates rejecting deprecated `listen ... http2` configuration.
- Fixes configuration tests clearing only some environment variables, allowing RustFS and database overrides injected by Release jobs to contaminate static configuration loading; tests now isolate all `APP_*` configuration overrides uniformly.

### Security

- Refresh Cookies are fixed to host-only, `HttpOnly`, `SameSite=Lax`, and `Path=/api/v1/auth`, with mandatory `Secure` in production and an absolute session lifetime of seven days from login.
- Login, refresh, and logout uniformly validate short-lived signed CSRF challenges; authentication endpoints cannot enter idempotency caches, and logs do not record Authorization, Cookies, CSRF, tokens, passwords, or query parameters.
- Every access-token validation verifies `sid` liveness in Redis; Redis failures return `503`, and confirmed replay revokes the entire refresh family.
- Only trusted Nginx peers may supply forwarding headers; forged XFF on direct connections no longer affects client IPs, login rate limits, or audit logs.
- All avatars and ordinary files remain private and are downloaded through backend Bearer authentication; both fixed-length and chunked uploads enforce server-side limits.
- Object-storage startup checks reject public bucket-policy expressions such as anonymous `Principal` and `NotPrincipal`, preventing complex IAM rules from bypassing private-file constraints.

### Validation

- CI statically validates automatic-promotion workflow YAML, the 48-hour threshold, HTTPS probes, heartbeat-interruption resets, CHANGELOG tag descriptions, frontend-first pushes, and dedicated-bot authorization boundaries.
- Stable tags must be on `main`, frontend/backend workspace/package/OpenAPI versions must match the tag, and an RC of the same version observed for at least 48 hours must exist.
- Backend release gates cover fmt, Clippy, complete MySQL tests, Redis/object-storage smoke, migration upgrades, Seeder, application smoke, ShellCheck, dependency audits, and backup/restore drills.
- Frontend release gates cover contract, source architecture, typecheck, lint, unit/coverage, E2E, and bundle budget, and verify backend OpenAPI from the same tag.
- Release gates reject lightweight tags, empty CHANGELOG version sections, version sections without actual updates, and tag annotations not exactly matching the corresponding full version notes; frontend/backend tag object IDs and commit SHAs are locked and reverified before release, and publication verifies that Release bodies match generated files and custom-attachment counts are zero.

## [v0.4.2]

### Added

- Supports selecting absolute output directories outside the project during frontend code generation, preventing generated files from being written directly into the current project.
- Improves primary-database, replica, RustFS object-storage, and protected-avatar access flows.

### Fixed

- Fixes production avatar-upload timeouts, old avatars still displayed after upload, and object-download failures.
- Fixes private-object-storage avatars failing to display under strict CSP.
- Fixes code-generation endpoints being out of sync with frontend OpenAPI contracts.

### Changed

- The code-generation disk-write endpoint now accepts a separate external absolute output root and rejects the current project directory and its parent/child directories, preventing generated files from entering the running project directly

### Fixed

- Private-object-storage avatars display after authenticated frontend download; `blob:` images are allowed by default in line with strict CSP, preventing avatars from showing old content after successful upload
- Nginx gains separate request-body, connection, send, and read timeout configuration for avatar uploads, preventing production proxies from prematurely disconnecting upload requests
- File-download `sys_file` metadata is now read from the primary database, fixing immediate downloads returning 404 after successful avatar uploads in primary/replica architectures

## [v0.4.1] - 2026-07-17

### Changed

- Workspace crates and OpenAPI `info.version` uniformly upgrade to `0.4.1`, keeping release tags, runtime versions, and checked-in contracts consistent

### Fixed

- Release workflows explicitly set the Git initialization default branch, eliminating default-branch warnings emitted by `actions/checkout` during tag releases

## [v0.4.0] - 2026-07-17

### Added

- Adds explicit `ActorContext`, unifying tenant, actor, and data-scope inputs for Services and adding corresponding template contracts to the code generator
- Adds tests for tenant lifecycles, online-session tenant isolation, cache policies, LRU eviction, null protection, and concurrent cache-miss protection
- Adds a standalone `ryframe-storage` crate, centralizing object-storage ports, local/RustFS/MinIO/S3 backends, path validation, and SigV4 signatures
- Adds tests for safe object-storage paths, URL encoding, deterministic signatures, and compensation after file-metadata failure
- Adds `DatabaseCluster` primary/named read-only replica topology, fallback without replicas, atomic round-robin, and real read/write routing tests across three independent databases
- Adds named business-data-source configuration and explicit resolution, allowing the code generator to select a business database by name for table-structure reading
- Adds a first-class RustFS configuration backend, startup bucket validation, dynamic runtime health status, and a local operations guide
- Adds captcha challenge, glyph, and rendering-module tests covering arithmetic symbols, UTF-8 layout, and invalid inputs
- Adds narrow `PrincipalResolver` and `DatabaseMonitor` ports and SeaORM database-monitor adapter tests
- Adds a `syn`-based compile-time route-permission catalog, embedding complete `#[perm(...)]` permission codes in release artifacts
- Adds Redis cursor-scan and pattern-based batch-deletion wrappers; production caches and online sessions no longer call blocking `KEYS`
- Adds a deterministic `export_openapi` tool and canonical `openapi/openapi.json` snapshot; CI checks differences and uploads a separate contract artifact
- Adds OpenAPI contract tests covering 89 paths, 119 operations, 155 schemas, 34 query operations, success responses, and write-request bodies
- Adds the OpenAPI `x-ryframe-menu-routes` contract; CI also checks default menu SQL, route-key backfill migrations, and the frontend page registry
- Adds the OpenAPI `x-ryframe-password-policy` contract and generated frontend policies; CI verifies that all new-password entry points use the same rules
- Adds runtime CI jobs with real MySQL, Redis, and RustFS services, running health, authentication, permission, menu, monitoring, object upload/download, and OpenAPI smoke after initializing default data

### Changed

- CI security audits now install and strictly run `cargo audit --deny warnings`, removing Node 20 actions and dependencies on additional Checks write permissions
- CI Rust build caching now uses official `actions/cache@v5` and explicit keys isolated by operating system, architecture, job, and Cargo manifests, eliminating Node deprecation warnings from third-party cache Actions
- CI explicitly sets the Git initialization default branch, eliminating default-branch warnings emitted when `actions/checkout` creates temporary repositories
- Runtime Smoke explicitly orchestrates MySQL, Redis, and RustFS startup, health checks, log audits, and cleanup, using nondefault test credentials and eliminating container-environment warnings
- Release workflows extract the exact version notes corresponding to the triggering tag, retaining an empty `Unreleased` section and no longer publishing incorrect sections
- Uniformly requires all `pnpm` commands to run in the separate frontend directory `ryframe-vue3`; backend source gates reject root-level `.pnpm-store`
- Repositories all explicitly receive `tenant_id`; task-local remains only for consistency checks inside HTTP requests
- Database configuration becomes explicit `[database.primary]` and `[[database.replicas]]`; commands and consistency-sensitive reads use the primary, while ordinary queries round-robin across read-only replicas
- Startup runs migrations only on the primary and connects, probes, and validates all configured replicas before receiving traffic; failed replicas are no longer silently skipped
- Splits user Services/Handlers into commands, queries, roles, password reset, CRUD, and import/export, and moves tenant-initialization transactions into dedicated Repositories
- Splits `ryframe-core` cache implementation into backend, permission cache, protected entries, policies, cache-miss protection, and warmup submodules, keeping public exports centralized
- Online users, forced logout, and session blacklists uniformly use tenant-scoped keys; passwords and `authorization_version` update in the same transaction
- Moves file public-URL selection from Repositories into `FileService`; the application composition root explicitly constructs storage backends, and invalid RustFS/S3 configuration no longer silently falls back to local storage
- Role permissions and data scopes become resource-based full-replacement endpoints, with data-scope fields and department associations updated in the same transaction
- User profile, role, and status endpoints are split by responsibility; user creation and initial roles commit in one transaction, and user-role replacement becomes a uniform atomic Repository operation
- Permission types become serializable enums; APIs strictly validate invalid permission types and user states
- Captcha implementation splits into challenge, glyph, and image-rendering modules, and the public module no longer contains a single oversized implementation file
- Authentication middleware delegates principal resolution to `AuthService`; the monitoring module receives database-monitor ports through the composition root, and `ryframe-auth` and `ryframe-monitor` no longer depend on `ryframe-db` or SeaORM
- `AuthService` splits into session, identity authorization, principal resolution, and brute-force protection; login, refresh, and current-user endpoints reuse the same authorization-loading flow
- Permission models, tree building, and use-case orchestration split into separate modules; API explicitly passes the compile-time catalog for permission synchronization, which no longer reads Rust source at runtime
- Menus split into model and hierarchy validation, and departments into command/query/model; menu types become a strongly typed `M/C/F` enum
- `ConfigService` handles configuration-cache cleanup, and Handlers no longer operate Redis directly; department-reference checks move into Repositories
- Stable Service outputs, monitoring states, file uploads, and multipart forms all enter OpenAPI components; pagination-query macros uniformly generate `IntoParams`
- Snowflake IDs in JSON responses uniformly serialize as strings, allowing frontend query, request-body, and response types to be generated directly from canonical snapshots
- Pagination-query macros generate both `ListQuery` and `FilterQuery`; list, full-list, and export queries uniformly map to named Service query parameters
- Menu pagination moves into Repositories, and code-generator table filtering and pagination into Services; Handlers no longer load complete collections and slice them
- Post-login password changes, password resets, and initial tenant-administrator passwords uniformly use a strong-password policy of 8–72 visible ASCII characters; personal password changes increment `authorization_version` to invalidate old sessions

### Fixed

- Fixes RustFS integration tests hardcoding default credentials separately from application and CI object-storage configuration; test clients uniformly read the `APP_OBJECT_STORAGE_*` contract
- Fixes `caching_sha2_password` failing TCP authentication after Runtime Smoke disables MySQL TLS; probes continue to cover actual application connection protocols and retain the zero-diagnostic-log gate
- Fixes MySQL startup migrations creating duplicate menu indexes when complete SQL had already created them but migration history was empty, adding preset-index regression tests
- Fixes redundant borrowing in rate-limit key formatting detected by Rust 1.97.1 and applies an auditable patch to `proc-macro-error2 2.0.1` to eliminate future-incompatibility warnings
- Upgrades `crossbeam-epoch`, `calamine`/`quick-xml`, and `spin`, fixing newly disclosed memory-safety and XML denial-of-service vulnerabilities and removing withdrawn-version warnings
- Fixes non-string values becoming unreadable after cache policies, local cache capacity not taking effect, and expired keys still being returned by `exists`/`keys`
- Fixes SeaORM non-auto-increment tenant primary-key updates not persisting, N+1 role-assignment queries, and inconsistent frontend/backend tenant contracts for password reset
- Fixes local object-key directory traversal risks, ineffective S3 regions, and potentially inconsistent signature times; compensates for failed file-metadata writes by deleting uploaded objects
- Fixes missing captcha minus signs, incorrect UTF-8 width calculation for multiplication signs, and possible panics from empty-dimension inputs
- Fixes brief dirty reads caused by asynchronous cache invalidation after department/menu writes and inconsistent validated and persisted menu `route_key` values
- Fixes non-atomic captcha Redis retrieval/deletion allowing concurrent reuse of the same answer and silently ignored cache-write failures
- Fixes contract discrepancies modeling announcement creators, personal-profile departments, and uploaded-file IDs as JavaScript `number` in OpenAPI
- Fixes dictionary, post, role, and announcement filters being ignored in list or export flows
- Fixes menus and code generators performing in-memory pagination in HTTP Handlers, leaking layered responsibilities
- Fixes personal centers, password resets, and tenant creation maintaining different password rules, and old tokens remaining usable after password changes
- Fixes deployment smoke scripts still accessing old `/system/permissions/tree`, and returns failure when database initialization encounters SQL or default-account update errors
- Fixes expected 4xx authentication/permission responses being recorded as telemetry warnings, causing false failures in the runtime zero-warning gate; 5xx responses remain errors

### Security

- Captcha answers are no longer logged; operation logs recursively redact fields such as passwords, tokens, captchas, and client secrets
- User and tenant Services centrally prevent self-disabling, self-deletion, super-administrator modification, and unauthorized grants of super roles
- RustFS/MinIO/S3 backends keep buckets private by default and no longer automatically write public-read policies during initialization
- New passwords uniformly require uppercase and lowercase letters, digits, and special characters, rejecting spaces, control characters, and non-ASCII input with inconsistent frontend/backend length semantics

### Validation

- Source hygiene, permission routes, architecture boundaries, `cargo fmt`, and full-workspace `check`/`clippy -D warnings` all pass
- Full-workspace tests and documentation tests with `RUSTDOCFLAGS=-Dwarnings` pass; only explicitly allowlisted external MySQL and RustFS/S3 integration tests remain
- `cargo llvm-cov --workspace --fail-under-lines 55` passes with 69.17% line coverage
- Local `cargo audit --no-fetch --deny warnings` and `cargo deny check licenses bans sources` checks pass; CI fetches the latest advisories online and treats warnings as errors

---

## [v0.3.1] - 2026-07-15

### Added

- Adds source-hygiene and architecture-boundary checks covering workspace dependencies, Handler/Service layering, route macros, and OpenAPI registration
- Adds tests for OpenAPI path coverage, Canonical paths, and unique `operationId`
- Adds code-generator contract tests for output-path safety, duplicate files, write reports, Rust syntax, and Golden Hash
- Restores and completes Repository tests for tenant isolation, update persistence, and relationships
- Converts API and procedural-macro examples to compilable documentation tests, with source gates prohibiting new ignored doctests

### Fixed

- Fixes SeaORM update models not resetting ActiveValue, causing fields not to be written
- Fixes tenant filtering and cross-tenant access boundaries in user, role, and dictionary Repositories
- Fixes database-table foreign keys and menu-permission synchronization, improving data integrity
- Fixes invalid IDs being silently discarded, inaccurate authentication-error statuses, and overly broad CORS configuration
- Fixes CI swallowing Codecov, dependency-audit, and build warnings
- Fixes the code generator mixing MySQL metadata fields, reading tables, columns, constraints, auto-increment, and comments separately for MySQL, PostgreSQL, and SQLite
- Fixes pagination defaulting to page `0` and deployment smoke scripts still using old paths and camelCase parameters
- Fixes AES-GCM configuration encryption calling deprecated APIs, no longer hiding deprecation warnings through lint attributes
- Fixes configuration-page `name`/`key` queries not reaching backend filters and configuration values being output as plaintext in debug logs

### Changed

- Unifies authentication through a single `RequestPrincipal`, resolving users, roles, permissions, and data scopes once
- Protected routes uniformly perform tenant rate limiting through authenticated principals, removing duplicate header-driven tenant-quota state
- Splits public and protected monitoring routes and centralizes protected-route policy assembly
- Database configuration becomes a single `[database.connection]`, with connection or migration failures terminating startup immediately
- APIs use plural resources and unified paths; paginated lists use resource roots without old-path aliases
- HTTP 64-bit IDs uniformly serialize as strings, and write DTOs reject unknown fields by default
- Handlers no longer import database entities or Repositories, and Service Repository fields become private
- Database connections are injected centrally into Services at the composition root; Handlers and public use-case methods no longer pass connections on every call
- File services hold database and object-storage dependencies, and `AppState` no longer exposes object-storage implementations to the HTTP layer
- Renames `*ServiceImpl` without corresponding traits uniformly to `*Service`, removing misleading implementation-layer naming
- Users, login logs, operation logs, and file uploads use named Commands/Queries, removing uncalled user-query methods
- `AppState` removes raw database connections, and API production dependencies no longer include database implementations; operation-log middleware writes through `OperLogService`
- Pagination parameters are unified as `page`/`page_size` and reject unknown fields; frontend, backend, and deployment scripts no longer accept old camelCase forms
- Monitoring OpenAPI annotations move to actual Handlers, and rate limiters use public facades and private policies; production Rust source no longer contains `#[allow(...)]`
- Frontend pagination base classes remove arbitrary-field indexes, and configuration and dictionary queries use explicit contracts; backend query DTOs uniformly reject unknown fields
- Restores personal-center and code-generator API integration tests, with no ignored items left in the local SQLite API suite
- `AppState` moves into a separate module, and runtime monitoring displays only real capabilities such as databases, Redis, object storage, and upload circuit breakers
- The code generator uses structured options, bounded relative output paths, and auditable write results; generated Services hold database dependencies, and generated Handlers automatically include route macros and OpenAPI annotations without accessing `state.db`
- Announcement response fields are unified as `notice_type`, matching query and write contracts
- Refactors CI workflows: all events run source hygiene, architecture checks, all-target check/clippy, tests, documentation tests, coverage, and security checks
- Rustdoc warnings are treated as errors in CI, and documentation tests no longer silently skip examples
- Allowed duplicate transitive dependency versions become explicit allowances, retaining bans/source checks while eliminating non-actionable `cargo deny` warnings

### Removed

- Removes event buses, message queues, task queues, gRPC, and hardcoded feature toggles without production consumers
- Removes pseudo-multiple-data-source support, read/write connection selection, duplicate user-context middleware, and invalid runtime state
- Removes old API path aliases and unused intermediate artifacts; frontend and backend no longer support old interface forms
- Removes unused pagination extractors and OpenAPI monitoring placeholder functions

### Security

- Fixes known dependency-vulnerability ignore configuration (cargo-audit)
- Password reset becomes a one-time request-and-completion flow, with status or permission changes invalidating existing sessions

### Validation

- `cargo fmt --all -- --check`
- `python tools/python/check_permission_routes.py`
- `cargo check --workspace --all-targets`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo llvm-cov --workspace --fail-under-lines 55`

---

## [v0.3.0] - 2026-07-01

### Added

- Adds tenant-level request-rate limiting
- Integrates permission-validation middleware into tenant-management routes
- Adds database indexes for permission-code fields

### Changed

- Updates project dependencies and license-validation configuration
- Updates development CORS configuration, API documentation, and project descriptions
- Updates the pnpm version used by CI

### Fixed

- Optimizes user-token invalidation and permission-cache refresh logic

---

## [v0.2.0] - 2026-06-17

### Added

- Permission-resource CRUD
- Automatic endpoint-permission scanning and synchronization
- CI checks for missing permission codes on new endpoints
- Frontend `v-permission` supports wildcards
- Tenant IDs are actually applied to entity and query layers
- Adds protection against privilege escalation during role assignment
- Automatically refreshes menu and button permissions after permission changes

### Fixed

- Fixes invisible system-permission page routes and menus
- Completes initialization SQL for permissions and menus
- Fixes frontend refresh logic after permission changes

### Validation

- `cargo check --workspace --all-targets`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace --lib --tests`
- `cd ryframe-vue3 && pnpm build`
- `vue-tsc -p ryframe-vue3/tsconfig.json --noEmit`

---

## [v0.1.0] - 2026-06-08

### First Stable Version

A modern enterprise backend framework built on Rust + Axum, providing complete authentication and authorization, system management, monitoring and operations, and other capabilities out of the box.

### Features

- **Authentication and authorization**: JWT login/refresh/blacklist + RBAC permission model + DataScope data permissions
- **System management**: Complete CRUD for users / roles / permissions / menus / departments / posts / parameters / dictionaries / notifications
- **Security protection**: XSS filtering, multilayer rate limiting (global / user / endpoint), replay protection, idempotency, and security response headers
- **Caching**: Redis caches (configuration / dictionaries / menu trees / department trees), read caching + write invalidation + cache-miss protection, with automatic fallback to in-memory mode without Redis
- **Monitoring and operations**: Server information, enhanced health checks (DB+Redis connectivity), DB connection pools, online users, cache statistics, and Prometheus Metrics
- **Tracing**: OpenTelemetry distributed tracing with configurable sampling rates
- **Scheduled tasks**: Cron scheduling + task management + execution history + built-in cleanup tasks
- **Code generator**: Reads table structures to automatically generate five-layer Entity / Repository / Service / Handler / DTO CRUD code
- **Resilience**: Retries with exponential backoff + circuit breakers + fallback
- **Data access**: MySQL / PostgreSQL / SQLite support, dynamic switching among multiple data sources + read/write separation
- **Configuration hot reload**: Automatically detects and applies configuration changes at runtime
- **Message queues**: Kafka integration + in-process memory queues
- **Distributed locks**: Redis distributed locks
- **Event bus**: In-process asynchronous event publication / subscription
- **Multi-tenancy**: Tenant isolation (database / Schema level) + tenant quotas
- **gRPC**: Tonic server / client
- **WebSocket**: WebSocket connection management and message broadcasting
- **Object storage**: Dynamic switching among local / MinIO / S3 backends
- **File processing**: File upload / download + image compression + Excel import/export
- **Internationalization**: i18n multilingual support (Chinese / English)
- **Swagger UI**: Interactive API documentation

### Architecture

- Layered Cargo Workspace architecture with 12 crates
- Five-layer model: shared foundations -> infrastructure -> domain -> access -> entry point
- Trait-oriented programming, with mockable Service / Repository tests
- Constructor injection + centralized dependency management through AppState

### CI/CD

- GitHub Actions CI (fmt / clippy / test / coverage / security-audit)
- Code coverage uploaded to Codecov (50% gate)
- Automatic Nightly releases (push main)
- Stable releases triggered manually through `v*` tags

All notable changes to this project will be documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/zh-CN/1.0.0/),
and version numbers follow [Semantic Versioning](https://semver.org/lang/zh-CN/).
