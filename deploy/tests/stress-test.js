// ============================================================
// RyFrame 压力测试脚本
//
// 使用方式：
//   node deploy/tests/stress-test.js [base_url]
//
// 示例：
//   node deploy/tests/stress-test.js
//   node deploy/tests/stress-test.js http://localhost:8080
//   node deploy/tests/stress-test.js https://api.example.com
//
// 测试场景：
//   1. Health Check - 基准可用性
//   2. 登录接口 - 认证压力
//   3. 公开 API - 并发吞吐
//   4. Metrics - 监控端点
// ============================================================

const BASE_URL = process.argv[2] || "http://localhost:8080";
const ADMIN_USER = process.env.ADMIN_USER || "admin";
const ADMIN_PASS = process.env.ADMIN_PASS || "123456";

// ============================================================
// 工具函数
// ============================================================

let totalRequests = 0;
let totalSuccess = 0;
let totalFail = 0;
let totalLatency = 0;
let minLatency = Infinity;
let maxLatency = -Infinity;

async function timedRequest(label, fn) {
  const start = performance.now();
  try {
    const result = await fn();
    const elapsed = (performance.now() - start).toFixed(1);
    totalRequests++;
    totalSuccess++;
    totalLatency += parseFloat(elapsed);
    minLatency = Math.min(minLatency, parseFloat(elapsed));
    maxLatency = Math.max(maxLatency, parseFloat(elapsed));
    return { ok: true, latency: elapsed, result };
  } catch (err) {
    const elapsed = (performance.now() - start).toFixed(1);
    totalRequests++;
    totalFail++;
    totalLatency += parseFloat(elapsed);
    minLatency = Math.min(minLatency, parseFloat(elapsed));
    maxLatency = Math.max(maxLatency, parseFloat(elapsed));
    return { ok: false, latency: elapsed, error: err.message };
  }
}

function meanLatency() {
  return totalRequests > 0 ? (totalLatency / totalRequests).toFixed(1) : 0;
}

function successRate() {
  return totalRequests > 0
    ? ((totalSuccess / totalRequests) * 100).toFixed(2)
    : "0.00";
}

function printStats(label) {
  console.log(`\n📊 ${label}`);
  console.log(`   总请求: ${totalRequests} | 成功: ${totalSuccess} | 失败: ${totalFail}`);
  console.log(`   成功率: ${successRate()}%`);
  console.log(`   平均延迟: ${meanLatency()}ms | 最小: ${minLatency.toFixed(1)}ms | 最大: ${maxLatency.toFixed(1)}ms`);
}

function resetStats() {
  totalRequests = 0;
  totalSuccess = 0;
  totalFail = 0;
  totalLatency = 0;
  minLatency = Infinity;
  maxLatency = -Infinity;
}

async function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function loginWithCsrf() {
  const challengeResponse = await fetch(`${BASE_URL}/api/v1/auth/csrf`);
  if (!challengeResponse.ok) throw new Error(`CSRF HTTP ${challengeResponse.status}`);
  const challenge = await challengeResponse.json();
  const csrfToken = challenge?.data?.csrf_token;
  const setCookies = typeof challengeResponse.headers.getSetCookie === "function"
    ? challengeResponse.headers.getSetCookie()
    : [challengeResponse.headers.get("set-cookie")].filter(Boolean);
  const cookie = setCookies.map((value) => value.split(";", 1)[0]).join("; ");
  if (!csrfToken || !cookie) throw new Error("CSRF challenge contract is incomplete");

  return fetch(`${BASE_URL}/api/v1/auth/login`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      "X-Tenant-Id": "system",
      "X-CSRF-Token": csrfToken,
      Cookie: cookie,
    },
    body: JSON.stringify({ username: ADMIN_USER, password: ADMIN_PASS }),
  });
}

// ============================================================
// 测试场景
// ============================================================

/**
 * 场景 1：Health Check（轻量，快速建立基线）
 */
async function testHealthCheck({ concurrency, rounds }) {
  console.log(`\n🏥 [场景 1] Health Check —— ${concurrency} 并发 × ${rounds} 轮`);

  const tasks = [];
  for (let c = 0; c < concurrency; c++) {
    tasks.push(
      (async () => {
        for (let r = 0; r < rounds; r++) {
          await timedRequest("health", () =>
            fetch(`${BASE_URL}/livez`).then((r) => {
              if (!r.ok) throw new Error(`HTTP ${r.status}`);
              return r.text();
            })
          );
        }
      })()
    );
  }
  await Promise.all(tasks);
  printStats("场景 1 - Health Check");
}

/**
 * 场景 2：登录接口（认证压力）
 */
async function testLogin({ concurrency, rounds }) {
  console.log(`\n🔐 [场景 2] 登录接口 —— ${concurrency} 并发 × ${rounds} 轮`);

  const tasks = [];
  for (let c = 0; c < concurrency; c++) {
    tasks.push(
      (async () => {
        for (let r = 0; r < rounds; r++) {
          await timedRequest("login", () =>
            loginWithCsrf().then((r) => {
              if (!r.ok) throw new Error(`HTTP ${r.status}`);
              return r.json();
            })
          );
        }
      })()
    );
  }
  await Promise.all(tasks);
  printStats("场景 2 - 登录接口");
}

/**
 * 场景 3：公开 API（版本信息端点，验证基础吞吐量）
 */
