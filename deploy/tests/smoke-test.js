// RyFrame smoke test.
// Usage: node deploy/tests/smoke-test.js [base_url]

const BASE_URL = process.argv[2] || "http://localhost:8080";
const ADMIN_USER = process.env.ADMIN_USER || "admin";
const ADMIN_PASS = process.env.ADMIN_PASS || "123456";
const TENANT_ID = process.env.TENANT_ID || "system";
const DATASOURCE_SMOKE_TABLE = process.env.DATASOURCE_SMOKE_TABLE || "t_gongxv";

let passed = 0;
let failed = 0;
const failures = [];
const sessionCookies = new Map();

async function test(name, fn) {
  process.stdout.write(`  ${name} ... `);
  try {
    await fn();
    console.log("PASS");
    passed += 1;
  } catch (err) {
    console.log(`FAIL\n       ${err.message}`);
    failed += 1;
    failures.push({ name, error: err.message });
  }
}

async function assertStatus(res, expected, label) {
  if (res.status !== expected) {
    const body = await res.text().catch(() => "<unreadable body>");
    throw new Error(`${label}: expected HTTP ${expected}, got ${res.status}. ${body.slice(0, 200)}`);
  }
}

async function assertOk(res, label) {
  await assertStatus(res, 200, label);
}

function authHeaders(token) {
  return { Authorization: `Bearer ${token}`, "X-Tenant-Id": TENANT_ID };
}

function loginHeaders(csrfToken) {
  return {
    "Content-Type": "application/json",
    "X-Tenant-Id": TENANT_ID,
    "X-CSRF-Token": csrfToken,
  };
}

function assertPage(json, label) {
  if (json.code !== 200) {
    throw new Error(`${label}: business code is ${json.code}`);
  }
  if (!Array.isArray(json.rows) || typeof json.total !== "number") {
    throw new Error(`${label}: expected top-level rows/total page response`);
  }
}

function storeResponseCookies(res, jar) {
  const values = typeof res.headers.getSetCookie === "function"
    ? res.headers.getSetCookie()
    : [res.headers.get("set-cookie")].filter(Boolean);
  for (const value of values) {
    const [pair, ...attributes] = value.split(";");
    const separator = pair.indexOf("=");
    if (separator < 1) continue;
    const name = pair.slice(0, separator).trim();
    const cookieValue = pair.slice(separator + 1).trim();
    const expired = attributes.some((attribute) => /^\s*Max-Age=0\s*$/i.test(attribute));
    if (expired || cookieValue === "") jar.delete(name);
    else jar.set(name, cookieValue);
  }
}

async function fetchWithCookies(url, options = {}, jar = sessionCookies) {
  const headers = new Headers(options.headers || {});
  if (jar.size) {
    headers.set("Cookie", [...jar].map(([name, value]) => `${name}=${value}`).join("; "));
  }
  const res = await fetch(url, { ...options, headers });
  storeResponseCookies(res, jar);
  return res;
}

async function jsonRequest(url, options = {}, jar = sessionCookies) {
  const res = await fetchWithCookies(url, options, jar);
  const json = await res.json().catch(() => null);
  return { res, json };
}

async function csrfChallenge(jar = sessionCookies) {
  const { res, json } = await jsonRequest(`${BASE_URL}/api/v1/auth/csrf`, {}, jar);
  await assertOk(res, "CSRF challenge");
  const token = json?.data?.csrf_token;
  if (!token) throw new Error(`CSRF response missing token: ${JSON.stringify(json)}`);
  return token;
}

