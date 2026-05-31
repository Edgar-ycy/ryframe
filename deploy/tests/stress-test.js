// ============================================================
// RyFrame k6 性能压测脚本
// ============================================================
// 用法:
//   k6 run deploy/tests/stress-test.js
//   k6 run --vus 50 --duration 60s deploy/tests/stress-test.js
//   k6 run --vus 100 --duration 5m --env BASE_URL=http://prod:8080 deploy/tests/stress-test.js
//
// 场景:
//   - smoke:  1 VU, 30s  (冒烟测试)
//   - load:   50 VU, 5m   (负载测试)
//   - stress: 200 VU, 10m (压力测试)
//   - spike:  500 VU, 30s  (峰值测试)
// ============================================================

import http from 'k6/http';
import { check, sleep, group } from 'k6';
import { Counter, Trend, Rate } from 'k6/metrics';
import { randomString, randomIntBetween } from 'https://jslib.k6.io/k6-utils/1.2.0/index.js';

// ============ 配置 ============

const BASE_URL = __ENV.BASE_URL || 'http://localhost:8080';
const API_PREFIX = '/api/v1';

// 自定义指标
const loginFailures = new Counter('login_failures');
const loginSuccesses = new Counter('login_successes');
const authLatency = new Trend('auth_latency', true);
const apiLatency = new Trend('api_latency', true);
const errorRate = new Rate('error_rate');

// 压测账号池
const TEST_USERS = [
  { username: 'admin', password: 'admin123' },
  { username: 'ry', password: 'admin123' },
];

// ============ K6 配置 ============

export const options = {
  // 冒烟测试 (默认)
  scenarios: {
    default: {
      executor: 'ramping-vus',
      startVUs: 1,
      stages: [
        { duration: '30s', target: 10 },   // 预热
        { duration: '1m', target: 50 },    // 爬升
        { duration: '2m', target: 50 },    // 稳定
        { duration: '30s', target: 0 },    // 冷却
      ],
      gracefulRampDown: '10s',
    },
  },

  thresholds: {
    // HTTP 请求失败率 < 5%
    http_req_failed: ['rate<0.05'],
    // 登录 P95 延迟 < 2s
    'auth_latency': ['p(95)<2000'],
    // API P95 延迟 < 1s
    'api_latency': ['p(95)<1000'],
    // 错误率 < 5%
    'error_rate': ['rate<0.05'],
  },

  // 摘要输出
  summaryTrendStats: ['avg', 'min', 'med', 'p(90)', 'p(95)', 'p(99)', 'max'],
};

// ============ Setup: 初始化 ============

export function setup() {
  console.log(`=== RyFrame 性能压测 ===`);
  console.log(`目标: ${BASE_URL}`);
  console.log(`场景: ${JSON.stringify(options.scenarios.default.stages)}`);

  // 健康检查
  const healthResp = http.get(`${BASE_URL}/`);
  check(healthResp, { '健康检查通过': (r) => r.status === 200 });

  return { baseUrl: BASE_URL };
}

// ============ 主测试逻辑 ============

