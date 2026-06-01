// ============================================================
// RyFrame 冒烟测试脚本
//
// 使用方式：
//   node deploy/tests/smoke-test.js [base_url]
//
// 示例：
//   node deploy/tests/smoke-test.js
//   node deploy/tests/smoke-test.js http://localhost:8080
//   node deploy/tests/smoke-test.js https://api.example.com
//
// 退出码：
//   0 - 所有冒烟测试通过
//   1 - 至少一项测试失败
//
// CI 集成示例：
//   node deploy/tests/smoke-test.js || (echo "冒烟测试失败" && exit 1)
// ============================================================

const BASE_URL = process.argv[2] || "http://localhost:8080";
const ADMIN_USER = process.env.ADMIN_USER || "admin";
const ADMIN_PASS = process.env.ADMIN_PASS || "123456";

// ============================================================
// 测试结果收集
// ============================================================
let passed = 0;
let failed = 0;
const failures = [];

async function test(name, fn) {
  process.stdout.write(`  ${name} ... `);
  try {
    await fn();
    console.log("✅ PASS");
    passed++;
    return true;
  } catch (err) {
    console.log(`❌ FAIL\n       ${err.message}`);
    failed++;
    failures.push({ name, error: err.message });
    return false;
  }
}

async function assertStatus(res, expected, label) {
  if (res.status !== expected) {
    const body = await res.text().catch(() => "<无法读取响应体>");
    throw new Error(
      `${label}: 期望 HTTP ${expected}，实际 ${res.status}。响应: ${body.slice(0, 200)}`
    );
  }
}

async function assertOk(res, label) {
  return assertStatus(res, 200, label);
}