async function runSmokeTests() {
  console.log("=".repeat(60));
  console.log("RyFrame smoke test");
  console.log("=".repeat(60));
  console.log(`Base URL: ${BASE_URL}`);
  console.log(`Time: ${new Date().toISOString()}`);
  console.log("");

  await test("liveness and readiness endpoints", async () => {
    await assertOk(await fetch(`${BASE_URL}/livez`), "Liveness");
    await assertOk(await fetch(`${BASE_URL}/readyz`), "Readiness");
    await assertStatus(await fetch(`${BASE_URL}/health`), 404, "Removed legacy health endpoint");
  });

  await test("version endpoint", async () => {
    const { res, json } = await jsonRequest(`${BASE_URL}/api/v1/version`);
    await assertOk(res, "Version");
    if (!json?.name || !json?.version) throw new Error("version response missing name/version");
  });

  let accessToken = null;

  await test("login", async () => {
    const csrfToken = await csrfChallenge();
    const { res, json } = await jsonRequest(`${BASE_URL}/api/v1/auth/login`, {
      method: "POST",
      headers: loginHeaders(csrfToken),
      body: JSON.stringify({ username: ADMIN_USER, password: ADMIN_PASS }),
    });
    await assertOk(res, "Login");
    if (json?.code !== 200 || !json?.data?.access_token) {
      throw new Error(`login response missing access_token: ${JSON.stringify(json)}`);
    }
    accessToken = json.data.access_token;
    if (json.data.refresh_token) throw new Error("refresh token leaked into login JSON");
    if (!sessionCookies.has("ryframe_refresh_token")) {
      throw new Error("login did not set the refresh cookie");
    }
  });

  await test("wrong password rejected", async () => {
    const anonymousCookies = new Map();
    const csrfToken = await csrfChallenge(anonymousCookies);
    const res = await fetchWithCookies(`${BASE_URL}/api/v1/auth/login`, {
      method: "POST",
      headers: loginHeaders(csrfToken),
      body: JSON.stringify({ username: ADMIN_USER, password: "wrong_password_xyz" }),
    }, anonymousCookies);
    if (res.status === 200) throw new Error("wrong password returned HTTP 200");
  });

  await test("current user", async () => {
    const { res, json } = await jsonRequest(`${BASE_URL}/api/v1/auth/me`, {
      headers: authHeaders(accessToken),
    });
    await assertOk(res, "Me");
    if (!json?.data) throw new Error("me response missing data");
  });

  await test("unauthenticated request rejected", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/auth/me`);
    if (res.status === 200) throw new Error("unauthenticated request returned HTTP 200");
  });

  await test("refresh token", async () => {
    const csrfToken = await csrfChallenge();
    const { res, json } = await jsonRequest(`${BASE_URL}/api/v1/auth/refresh`, {
      method: "POST",
      headers: { "X-CSRF-Token": csrfToken },
    });
    await assertOk(res, "Refresh");
    if (!json?.data?.access_token) throw new Error("refresh response missing access_token");
    if (json.data.refresh_token) throw new Error("refresh token leaked into refresh JSON");
    accessToken = json.data.access_token;
  });

  await test("openapi json", async () => {
    const { res, json } = await jsonRequest(`${BASE_URL}/api/v1/api-docs/openapi.json`);
    await assertOk(res, "OpenAPI");
    if (!json?.openapi) throw new Error("invalid OpenAPI document");
    if (!json?.["x-ryframe-menu-routes"]?.routes?.length) {
      throw new Error("OpenAPI menu route contract is missing");
    }
    if (!json?.["x-ryframe-password-policy"]?.pattern) {
      throw new Error("OpenAPI password policy contract is missing");
    }
  });

  await test("swagger ui", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/swagger-ui`);
    await assertOk(res, "Swagger UI");
  });

  await test("prometheus metrics", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/monitor/metrics`);
    await assertOk(res, "Metrics");
    const text = await res.text();
    if (!text.includes("ryframe_")) throw new Error("metrics output missing ryframe_ prefix");
  });

  await test("runtime topology", async () => {
    const { res, json } = await jsonRequest(`${BASE_URL}/api/v1/monitor/runtime`, {
      headers: authHeaders(accessToken),
    });
    await assertOk(res, "Runtime");
    const runtime = json?.data;
    if (!runtime?.database?.connected || runtime.database.replica_count !== 0) {
      throw new Error(`database topology is not healthy: ${JSON.stringify(runtime?.database)}`);
    }
    if (
      runtime.database.read_policy !== "primary" ||
      runtime.database.replicas.length !== 0
    ) {
      throw new Error(`database read routing is not healthy: ${JSON.stringify(runtime.database)}`);
    }
    if (
      runtime.database.source_count !== 1 ||
      runtime.database.sources[0]?.name !== "ryframe_device" ||
      !runtime.database.sources[0]?.connected
    ) {
      throw new Error(`named data source is not healthy: ${JSON.stringify(runtime.database)}`);
    }
    if (runtime?.object_storage?.backend !== "rustfs" || !runtime.object_storage.connected) {
      throw new Error(`RustFS is not healthy: ${JSON.stringify(runtime?.object_storage)}`);
    }
  });

  await test("ryframe_device generator source", async () => {
    const query = new URLSearchParams({ table_name: DATASOURCE_SMOKE_TABLE });
    const { res, json } = await jsonRequest(`${BASE_URL}/api/v1/tools/gen/tables?${query}`, {
      headers: authHeaders(accessToken),
    });
    await assertOk(res, "Generator Data Source");
    assertPage(json, "Generator Data Source");
    if (!json.rows.some((table) => table.table_name === DATASOURCE_SMOKE_TABLE)) {
      throw new Error(
        `generator did not read ${DATASOURCE_SMOKE_TABLE} from ryframe_device: ${JSON.stringify(json.rows)}`,
      );
    }
  });

  await test("RustFS upload and download", async () => {
    const payload = "ryframe-rustfs-smoke";
    const form = new FormData();
    form.append("file", new Blob([payload], { type: "text/plain" }), "rustfs-smoke.txt");
    const { res, json } = await jsonRequest(`${BASE_URL}/api/v1/common/upload`, {
      method: "POST",
      headers: authHeaders(accessToken),
      body: form,
    });
    await assertOk(res, "RustFS Upload");
    const filePath = json?.data?.[0]?.file_info?.file_path;
    if (!filePath) throw new Error(`upload response missing file path: ${JSON.stringify(json)}`);

    const query = new URLSearchParams({ bucket: "uploads", path: filePath });
    const download = await fetch(`${BASE_URL}/api/v1/common/file/download?${query}`, {
      headers: authHeaders(accessToken),
    });
    await assertOk(download, "RustFS Download");
    if ((await download.text()) !== payload) {
      throw new Error("downloaded RustFS object differs from uploaded payload");
    }
  });

  await test("user list page", async () => {
    const { res, json } = await jsonRequest(`${BASE_URL}/api/v1/system/users?page=1&page_size=5`, {
      headers: authHeaders(accessToken),
    });
    await assertOk(res, "User List");
    assertPage(json, "User List");
  });

  await test("role list", async () => {
    const { res, json } = await jsonRequest(`${BASE_URL}/api/v1/system/roles?page=1&page_size=5`, {
      headers: authHeaders(accessToken),
    });
    await assertOk(res, "Role List");
    assertPage(json, "Role List");
  });

  await test("menu tree", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/system/menus/tree`, {
      headers: authHeaders(accessToken),
    });
    await assertOk(res, "Menu Tree");
  });

  await test("current user menus", async () => {
    const { res, json } = await jsonRequest(`${BASE_URL}/api/v1/system/menus/current`, {
      headers: authHeaders(accessToken),
    });
    await assertOk(res, "Current Menus");
    if (json?.code !== 200 || !Array.isArray(json?.data)) {
      throw new Error("current menu response missing data array");
    }
  });

  await test("dept tree", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/system/depts/tree`, {
      headers: authHeaders(accessToken),
    });
    await assertOk(res, "Dept Tree");
  });

  await test("permission tree", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/system/perms/tree`, {
      headers: authHeaders(accessToken),
    });
    await assertOk(res, "Permission Tree");
  });

  await test("operation log page", async () => {
    const { res, json } = await jsonRequest(`${BASE_URL}/api/v1/system/operlogs?page=1&page_size=5`, {
      headers: authHeaders(accessToken),
    });
    await assertOk(res, "OperLog List");
    assertPage(json, "OperLog List");
  });

  await test("invalid token rejected", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/auth/me`, {
      headers: authHeaders("invalid_token_xyz_123"),
    });
    if (res.status === 200) throw new Error("invalid token returned HTTP 200");
  });

  await test("logout invalidates token", async () => {
    const logoutCookies = new Map();
    const loginCsrf = await csrfChallenge(logoutCookies);
    const { json } = await jsonRequest(`${BASE_URL}/api/v1/auth/login`, {
      method: "POST",
      headers: loginHeaders(loginCsrf),
      body: JSON.stringify({ username: ADMIN_USER, password: ADMIN_PASS }),
    }, logoutCookies);
    const token = json?.data?.access_token;
    if (!token) throw new Error("login response missing token");

    const logoutCsrf = await csrfChallenge(logoutCookies);
    const logoutRes = await fetchWithCookies(`${BASE_URL}/api/v1/auth/logout`, {
      method: "POST",
      headers: { ...authHeaders(token), "X-CSRF-Token": logoutCsrf },
    }, logoutCookies);
    await assertOk(logoutRes, "Logout");
    if (logoutCookies.has("ryframe_refresh_token")) {
      throw new Error("logout did not clear the refresh cookie");
    }

    const meRes = await fetch(`${BASE_URL}/api/v1/auth/me`, {
      headers: authHeaders(token),
    });
    if (meRes.status === 200) throw new Error("logged out token still works");
  });

  console.log("");
  console.log("=".repeat(60));
  console.log(`Result: ${passed} passed / ${failed} failed / ${passed + failed} total`);
  console.log("=".repeat(60));

  if (failures.length) {
    console.log("Failures:");
    for (const failure of failures) {
      console.log(`- ${failure.name}: ${failure.error}`);
    }
  }
}

async function main() {
  await runSmokeTests();
  process.exit(failed > 0 ? 1 : 0);
}

main().catch((err) => {
  console.error("Smoke test crashed:", err);
  process.exit(1);
});