async function testPublicApi({ concurrency, rounds }) {
  console.log(`\n🌐 [场景 3] 公开 API —— ${concurrency} 并发 × ${rounds} 轮`);

  const tasks = [];
  for (let c = 0; c < concurrency; c++) {
    tasks.push(
      (async () => {
        for (let r = 0; r < rounds; r++) {
          await timedRequest("version", () =>
            fetch(`${BASE_URL}/api/v1/version`).then((r) => {
              if (!r.ok) throw new Error(`HTTP ${r.status}`);
              return r.json();
            })
          );
        }
      })()
    );
  }
  await Promise.all(tasks);
  printStats("场景 3 - 公开 API");
}

/**
 * 场景 4：Metrics 端点（监控端点压力）
 */
async function testMetrics({ concurrency, rounds }) {
  console.log(`\n📈 [场景 4] Metrics 端点 —— ${concurrency} 并发 × ${rounds} 轮`);

  const tasks = [];
  for (let c = 0; c < concurrency; c++) {
    tasks.push(
      (async () => {
        for (let r = 0; r < rounds; r++) {
          await timedRequest("metrics", () =>
            fetch(`${BASE_URL}/api/v1/monitor/metrics`).then((r) => {
              if (!r.ok) throw new Error(`HTTP ${r.status}`);
              return r.text();
            })
          );
        }
      })()
    );
  }
  await Promise.all(tasks);
  printStats("场景 4 - Metrics");
}

/**
 * 场景 5：混合负载（模拟真实流量）
 */
async function testMixedLoad({ concurrency, rounds }) {
  console.log(`\n🎯 [场景 5] 混合负载 —— ${concurrency} 并发 × ${rounds} 轮`);

  const endpoints = [
    { weight: 40, name: "liveness", fn: () => fetch(`${BASE_URL}/livez`) },
    {
      weight: 20,
      name: "version",
      fn: () => fetch(`${BASE_URL}/api/v1/version`),
    },
    {
      weight: 15,
      name: "openapi",
      fn: () =>
        fetch(`${BASE_URL}/api/v1/api-docs/openapi.json`).then((r) => {
          if (!r.ok) throw new Error(`HTTP ${r.status}`);
          return r.json();
        }),
    },
    {
      weight: 10,
      name: "metrics",
      fn: () => fetch(`${BASE_URL}/api/v1/monitor/metrics`),
    },
    {
      weight: 15,
      name: "login",
      fn: loginWithCsrf,
    },
  ];

  // 构建加权选择器
  const totalWeight = endpoints.reduce((s, e) => s + e.weight, 0);
  function pickEndpoint() {
    let r = Math.random() * totalWeight;
    for (const ep of endpoints) {
      r -= ep.weight;
      if (r <= 0) return ep;
    }
    return endpoints[0];
  }

  const tasks = [];
  for (let c = 0; c < concurrency; c++) {
    tasks.push(
      (async () => {
        for (let r = 0; r < rounds; r++) {
          const ep = pickEndpoint();
          await timedRequest(ep.name, ep.fn);
        }
      })()
    );
  }
  await Promise.all(tasks);
  printStats("场景 5 - 混合负载");
}

// ============================================================
// 主函数
// ============================================================
async function main() {
  console.log("=".repeat(60));
  console.log("RyFrame 压力测试");
  console.log("=".repeat(60));
  console.log(`目标地址: ${BASE_URL}`);
  console.log(`测试时间: ${new Date().toISOString()}`);
  console.log("=".repeat(60));

  // ---- Phase 1: 预热 ----
  console.log("\n🔥 Phase 1: 预热（建立连接池）");
  resetStats();
  await testHealthCheck({ concurrency: 5, rounds: 10 });

  // ---- Phase 2: 低负载 ----
  console.log("\n\n🔵 Phase 2: 低负载");
  resetStats();
  await testHealthCheck({ concurrency: 10, rounds: 100 });
  resetStats();
  await testPublicApi({ concurrency: 10, rounds: 100 });

  // ---- Phase 3: 中负载 ----
  console.log("\n\n🟡 Phase 3: 中负载");
  resetStats();
  await testHealthCheck({ concurrency: 50, rounds: 100 });
  resetStats();
  await testLogin({ concurrency: 20, rounds: 50 });

  // ---- Phase 4: 高负载 ----
  console.log("\n\n🔴 Phase 4: 高负载");
  resetStats();
  await testHealthCheck({ concurrency: 100, rounds: 100 });
  resetStats();
  await testPublicApi({ concurrency: 100, rounds: 100 });
  resetStats();
  await testMetrics({ concurrency: 50, rounds: 100 });

  // ---- Phase 5: 混合负载 ----
  console.log("\n\n🌈 Phase 5: 混合负载（模拟真实流量）");
  resetStats();
  await testMixedLoad({ concurrency: 100, rounds: 100 });

  // ---- 总结 ----
  console.log("\n\n" + "=".repeat(60));
  console.log("测试完成！");
  console.log("=".repeat(60));
  console.log(`目标: ${BASE_URL}`);
  console.log(`时间: ${new Date().toISOString()}`);
  console.log("\n💡 提示:");
  console.log("   - 查看 Prometheus:  " + BASE_URL + "/api/v1/monitor/metrics");
  console.log("   - 查看 Grafana:    查看 Dashboard 中的 QPS/延迟/错误率面板");
  console.log("   - 压测后务必检查:  数据库连接池是否恢复正常");
}

main().catch((err) => {
  console.error("❌ 压测异常:", err.message);
  process.exit(1);
});