export default function (data) {
  const baseUrl = data.baseUrl;

  // ========== 场景 1: 健康检查 ==========
  group('健康检查', () => {
    const resp = http.get(`${baseUrl}/`);
    const ok = check(resp, {
      'GET /: status 200': (r) => r.status === 200,
    });
    errorRate.add(!ok);
    sleep(0.5);
  });

  // ========== 场景 2: 认证流程 ==========
  let accessToken = '';

  group('认证流程', () => {
    // 2.1 登录
    const user = TEST_USERS[Math.floor(Math.random() * TEST_USERS.length)];
    const loginResp = http.post(`${baseUrl}${API_PREFIX}/auth/login`, JSON.stringify({
      username: user.username,
      password: user.password,
      captcha: '',
      uuid: '',
    }), {
      headers: { 'Content-Type': 'application/json' },
      tags: { name: 'login' },
    });

    const loginOk = check(loginResp, {
      'POST /auth/login: status 200': (r) => r.status === 200,
    });

    authLatency.add(loginResp.timings.duration);

    if (loginOk) {
      try {
        const body = JSON.parse(loginResp.body);
        if (body.data && body.data.access_token) {
          accessToken = body.data.access_token;
          loginSuccesses.add(1);
        } else {
          loginFailures.add(1);
          errorRate.add(true);
        }
      } catch (e) {
        loginFailures.add(1);
        errorRate.add(true);
      }
    } else {
      loginFailures.add(1);
      errorRate.add(true);
      // 登录失败则跳过后续 API 测试
      return;
    }

    sleep(0.5);

    // 2.2 Token 验证 (me)
    if (accessToken) {
      const meResp = http.get(`${baseUrl}${API_PREFIX}/auth/me`, {
        headers: { Authorization: `Bearer ${accessToken}` },
        tags: { name: 'auth/me' },
      });

      check(meResp, {
        'GET /auth/me: status 200': (r) => r.status === 200,
      });
      authLatency.add(meResp.timings.duration);
    }

    sleep(0.5);
  });

  // ========== 场景 3: 系统管理 API ==========
  if (accessToken) {
    const authHeaders = {
      Authorization: `Bearer ${accessToken}`,
      'Content-Type': 'application/json',
    };

    // 3.1 用户列表
    group('系统管理: 用户', () => {
      const pageSize = randomIntBetween(5, 20);
      const resp = http.get(
        `${baseUrl}${API_PREFIX}/system/users?page=1&pageSize=${pageSize}`,
        { headers: authHeaders, tags: { name: 'system/users' } }
      );

      check(resp, {
        'GET /system/users: status 200': (r) => r.status === 200,
      });
      apiLatency.add(resp.timings.duration);
      sleep(0.5);
    });

    // 3.2 角色列表
    group('系统管理: 角色', () => {
      const resp = http.get(
        `${baseUrl}${API_PREFIX}/system/roles?page=1&pageSize=10`,
        { headers: authHeaders, tags: { name: 'system/roles' } }
      );

      check(resp, {
        'GET /system/roles: status 200': (r) => r.status === 200,
      });
      apiLatency.add(resp.timings.duration);
      sleep(0.5);
    });

    // 3.3 菜单列表
    group('系统管理: 菜单', () => {
      const resp = http.get(
        `${baseUrl}${API_PREFIX}/system/menus`,
        { headers: authHeaders, tags: { name: 'system/menus' } }
      );

      check(resp, {
        'GET /system/menus: status 200': (r) => r.status === 200,
      });
      apiLatency.add(resp.timings.duration);
      sleep(0.5);
    });

    // 3.4 部门列表
    group('系统管理: 部门', () => {
      const resp = http.get(
        `${baseUrl}${API_PREFIX}/system/depts`,
        { headers: authHeaders, tags: { name: 'system/depts' } }
      );

      check(resp, {
        'GET /system/depts: status 200': (r) => r.status === 200,
      });
      apiLatency.add(resp.timings.duration);
      sleep(0.5);
    });

    // 3.5 字典列表
    group('系统管理: 字典', () => {
      const resp = http.get(
        `${baseUrl}${API_PREFIX}/system/dict/type?page=1&pageSize=10`,
        { headers: authHeaders, tags: { name: 'system/dict' } }
      );

      check(resp, {
        'GET /system/dict/type: status 200': (r) => r.status === 200,
      });
      apiLatency.add(resp.timings.duration);
      sleep(0.5);
    });

    // 3.6 通知公告
    group('系统管理: 通知', () => {
      const resp = http.get(
        `${baseUrl}${API_PREFIX}/system/notices?page=1&pageSize=10`,
        { headers: authHeaders, tags: { name: 'system/notices' } }
      );

      check(resp, {
        'GET /system/notices: status 200': (r) => r.status === 200,
      });
      apiLatency.add(resp.timings.duration);
      sleep(0.5);
    });

    // 3.7 API 版本
    group('通用: API 版本', () => {
      const resp = http.get(
        `${baseUrl}${API_PREFIX}/version`,
        { tags: { name: 'version' } }
      );

      check(resp, {
        'GET /version: status 200': (r) => r.status === 200,
      });
      sleep(0.3);
    });

    // 3.8 OpenAPI 文档
    group('通用: OpenAPI 文档', () => {
      const resp = http.get(
        `${baseUrl}${API_PREFIX}/api-docs/openapi.json`,
        { tags: { name: 'openapi' } }
      );

      check(resp, {
        'GET /api-docs: status 200': (r) => r.status === 200,
      });
      sleep(0.3);
    });
  }
}

// ============ Teardown ============

export function teardown(data) {
  console.log('=== 压测完成 ===');
  console.log(`登录成功: ${loginSuccesses.value} / 失败: ${loginFailures.value}`);
}

// ============ 预定义场景快捷入口 ============

// 冒烟测试: k6 run --env SCENARIO=smoke deploy/tests/stress-test.js
const SCENARIO = __ENV.SCENARIO || '';

if (SCENARIO === 'smoke') {
  options.scenarios.default.stages = [
    { duration: '10s', target: 1 },
    { duration: '20s', target: 1 },
  ];
} else if (SCENARIO === 'load') {
  options.scenarios.default.stages = [
    { duration: '1m', target: 50 },
    { duration: '3m', target: 50 },
    { duration: '1m', target: 0 },
  ];
} else if (SCENARIO === 'stress') {
  options.scenarios.default.stages = [
    { duration: '2m', target: 100 },
    { duration: '5m', target: 200 },
    { duration: '3m', target: 0 },
  ];
} else if (SCENARIO === 'spike') {
  options.scenarios.default.stages = [
    { duration: '10s', target: 500 },
    { duration: '20s', target: 500 },
    { duration: '30s', target: 0 },
  ];
}
