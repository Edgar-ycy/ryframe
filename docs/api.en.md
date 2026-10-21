[简体中文](api.md)

# API

Login, refresh, and `/auth/context` use the same `SessionContext`, which must contain the boolean field `password_change_required`. When this value is true, the super administrator flag is false, and the role, permission, menu, and capability collections are empty. A restricted token can only read the session context, change the user's own password, and log out; it cannot issue message tickets. A successful password change clears the flag, updates the authorization version, and revokes the user's login sessions. The client must log in again.

Authorization diagnostics distinguish the cache states `current`, `behind`, `ahead`, `missing`, `disabled`, and `unavailable`. When the mirror is behind a new token, the system verifies the identity against the primary database and rebuilds the mirror atomically. When the mirror has a newer version, the old token is still rejected, and the cache is not overwritten with an older version. A disabled cache is a normal configuration state and is not reported as a cache failure warning.

## Recovering Accounts with Unknown Passwords

Ordinary users created through creation, import, or initialization use `123456` and must change their password after the first login. Existing accounts are not reset at startup or during repeated initialization. If an account from an older development version has an unknown password, the administrator must first complete their own initial password change, then select the ordinary user's password reset action in user management, enter a reason, and generate a one-time link. The user opens the link, sets a new password that meets the complexity requirements, and logs in again. This process does not apply to super administrators and does not directly rewrite password hashes in the database.

A reset request is valid for at most 24 hours and can be consumed only once. A successful reset or self-service password change updates the password, clears the password change flag, increments the authorization version, and immediately expires the user's other pending reset requests in the same transaction. Old access tokens and refresh tokens therefore become invalid. Only one concurrent reset for the same user can complete; requests for other users are unaffected, and reset requests explicitly created afterward remain usable. Resetting a password does not automatically enable a disabled account; an administrator must enable it separately. Enabling an account alone does not clear the initial password change requirement. A reset in single-tenant mode always uses the system tenant. Links in multi-tenant mode retain the target tenant identifier and cannot be used to reset a password through another tenant's link.

## Contract Entry Points

HTTP APIs use the `/api/v1` prefix. See `openapi/openapi.json` for the complete paths, fields, and operation IDs. In a development environment, after enabling `api_docs.enabled` and compiling the `runtime-swagger-ui` feature, the documentation is available at `/api/v1/swagger-ui`; the raw document is at `/api/v1/api-docs/openapi.json`.

Callers should use the callers generated from the current contract for the four domains `core/system/platform/monitor`. API changes in development versions require corresponding caller updates; mappings for old structures are not provided.

New configurations disable multi-tenancy by default. The `multi_tenancy_enabled` field of `GET /api/v1/version` determines the client's operating mode; retry if fetching it fails. The identity in single-tenant mode is fixed to `system`. Tenant and plan management routes are not exposed, and the session menu also excludes these pages. Configuration packages, configuration transfers, and data maintenance remain available and retain system tenant and permission checks. Explicitly enabling multi-tenancy makes tenant management, plan assignment, and tenant selection available again.

Ordinary business endpoints use JSON. Uploads, text, CAPTCHA images, Blobs, and file downloads use the media types declared in OpenAPI, and callers must not decode these responses as JSON. Timestamps use RFC3339 with a time zone and are normalized to UTC on the server. JSON DTOs reject unknown fields by default. Invalid enums, reversed time ranges, and batch requests outside the allowed bounds return 400 before being enqueued.

IDs are always transmitted as strings. Exact Decimal fields also accept only decimal strings, such as `"0.125000"`. The numbers of integer and fractional digits are determined by the model's `DECIMAL(p,s)`. Values exceeding that capacity are rejected; conversion through floating point and automatic rounding are prohibited. For `i64/u64` fields declared by the business generator to use string transport, request bodies, queries, and responses all use canonical decimal strings, such as `"9007199254740993"`. Request bodies do not accept JSON numbers, and values must not first pass through JavaScript Number before conversion. URL parameters for unsigned business primary keys likewise retain canonical decimal notation and the full u64 range. These business integer fields reject leading zeros, plus signs, negative zero, fractional values, whitespace, and out-of-range values. Optional fields may be omitted or set to null. Business defaults and nullable update semantics follow the current model and OpenAPI.

Uploads are subject to both the configured file size limit and request body limit, including chunked transfers. The Multipart extractor's default 2 MiB limit is not applied as an additional limit. Malformed upload forms return 400, size limit violations return 413, and underlying read failures return 500.

JSON business responses use a unified envelope. Text and binary responses directly use the content type declared by the contract, without a JSON envelope:

```json
{
  "code": 200,
  "message": "操作成功",
  "data": {},
  "request_id": "01K...",
  "error_key": null,
  "details": null
}
```

## Authentication and Authorization

First request `/api/v1/auth/csrf` to obtain a CSRF token. Session write operations such as login, refresh, and logout include it in the `X-CSRF-Token` request header. Login and refresh responses both contain a short-lived access token and a structurally identical `SessionContext`; `GET /api/v1/auth/context` returns the same session snapshot. The long-lived refresh token is stored in an HttpOnly Cookie. Protected APIs use:

```http
Authorization: Bearer <access_token>
```

