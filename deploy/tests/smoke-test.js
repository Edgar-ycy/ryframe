// RyFrame smoke test.
// Usage: node deploy/tests/smoke-test.js [base_url]

const BASE_URL = process.argv[2] || "http://localhost:8080";
const ADMIN_USER = process.env.ADMIN_USER || "admin";
const ADMIN_PASS = process.env.ADMIN_PASS || "123456";

let passed = 0;
let failed = 0;
const failures = [];

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
  return { Authorization: `Bearer ${token}` };
}

function assertPage(json, label) {
  if (json.code !== 200) {
    throw new Error(`${label}: business code is ${json.code}`);
  }
  if (!Array.isArray(json.rows) || typeof json.total !== "number") {
    throw new Error(`${label}: expected top-level rows/total page response`);
  }
}

async function jsonRequest(url, options = {}) {
  const res = await fetch(url, options);
  const json = await res.json().catch(() => null);
  return { res, json };
}

async function runSmokeTests() {
  console.log("=".repeat(60));
  console.log("RyFrame smoke test");
  console.log("=".repeat(60));
  console.log(`Base URL: ${BASE_URL}`);
  console.log(`Time: ${new Date().toISOString()}`);
  console.log("");

  await test("health endpoint", async () => {
    const res = await fetch(`${BASE_URL}/health`);
    await assertOk(res, "Health");
    const text = await res.text();
    if (!/ok/i.test(text)) throw new Error("health body does not contain ok");
  });

  await test("version endpoint", async () => {
    const { res, json } = await jsonRequest(`${BASE_URL}/api/v1/version`);
    await assertOk(res, "Version");
    if (!json?.name || !json?.version) throw new Error("version response missing name/version");
  });

  let accessToken = null;
  let refreshToken = null;

  await test("login", async () => {
    const { res, json } = await jsonRequest(`${BASE_URL}/api/v1/auth/login`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username: ADMIN_USER, password: ADMIN_PASS }),
    });
    await assertOk(res, "Login");
    if (json?.code !== 200 || !json?.data?.access_token) {
      throw new Error(`login response missing access_token: ${JSON.stringify(json)}`);
    }
    accessToken = json.data.access_token;
    refreshToken = json.data.refresh_token;
  });

  await test("wrong password rejected", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/auth/login`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username: ADMIN_USER, password: "wrong_password_xyz" }),
    });
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
    if (!refreshToken) return;
    const { res, json } = await jsonRequest(`${BASE_URL}/api/v1/auth/refresh`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ refresh_token: refreshToken }),
    });
    await assertOk(res, "Refresh");
    if (!json?.data?.access_token) throw new Error("refresh response missing access_token");
    accessToken = json.data.access_token;
  });

  await test("openapi json", async () => {
    const { res, json } = await jsonRequest(`${BASE_URL}/api/v1/api-docs/openapi.json`);
    await assertOk(res, "OpenAPI");
    if (!json?.openapi) throw new Error("invalid OpenAPI document");
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

  await test("monitor health", async () => {
    const { res, json } = await jsonRequest(`${BASE_URL}/api/v1/monitor/health`);
    await assertOk(res, "Monitor Health");
    if (json?.code !== 200) throw new Error("monitor health business code is not 200");
  });

  await test("user list page", async () => {
    const { res, json } = await jsonRequest(`${BASE_URL}/api/v1/system/users/list?page=1&pageSize=5`, {
      headers: authHeaders(accessToken),
    });
    await assertOk(res, "User List");
    assertPage(json, "User List");
  });

  await test("role list", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/system/roles/list`, {
      headers: authHeaders(accessToken),
    });
    await assertOk(res, "Role List");
  });

  await test("menu tree", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/system/menus/tree`, {
      headers: authHeaders(accessToken),
    });
    await assertOk(res, "Menu Tree");
  });

  await test("dept tree", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/system/depts/tree`, {
      headers: authHeaders(accessToken),
    });
    await assertOk(res, "Dept Tree");
  });

  await test("permission tree", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/system/permissions/tree`, {
      headers: authHeaders(accessToken),
    });
    await assertOk(res, "Permission Tree");
  });

  await test("operation log page", async () => {
    const { res, json } = await jsonRequest(`${BASE_URL}/api/v1/system/operlogs/list?page=1&pageSize=5`, {
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
    const { json } = await jsonRequest(`${BASE_URL}/api/v1/auth/login`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username: ADMIN_USER, password: ADMIN_PASS }),
    });
    const token = json?.data?.access_token;
    if (!token) throw new Error("login response missing token");

    const logoutRes = await fetch(`${BASE_URL}/api/v1/auth/logout`, {
      method: "POST",
      headers: authHeaders(token),
    });
    await assertOk(logoutRes, "Logout");

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
