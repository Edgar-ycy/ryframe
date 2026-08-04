import { randomUUID } from "node:crypto";
import { link, open, realpath, stat, unlink } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const INTERNAL_TOKEN = "RUN-RYFRAME-V0-7-REPLICA-CLIENT";
const REPOSITORY_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const ACCEPTANCE_TARGET_ROOT = path.join(REPOSITORY_ROOT, "target", "runtime-acceptance-0-7");
const ALLOWED_ARGUMENTS = new Set([
  "--api-base",
  "--evidence",
  "--evidence-root",
  "--expected-state",
  "--internal-token",
  "--ready-evidence",
  "--replica-nickname",
  "--sentinel-id",
  "--sentinel-user",
  "--stability-seconds",
]);

function fail(message) {
  throw new Error(message);
}

function parseArguments(values) {
  const parsed = new Map();
  for (let index = 0; index < values.length; index += 2) {
    const name = values[index];
    const value = values[index + 1];
    if (!name?.startsWith("--") || value === undefined) {
      fail("副本验收客户端参数必须使用成对的 --名称 值格式");
    }
    if (!ALLOWED_ARGUMENTS.has(name)) fail(`副本验收客户端参数不受支持：${name}`);
    if (parsed.has(name)) fail(`副本验收客户端参数重复：${name}`);
    parsed.set(name, value);
  }
  for (const required of [
    "--api-base",
    "--evidence",
    "--evidence-root",
    "--expected-state",
    "--internal-token",
    "--sentinel-user",
    "--sentinel-id",
    "--replica-nickname",
  ]) {
    if (!parsed.has(required)) fail(`副本验收客户端缺少参数：${required}`);
  }
  if (parsed.get("--internal-token") !== INTERNAL_TOKEN) {
    fail("副本验收客户端内部确认令牌不匹配");
  }
  const expectedState = parsed.get("--expected-state");
  if (!new Set([
    "healthy",
    "fallback",
    "failure-threshold",
    "recovery-threshold",
  ]).has(expectedState)) {
    fail(`不支持的副本预期状态：${expectedState}`);
  }
  const rawApiBase = parsed.get("--api-base");
  const apiBase = new URL(rawApiBase);
  const apiPort = Number(apiBase.port);
  if (
    rawApiBase !== apiBase.origin
    || apiBase.protocol !== "http:"
    || apiBase.hostname !== "127.0.0.1"
    || !Number.isInteger(apiPort)
    || apiPort < 1
    || apiPort > 65_535
  ) fail("API 地址必须精确为 http://127.0.0.1:<port>");
  const stabilitySeconds = Number.parseInt(parsed.get("--stability-seconds") ?? "0", 10);
  if (!Number.isInteger(stabilitySeconds) || stabilitySeconds < 0 || stabilitySeconds > 60) {
    fail("稳定观察时间必须是 0-60 秒之间的整数");
  }
  const readyEvidenceValue = parsed.get("--ready-evidence");
  const thresholdObservation = expectedState.endsWith("-threshold");
  if (thresholdObservation !== (readyEvidenceValue !== undefined)) {
    fail("阈值观察状态必须且只能提供就绪证据路径");
  }
  for (const name of ["--evidence", "--evidence-root", "--ready-evidence"]) {
    const value = parsed.get(name);
    if (value !== undefined && !path.isAbsolute(value)) {
      fail(`副本验收路径参数必须是绝对路径：${name}`);
    }
  }
  const evidence = path.resolve(parsed.get("--evidence"));
  const evidenceRoot = path.resolve(parsed.get("--evidence-root"));
  const readyEvidence = readyEvidenceValue === undefined ? null : path.resolve(readyEvidenceValue);
  assertPathWithinAcceptanceTarget(evidenceRoot, "副本验收证据目录");
  assertPathWithinAcceptanceTarget(evidence, "副本验收证据文件");
  if (readyEvidence) assertPathWithinAcceptanceTarget(readyEvidence, "副本验收就绪证据文件");
  return {
    apiBase: new URL(`${apiBase.origin}/`),
    evidence,
    evidenceRoot,
    expectedState,
    readyEvidence,
    sentinelUser: parsed.get("--sentinel-user"),
    sentinelId: parsed.get("--sentinel-id"),
    replicaNickname: parsed.get("--replica-nickname"),
    stabilitySeconds,
  };
}