The client should atomically apply the user, tenant, roles, permissions, authorization and runtime epochs, capability, and menu projections from `SessionContext`. It must not mix session fields from different responses. The session context explicitly returns `is_super_admin`; the client uses this field and the authorization projection to render the interface and does not infer super administrator status from role codes. The server-side authenticated principal retains the user authorization version and tenant authorization epoch from its construction. A snapshot is rejected if its versions do not match those outside the snapshot; a version read later does not overwrite the old principal. Message ticket issuance also checks the user authorization versions of the principal and access token.

Plan versions may have an empty capability set, and tenants can still be provisioned with authentication and basic resources. Notice management and tenant-initiated message publishing are constrained jointly by the compiled capability catalog and the effective plan. Other basic management APIs continue to follow RBAC. Permission wildcards for ordinary tenants cannot enable disabled capabilities or access platform-only APIs.

Capability catalog variants return a complete `config_schema`; the initial variants accept only empty objects. Delivering notices to the message center requires both the notice and proactive messaging capabilities. The inbox, delivery of already accepted messages, and system task notifications are unaffected by the proactive messaging switch.

Configuration transfers are platform management operations. A system tenant operator explicitly specifies the tenant that owns the resources through `/api/v1/platform/tenants/{tenant_id}/config-packages` and `/api/v1/platform/tenants/{tenant_id}/config-transfers`. The `tenant_id` identifies the configuration source or application target; the operator still undergoes authorization and background revalidation as a system tenant identity. Configuration packages filter out platform management, tenant management, wildcard permissions, and capabilities that have not been enabled.

Each route declares one policy in the access catalog:

- `Public`: no login required.
- `Authenticated`: a valid session is required.
- `Permission`: a generated permission code is required.
- `Capability`: the capability must be enabled for both the current deployment and tenant.

The frontend routes missing permissions to 403, a known page with a missing capability to “Feature unavailable,” and an unknown page to 404.

Logged-in users read the current tenant's sidebar theme and skin through `GET /api/v1/common/shell-settings`. This API returns only fixed interface settings and does not accept configuration keys. Missing items return `null`, and the client uses defaults. Reading arbitrary configuration by key still requires configuration management permission.

## Errors

Error responses contain stable error codes, displayable messages, and request tracing information. Callers should handle them by HTTP status and error code rather than parsing the Chinese text.

- 400: invalid request or state transition.
- 401: unauthenticated or invalid session.
- 403: insufficient permission or tenant capability.
- 404: the resource does not exist, is outside the caller's authorized scope, or has been deleted. Standard resources configured with `owner_field` use the same data scope for listing, details, editing, and deletion. Writes lock rows within that scope in the current business transaction, and records outside the scope reveal no existence information. Scope-sensitive reads in the control database use the primary database to prevent stale user or department relationships on replicas from broadening the scope.
- 409: resource state conflict.
- 413: the number of matching export rows or the request body exceeds the limit.
- 501/503: a deployment capability is unavailable or a dependency is temporarily unavailable.

## Filtered Exports

The seven export types use separate strongly typed requests with a unified envelope:

```json
{
  "filter": {},
  "confirm_all": false,
  "ids": ["101", "102"]
}
```

Requests do not accept `page` or `page_size`. When rows are selected, pass their string primary keys in `ids`. Only selected records that match the last successfully applied filters and data permissions are exported. At most 1000 IDs may be supplied, and duplicate IDs are normalized. Select all in a list selects only the current page; changing pages, refreshing, or switching sessions clears the selection.

When no rows are selected, omit `ids` or pass an empty list to export all matching pages using the last successfully applied filters. If the filters are empty, a second confirmation is required and `confirm_all` must be set to `true`.

Creation synchronously captures the matching row count, permission fingerprint, and `upper_id` snapshot. The Worker reads only `id <= upper_id`, excluding later additions. Any permission change causes the operation to fail closed.

Stable error codes include:

- `EXPORT_ALL_CONFIRMATION_REQUIRED`
- `EXPORT_NO_MATCHING_ROWS`

## Deleting Export Records

Single and batch deletion of terminal records share the following endpoint:

```http
POST /api/v1/common/jobs/deletions
Idempotency-Key: <可重试键>
```

```json
{
  "ids": ["123", "456"]
}
```

After sorting and deduplication, there must be 1–100 IDs. The entire batch is first checked for tenant, requester, terminal state, and lease, then marked for deletion. Once accepted, records immediately disappear from the list and from the unread and download APIs. Object deletion failures are retried internally, and records do not reappear on the web page.

## Contract Validation

After OpenAPI changes, run `cargo xtask generate api --write` to refresh the frontend's derived contract, then run the frontend consumer self-check and browser smoke checks. If a caller reports a missing operation or mismatched DTO, resynchronize the contract first, then check the operation IDs exported by the backend.

When multi-tenancy is enabled, the login page selects a tenant through the public tenant name search, and authentication requests pass the selected tenant identifier. Single-tenant mode does not display or request tenant selection. `GET /api/v1/auth/tenants` supports `search`, `page`, and `page_size`, with at most 50 items per page. It returns only the names and identifiers of enabled, unexpired tenants; tenant status is still revalidated during login.