// ============================================================
// 冒烟测试用例
// ============================================================
async function runSmokeTests() {
  console.log("=".repeat(60));
  console.log("RyFrame 冒烟测试");
  console.log("=".repeat(60));
  console.log(`目标地址: ${BASE_URL}`);
  console.log(`测试时间: ${new Date().toISOString()}`);
  console.log("");

  // ---- 1. 基础可用性 ----
  console.log("📋 1. 基础可用性");
  await test("Health Check 端点可达", async () => {
    const res = await fetch(`${BASE_URL}/health`);
    await assertOk(res, "Health Check");
    const text = await res.text();
    if (!text.includes("ok") && !text.includes("OK")) {
      throw new Error("响应体不含 'ok'");
    }
  });

  await test("版本信息端点可用", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/version`);
    await assertOk(res, "Version");
    const json = await res.json();
    if (!json.name) throw new Error("响应缺少 name 字段");
    console.log(`       [版本: ${json.name} ${json.version}]`);
  });

  // ---- 2. 认证流程 ----
  console.log("\n📋 2. 认证流程");
  let accessToken = null;
  let refreshToken = null;

  await test("登录成功获取令牌", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/auth/login`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username: ADMIN_USER, password: ADMIN_PASS }),
    });
    await assertOk(res, "Login");
    const json = await res.json();
    if (json.code !== 200) throw new Error(`业务错误码: ${json.code}, ${json.message}`);
    if (!json.data || !json.data.access_token) throw new Error("响应缺少 access_token");
    accessToken = json.data.access_token;
    refreshToken = json.data.refresh_token;
  });

  await test("错误密码登录被拒绝", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/auth/login`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username: ADMIN_USER, password: "wrong_password_xyz" }),
    });
    if (res.status === 200) throw new Error("错误密码不应返回 HTTP 200");
  });

  await test("获取当前用户信息（需认证）", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/auth/me`, {
      headers: { Authorization: `Bearer ${accessToken}` },
    });
    await assertOk(res, "Me");
    const json = await res.json();
    if (!json.data || !json.data.user) throw new Error("响应缺少 user 字段");
  });

  await test("未认证请求被拒绝", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/auth/me`);
    if (res.status === 200) throw new Error("未认证请求不应返回 HTTP 200");
  });

  await test("刷新令牌", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/auth/refresh`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ refresh_token: refreshToken }),
    });
    await assertOk(res, "Refresh");
    const json = await res.json();
    if (!json.data || !json.data.access_token) throw new Error("刷新后缺少 access_token");
    accessToken = json.data.access_token; // 更新令牌
  });

  // ---- 3. 公开端点 ----
  console.log("\n📋 3. 公开端点");

  await test("OpenAPI JSON 文档可用", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/api-docs/openapi.json`);
    await assertOk(res, "OpenAPI");
    const json = await res.json();
    if (!json.openapi) throw new Error("非有效 OpenAPI 文档");
    console.log(`       [OpenAPI ${json.openapi}]`);
  });

  await test("Swagger UI 可访问", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/swagger-ui`);
    await assertOk(res, "Swagger UI");
  });

  // ---- 4. 监控端点 ----
  console.log("\n📋 4. 监控端点");

  await test("Prometheus Metrics 端点", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/monitor/metrics`);
    await assertOk(res, "Metrics");
    const text = await res.text();
    if (!text.includes("ryframe_")) throw new Error("Metrics 输出不含 ryframe_ 前缀指标");
    console.log(`       [指标行数: ${text.split("\n").filter((l) => l && !l.startsWith("#")).length}]`);
  });

  await test("增强健康检查端点", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/monitor/health`);
    await assertOk(res, "Monitor Health");
    const json = await res.json();
    if (json.code !== 200) throw new Error("Health 检查返回非 200");
  });

  // ---- 5. 系统管理（需认证） ----
  console.log("\n📋 5. 系统管理（需认证）");

  await test("用户列表查询", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/system/users/list?page=1&page_size=5`, {
      headers: { Authorization: `Bearer ${accessToken}` },
    });
    await assertOk(res, "User List");
    const json = await res.json();
    if (!json.data || !json.data.items) throw new Error("分页结构不正确");
    console.log(`       [用户数: ${json.data.total}]`);
  });

  await test("角色列表查询", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/system/roles/list`, {
      headers: { Authorization: `Bearer ${accessToken}` },
    });
    await assertOk(res, "Role List");
  });

  await test("菜单树查询", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/system/menus/tree`, {
      headers: { Authorization: `Bearer ${accessToken}` },
    });
    await assertOk(res, "Menu Tree");
  });

  await test("部门树查询", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/system/depts/tree`, {
      headers: { Authorization: `Bearer ${accessToken}` },
    });
    await assertOk(res, "Dept Tree");
  });

  await test("权限树查询", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/system/permissions/tree`, {
      headers: { Authorization: `Bearer ${accessToken}` },
    });
    await assertOk(res, "Permission Tree");
  });

  await test("操作日志查询", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/system/operlogs/list?page=1&page_size=5`, {
      headers: { Authorization: `Bearer ${accessToken}` },
    });
    await assertOk(res, "OperLog List");
  });

  // ---- 6. 安全验证 ----
  console.log("\n📋 6. 安全验证");

  await test("无效令牌被拒绝", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/auth/me`, {
      headers: { Authorization: "Bearer invalid_token_xyz_123" },
    });
    if (res.status === 200) throw new Error("无效令牌不应返回 HTTP 200");
  });

  await test("伪造令牌被拒绝", async () => {
    const res = await fetch(`${BASE_URL}/api/v1/auth/me`, {
      headers: {
        Authorization:
          "Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIiwianRpIjoidGVzdCJ9.fake_signature",
      },
    });
    if (res.status === 200) throw new Error("伪造签名令牌不应返回 HTTP 200");
  });

  await test("登出后令牌失效", async () => {
    // 先登录
    const loginRes = await fetch(`${BASE_URL}/api/v1/auth/login`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ username: ADMIN_USER, password: ADMIN_PASS }),
    });
    const loginJson = await loginRes.json();
    const token = loginJson.data.access_token;

    // 登出
    const logoutRes = await fetch(`${BASE_URL}/api/v1/auth/logout`, {
      method: "POST",
      headers: { Authorization: `Bearer ${token}` },
    });
    await assertOk(logoutRes, "Logout");

    // 登出后访问应被拒绝
    const meRes = await fetch(`${BASE_URL}/api/v1/auth/me`, {
      headers: { Authorization: `Bearer ${token}` },
    });
    if (meRes.status === 200) throw new Error("已登出令牌不应仍可用于访问");
  });

  // ---- 总结 ----
  console.log("\n" + "=".repeat(60));
  console.log(`测试结果: ${passed} 通过 / ${failed} 失败 / ${passed + failed} 总计`);
  console.log("=".repeat(60));

  if (failures.length > 0) {
    console.log("\n❌ 失败详情:");
    for (const f of failures) {
      console.log(`   - ${f.name}`);
      console.log(`     ${f.error}`);
    }
  }
}

// ============================================================
// 主函数
// ============================================================
async function main() {
  try {
    await runSmokeTests();
  } catch (err) {
    console.error("\n❌ 冒烟测试执行异常:", err.message);
    process.exit(1);
  }

  if (failed > 0) {
    console.log(`\n❌ 冒烟测试未通过！${failed} 项失败。`);
    process.exit(1);
  } else {
    console.log(`\n✅ 所有冒烟测试通过！`);
    process.exit(0);
  }
}

main();