function comparablePath(value) {
  return process.platform === "win32" ? value.toLowerCase() : value;
}

function assertPathWithinAcceptanceTarget(candidate, label) {
  const target = comparablePath(path.resolve(ACCEPTANCE_TARGET_ROOT));
  const resolved = comparablePath(path.resolve(candidate));
  if (!resolved.startsWith(`${target}${path.sep}`)) {
    fail(`${label} 必须位于 target/runtime-acceptance-0-7 内`);
  }
}

async function fileExists(filePath) {
  try {
    await stat(filePath);
    return true;
  } catch (error) {
    if (error?.code === "ENOENT") return false;
    throw error;
  }
}

async function validateEvidencePaths(options) {
  const [canonicalTarget, canonicalRoot] = await Promise.all([
    realpath(ACCEPTANCE_TARGET_ROOT),
    realpath(options.evidenceRoot),
  ]);
  const target = comparablePath(canonicalTarget);
  const root = comparablePath(canonicalRoot);
  if (!root.startsWith(`${target}${path.sep}`)) {
    fail("副本验收证据目录的真实路径越出专用 target 根目录");
  }
  if (!(await stat(canonicalRoot)).isDirectory()) fail("副本验收证据根路径不是目录");
  const candidates = [options.evidence, options.readyEvidence].filter(Boolean);
  for (const candidate of candidates) {
    const canonicalParent = await realpath(path.dirname(candidate));
    if (
      comparablePath(canonicalParent) !== root
      || comparablePath(path.dirname(candidate)) !== comparablePath(options.evidenceRoot)
    ) {
      fail(`证据文件必须是指定证据根目录的直接子文件：${candidate}`);
    }
    if (await fileExists(candidate)) fail(`副本验收证据已存在，拒绝覆盖：${candidate}`);
  }
  if (options.readyEvidence === options.evidence) {
    fail("就绪证据与结果证据不能使用同一路径");
  }
}

async function writeJsonExclusively(filePath, value) {
  const temporaryPath = `${filePath}.${process.pid}.${randomUUID()}.tmp`;
  const handle = await open(temporaryPath, "wx");
  try {
    await handle.writeFile(`${JSON.stringify(value, null, 2)}\n`, "utf8");
    await handle.sync();
  } finally {
    await handle.close();
  }
  try {
    await link(temporaryPath, filePath);
  } catch (error) {
    if (error?.code === "EEXIST") fail(`副本验收证据已存在，拒绝覆盖：${filePath}`);
    throw error;
  } finally {
    await unlink(temporaryPath).catch(() => {});
  }
}

