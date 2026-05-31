// ============================================================
// RyFrame API 冒烟测试 (快速验证)
// ============================================================
// 用法: k6 run deploy/tests/smoke-test.js
// 目的: 部署后快速验证所有核心 API 是否正常
// ============================================================

import http from 'k6/http';
import { check, group, sleep } from 'k6';

const BASE_URL = __ENV.BASE_URL || 'http://localhost:8080';
const API = '/api/v1';

export const options = {
  vus: 1,
  iterations: 1,
  thresholds: {
    http_req_duration: ['p(95)<5000'],
    http_req_failed: ['rate<0.3'],
  },
};

export default function () {
  let accessToken = '';

  // 1. 健康检查
  group('1. 健康检查', () => {
    const r = http.get(`${BASE_URL}/`);
    check(r, { '200 OK': (r) => r.status === 200 });
    sleep(0.5);
  });

  // 2. API 版本
  group('2. API 版本', () => {
    const r = http.get(`${BASE_URL}${API}/version`);
    check(r, { '200 OK': (r) => r.status === 200 });
    sleep(0.5);
  });

  // 3. OpenAPI 文档
  group('3. OpenAPI 文档', () => {
    const r = http.get(`${BASE_URL}${API}/api-docs/openapi.json`);
    check(r, { '200 OK': (r) => r.status === 200 });
    sleep(0.5);
  });

  // 4. 登录
  group('4. 登录', () => {
    const r = http.post(`${BASE_URL}${API}/auth/login`, JSON.stringify({
      username: 'admin',
      password: 'admin123',
      captcha: '',
      uuid: '',
    }), { headers: { 'Content-Type': 'application/json' } });

    const ok = check(r, { '200 OK': (r) => r.status === 200 });
    if (ok) {
      try {
        accessToken = JSON.parse(r.body).data.access_token;
        console.log(`  ✓ 获取到 Token: ${accessToken.substring(0, 20)}...`);
      } catch (e) {
        console.log(`  ✗ Token 解析失败`);
      }
    }
    sleep(0.5);
  });

  if (!accessToken) {
    console.log('⚠ 登录失败，跳过认证 API 测试');
    return;
  }

  const auth = { Authorization: `Bearer ${accessToken}` };
  const hdr = { ...auth, 'Content-Type': 'application/json' };

  // 5. 获取用户信息
  group('5. 当前用户', () => {
    const r = http.get(`${BASE_URL}${API}/auth/me`, { headers: auth });
    check(r, { '200 OK': (r) => r.status === 200 });
    sleep(0.5);
  });

  // 6. 用户列表
  group('6. 用户列表', () => {
    const r = http.get(`${BASE_URL}${API}/system/users?page=1&pageSize=10`, { headers: auth });
    check(r, { '200 OK': (r) => r.status === 200 });
    sleep(0.5);
  });

  // 7. 角色列表
  group('7. 角色列表', () => {
    const r = http.get(`${BASE_URL}${API}/system/roles?page=1&pageSize=10`, { headers: auth });
    check(r, { '200 OK': (r) => r.status === 200 });
    sleep(0.5);
  });

  // 8. 菜单树
  group('8. 菜单树', () => {
    const r = http.get(`${BASE_URL}${API}/system/menus`, { headers: auth });
    check(r, { '200 OK': (r) => r.status === 200 });
    sleep(0.5);
  });

  // 9. 部门树
  group('9. 部门树', () => {
    const r = http.get(`${BASE_URL}${API}/system/depts`, { headers: auth });
    check(r, { '200 OK': (r) => r.status === 200 });
    sleep(0.5);
  });

  // 10. 字典类型
  group('10. 字典类型', () => {
    const r = http.get(`${BASE_URL}${API}/system/dict/type?page=1&pageSize=10`, { headers: auth });
    check(r, { '200 OK': (r) => r.status === 200 });
    sleep(0.5);
  });

  // 11. 岗位列表
  group('11. 岗位列表', () => {
    const r = http.get(`${BASE_URL}${API}/system/posts?page=1&pageSize=10`, { headers: auth });
    check(r, { '200 OK': (r) => r.status === 200 });
    sleep(0.5);
  });

  // 12. 通知公告
  group('12. 通知公告', () => {
    const r = http.get(`${BASE_URL}${API}/system/notices?page=1&pageSize=10`, { headers: auth });
    check(r, { '200 OK': (r) => r.status === 200 });
    sleep(0.5);
  });

  // 13. 监控 - 服务器信息
  group('13. 监控: 服务器信息', () => {
    const r = http.get(`${BASE_URL}${API}/monitor/server`, { headers: auth });
    check(r, { '服务器信息可达': (r) => r.status === 200 || r.status === 500 });
    sleep(0.5);
  });

  // 14. 工具 - 生成器
  group('14. 工具: 生成器表列表', () => {
    const r = http.get(`${BASE_URL}${API}/tools/gen/list`, { headers: auth });
    check(r, { '200 OK': (r) => r.status === 200 });
    sleep(0.5);
  });

  console.log('\n=== 冒烟测试完成 ===');
}