function sleep(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

class CookieJar {
  constructor() {
    this.values = new Map();
  }

  capture(response) {
    const values = typeof response.headers.getSetCookie === "function"
      ? response.headers.getSetCookie()
      : [response.headers.get("set-cookie")].filter(Boolean);
    for (const value of values) {
      const [pair, ...attributes] = value.split(";");
      const separator = pair.indexOf("=");
      if (separator < 1) continue;
      const name = pair.slice(0, separator).trim();
      const cookieValue = pair.slice(separator + 1).trim();
      const expired = attributes.some((item) => /^\s*Max-Age=0\s*$/i.test(item));
      if (expired || cookieValue === "") this.values.delete(name);
      else this.values.set(name, cookieValue);
    }
  }

  apply(headers) {
    if (this.values.size > 0) {
      headers.set(
        "Cookie",
        [...this.values].map(([name, value]) => `${name}=${value}`).join("; "),
      );
    }
  }
}

async function request(apiBase, pathname, options = {}, jar = null) {
  const headers = new Headers(options.headers || {});
  if (jar) jar.apply(headers);
  const response = await fetch(new URL(pathname, apiBase), { ...options, headers });
  if (jar) jar.capture(response);
  return response;
}

async function requestJson(apiBase, pathname, options = {}, jar = null) {
  const response = await request(apiBase, pathname, options, jar);
  const text = await response.text();
  let json = null;
  try {
    json = text === "" ? null : JSON.parse(text);
  } catch {
    fail(`${pathname} 返回了无效 JSON：${text.slice(0, 300)}`);
  }
  if (!response.ok || json?.code !== 200) {
    fail(`${pathname} 请求失败，HTTP ${response.status}：${text.slice(0, 500)}`);
  }
  return json;
}

function authHeaders(token) {
  return {
    Authorization: `Bearer ${token}`,
    "X-Tenant-Id": "system",
  };
}

async function login(apiBase) {
  const jar = new CookieJar();
  const challenge = await requestJson(apiBase, "/api/v1/auth/csrf", {}, jar);
  const csrfToken = challenge?.data?.csrf_token;
  if (!csrfToken) fail("登录前的 CSRF 响应缺少令牌");
  const response = await requestJson(
    apiBase,
    "/api/v1/auth/login",
    {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-Tenant-Id": "system",
        "X-CSRF-Token": csrfToken,
      },
      body: JSON.stringify({ username: "admin", password: "123456" }),
    },
    jar,
  );
  const token = response?.data?.access_token;
  if (!token) fail("登录响应缺少访问令牌");
  return { jar, token };
}

async function runtimeState(options, session) {
  const response = await requestJson(
    options.apiBase,
    "/api/v1/monitor/runtime",
    { headers: authHeaders(session.token) },
    session.jar,
  );
  const database = response?.data?.database;
  if (!database || !Array.isArray(database.replicas)) {
    fail("运行时监控响应缺少数据库拓扑");
  }
  return database;
}

function topologyMatches(database, expectedState) {
  const replica = database.replicas.find((item) => item?.name === "replica-a");
  if (!replica || database.replica_count !== 1) return false;
  if (
    !Number.isInteger(replica.consecutive_failures)
    || replica.consecutive_failures < 0
    || !Number.isInteger(replica.consecutive_successes)
    || replica.consecutive_successes < 0
  ) {
    return false;
  }
  if (expectedState === "healthy") {
    return replica.connected === true && database.read_policy === "round_robin";
  }
  return replica.connected === false && database.read_policy === "primary_fallback";
}

function replicaSnapshot(database) {
  const replica = database.replicas.find((item) => item?.name === "replica-a");
  if (!replica || database.replica_count !== 1) {
    fail(`运行时监控响应缺少唯一副本 replica-a：${JSON.stringify(topologySummary(database))}`);
  }
  for (const field of ["consecutive_failures", "consecutive_successes"]) {
    if (!Number.isInteger(replica[field]) || replica[field] < 0) {
      fail(`副本探测计数 ${field} 无效：${JSON.stringify(replica)}`);
    }
  }
  return {
    connected: replica.connected,
    consecutive_failures: replica.consecutive_failures,
    consecutive_successes: replica.consecutive_successes,
    read_policy: database.read_policy,
    observed_at: new Date().toISOString(),
  };
}

function topologySummary(database) {
  return {
    connected: database.connected,
    primary_connected: database.primary_connected,
    replica_count: database.replica_count,
    replicas: database.replicas,
    read_policy: database.read_policy,
    read_fallback_total: database.read_fallback_total,
    read_selections: database.read_selections,
  };
}

async function waitForTopology(options, session) {
  const deadline = Date.now() + 90_000;
  let last = null;
  let lastError = null;
  while (Date.now() < deadline) {
    try {
      last = await runtimeState(options, session);
      if (topologyMatches(last, options.expectedState)) return last;
      lastError = null;
    } catch (error) {
      lastError = error;
    }
    await sleep(500);
  }
  const detail = lastError?.message ?? JSON.stringify(last && topologySummary(last));
  fail(`等待副本状态 ${options.expectedState} 超时：${detail}`);
}

function thresholdStepMatches(kind, step, snapshot) {
  if (kind === "failure") {
    return snapshot.consecutive_failures === step
      && snapshot.consecutive_successes === 0
      && snapshot.connected === (step < 3)
      && snapshot.read_policy === (step < 3 ? "round_robin" : "primary_fallback");
  }
  return snapshot.consecutive_successes === step
    && snapshot.consecutive_failures === 0
    && snapshot.connected === (step >= 2)
    && snapshot.read_policy === (step >= 2 ? "round_robin" : "primary_fallback");
}

async function observeProbeThreshold(options, session) {
  const kind = options.expectedState === "failure-threshold" ? "failure" : "recovery";
  const baselineState = kind === "failure" ? "healthy" : "fallback";
  const baseline = await waitForTopology({ ...options, expectedState: baselineState }, session);
  const baselineReplica = replicaSnapshot(baseline);
  if (
    kind === "failure"
      ? baselineReplica.consecutive_failures !== 0
      : baselineReplica.consecutive_failures < 3
  ) {
    fail(`阈值观察基线不符合预期：${JSON.stringify(baselineReplica)}`);
  }
  await writeJsonExclusively(options.readyEvidence, {
    schema_version: 1,
    status: "ready",
    expected_state: options.expectedState,
    baseline: topologySummary(baseline),
  });

  const steps = kind === "failure" ? [1, 2, 3] : [1, 2];
  const observations = [];
  const deadline = Date.now() + 120_000;
  let nextIndex = 0;
  let lastSnapshot = null;
  while (Date.now() < deadline && nextIndex < steps.length) {
    const database = await runtimeState(options, session);
    const snapshot = replicaSnapshot(database);
    lastSnapshot = snapshot;
    const expectedStep = steps[nextIndex];
    const observedStep = kind === "failure"
      ? snapshot.consecutive_failures
      : snapshot.consecutive_successes;

    if (observedStep > expectedStep) {
      fail(`错过副本 ${kind} 连续阈值 ${expectedStep}：${JSON.stringify(snapshot)}`);
    }
    if (nextIndex > 0 && observedStep < steps[nextIndex - 1]) {
      fail(`副本 ${kind} 连续探测在达到阈值前被中断：${JSON.stringify(snapshot)}`);
    }
    if (observedStep === expectedStep) {
      if (thresholdStepMatches(kind, expectedStep, snapshot)) {
        observations.push(snapshot);
        nextIndex += 1;
      }
    }
    if (nextIndex < steps.length) await sleep(100);
  }
  if (nextIndex !== steps.length) {
    fail(
      `等待副本 ${kind} 连续探测阈值超时，仅采集 ${nextIndex}/${steps.length} 个状态：`
      + JSON.stringify(lastSnapshot),
    );
  }
  return { kind, baseline: topologySummary(baseline), observations };
}

async function assertTopologyStability(options, session) {
  if (options.stabilitySeconds === 0) return 0;
  const deadline = Date.now() + options.stabilitySeconds * 1000;
  let observations = 0;
  while (Date.now() < deadline) {
    const database = await runtimeState(options, session);
    if (!topologyMatches(database, options.expectedState)) {
      fail(`稳定观察期间副本意外改变状态：${JSON.stringify(topologySummary(database))}`);
    }
    observations += 1;
    await sleep(500);
  }
  const minimumObservations = Math.max(3, Math.floor(options.stabilitySeconds / 2));
  if (observations < minimumObservations) {
    fail(`稳定观察证据不足：仅采集 ${observations} 次`);
  }
  return observations;
}

function selectionCount(database, target, reason) {
  const item = database.read_selections.find(
    (selection) => selection?.target === target && selection?.reason === reason,
  );
  return Number(item?.count ?? 0);
}

function delta(after, before, target, reason) {
  return selectionCount(after, target, reason) - selectionCount(before, target, reason);
}

async function assertEventualRouting(options, session) {
  const before = await runtimeState(options, session);
  const query = new URLSearchParams({
    page: "1",
    page_size: "10",
    user_name: options.sentinelUser,
  });
  const response = await requestJson(
    options.apiBase,
    `/api/v1/system/loginlogs?${query}`,
    { headers: authHeaders(session.token) },
    session.jar,
  );
  const items = response?.data?.items;
  if (!Array.isArray(items)) fail("登录日志响应缺少分页数据");
  const sentinelItems = items.filter(
    (item) => item?.user_name === options.sentinelUser && String(item?.id) === options.sentinelId,
  );
  const after = await runtimeState(options, session);
  const routing = {
    replica_delta: delta(after, before, "replica", "replica"),
    fallback_selection_delta: delta(after, before, "primary", "fallback"),
    fallback_total_delta: Number(after.read_fallback_total) - Number(before.read_fallback_total),
    sentinel_count: sentinelItems.length,
  };
  if (options.expectedState === "healthy") {
    if (
      routing.replica_delta !== 1
      || routing.fallback_selection_delta !== 0
      || routing.fallback_total_delta !== 0
      || routing.sentinel_count !== 1
    ) {
      fail(`健康副本读路由证据不符合预期：${JSON.stringify(routing)}`);
    }
  } else if (
    routing.replica_delta !== 0
    || routing.fallback_selection_delta !== 1
    || routing.fallback_total_delta !== 1
    || routing.sentinel_count !== 0
  ) {
    fail(`主库回退读路由证据不符合预期：${JSON.stringify(routing)}`);
  }
  return routing;
}

async function assertStrongRouting(options, session) {
  const before = await runtimeState(options, session);
  const response = await requestJson(
    options.apiBase,
    "/api/v1/auth/profile",
    { headers: authHeaders(session.token) },
    session.jar,
  );
  const nickname = response?.data?.nickname;
  if (typeof nickname !== "string" || nickname === options.replicaNickname) {
    fail(`强一致性读取命中了副本专属数据：${JSON.stringify({ nickname })}`);
  }
  const after = await runtimeState(options, session);
  const routing = {
    primary_strong_delta: delta(after, before, "primary", "strong"),
    replica_delta: delta(after, before, "replica", "replica"),
    fallback_selection_delta: delta(after, before, "primary", "fallback"),
    nickname,
  };
  if (
    routing.primary_strong_delta < 2
    || routing.replica_delta !== 0
    || routing.fallback_selection_delta !== 0
  ) {
    fail(`强一致性读路由证据不符合预期：${JSON.stringify(routing)}`);
  }
  return routing;
}

async function main() {
  const options = parseArguments(process.argv.slice(2));
  await validateEvidencePaths(options);
  const session = await login(options.apiBase);
  if (options.expectedState.endsWith("-threshold")) {
    const threshold = await observeProbeThreshold(options, session);
    await writeJsonExclusively(options.evidence, {
      schema_version: 1,
      status: "passed",
      expected_state: options.expectedState,
      threshold,
    });
    return;
  }
  const topology = await waitForTopology(options, session);
  const stabilityObservations = await assertTopologyStability(options, session);
  const eventual = await assertEventualRouting(options, session);
  const strong = await assertStrongRouting(options, session);
  await writeJsonExclusively(options.evidence, {
    schema_version: 1,
    status: "passed",
    expected_state: options.expectedState,
    stability_seconds: options.stabilitySeconds,
    stability_observations: stabilityObservations,
    topology: topologySummary(topology),
    eventual,
    strong,
  });
}

main().catch((error) => {
  console.error(error?.stack || String(error));
  process.exitCode = 1;
});
