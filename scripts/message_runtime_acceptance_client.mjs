import { randomBytes, randomUUID } from "node:crypto";
import { link, open, readFile, realpath, stat, unlink } from "node:fs/promises";
import { connect } from "node:net";
import path from "node:path";
import { fileURLToPath } from "node:url";

const INTERNAL_ACCEPTANCE_TOKEN = "RUN-RYFRAME-V0-7-MESSAGE-CLIENT";
const REPOSITORY_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const ACCEPTANCE_TARGET_ROOT = path.join(REPOSITORY_ROOT, "target", "runtime-acceptance-0-7");

function fail(message) {
  throw new Error(message);
}

function parseArguments(values) {
  const parsed = new Map();
  for (let index = 0; index < values.length; index += 2) {
    const name = values[index];
    const value = values[index + 1];
    if (!name?.startsWith("--") || value === undefined) {
      fail("消息验收客户端参数必须使用成对的 --名称 值格式");
    }
    if (parsed.has(name)) fail(`消息验收客户端参数重复：${name}`);
    parsed.set(name, value);
  }
  for (const required of [
    "--internal-token",
    "--api-base",
    "--secondary-api-base",
    "--control-directory",
  ]) {
    if (!parsed.has(required)) fail(`消息验收客户端缺少参数：${required}`);
  }
  if (parsed.get("--internal-token") !== INTERNAL_ACCEPTANCE_TOKEN) {
    fail("消息验收客户端内部令牌不匹配");
  }
  const controlDirectory = path.resolve(parsed.get("--control-directory"));
  if (!path.isAbsolute(parsed.get("--control-directory"))) {
    fail("消息验收控制目录必须是绝对路径");
  }
  assertPathWithinAcceptanceTarget(controlDirectory, "消息验收控制目录");
  return {
    apiBase: parseLoopbackApiBase(parsed.get("--api-base"), "主 API"),
    secondaryApiBase: parseLoopbackApiBase(parsed.get("--secondary-api-base"), "次 API"),
    controlDirectory,
  };
}

function parseLoopbackApiBase(rawValue, label) {
  let url;
  try {
    url = new URL(rawValue);
  } catch {
    fail(`${label} 地址无效`);
  }
  const port = Number(url.port);
  if (
    rawValue !== url.origin
    || url.protocol !== "http:"
    || url.hostname !== "127.0.0.1"
    || !Number.isInteger(port)
    || port < 1
    || port > 65_535
  ) {
    fail(`${label} 地址必须精确为 http://127.0.0.1:<port>`);
  }
  return new URL(`${url.origin}/`);
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

function controlPath(controlDirectory, filename) {
  const resolved = path.resolve(controlDirectory, filename);
  assertPathWithinAcceptanceTarget(resolved, `控制文件 ${filename}`);
  if (path.dirname(resolved) !== controlDirectory) {
    fail(`控制文件 ${filename} 不能离开本次运行目录`);
  }
  return resolved;
}

async function assertPreflightFilesystem(controlDirectory, outputPaths) {
  const [realTarget, realControl] = await Promise.all([
    realpath(ACCEPTANCE_TARGET_ROOT),
    realpath(controlDirectory),
  ]);
  const target = comparablePath(realTarget);
  const control = comparablePath(realControl);
  if (!control.startsWith(`${target}${path.sep}`)) {
    fail("消息验收控制目录的真实路径越出专用 target 根目录");
  }
  const controlStat = await stat(realControl);
  if (!controlStat.isDirectory()) fail("消息验收控制路径不是目录");
  for (const outputPath of outputPaths) {
    assertPathWithinAcceptanceTarget(outputPath, "消息验收证据文件");
    if (await fileExists(outputPath)) {
      fail(`消息验收证据已存在，拒绝覆盖：${outputPath}`);
    }
  }
}

async function writeJsonAtomically(filePath, value) {
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
    if (error?.code === "EEXIST") {
      fail(`消息验收证据已存在，拒绝覆盖：${filePath}`);
    }
    throw error;
  } finally {
    await unlink(temporaryPath).catch(() => {});
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

async function readJson(filePath) {
  return JSON.parse(await readFile(filePath, "utf8"));
}

function sleep(milliseconds) {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

async function waitFor(description, timeoutMilliseconds, predicate, terminalError = null) {
  const deadline = Date.now() + timeoutMilliseconds;
  let lastError = null;
  while (Date.now() < deadline) {
    const fatalError = terminalError?.();
    if (fatalError) throw fatalError;
    try {
      const value = await predicate();
      if (value) return value;
      lastError = null;
    } catch (error) {
      lastError = error;
    }
    await sleep(100);
  }
  const suffix = lastError ? `；最后错误：${lastError.message}` : "";
  fail(`等待${description}超时${suffix}`);
}

function probeTerminalError(probes) {
  return probes.map((probe) => probe.state.error).find(Boolean) ?? null;
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

function authHeaders(token, locale = null, tenantId = "system") {
  const headers = {
    Authorization: `Bearer ${token}`,
    "X-Tenant-Id": tenantId,
  };
  if (locale) headers["Accept-Language"] = locale;
  return headers;
}

async function login(apiBase, tenantId = "system", username = "admin") {
  const jar = new CookieJar();
  const challenge = await requestJson(
    apiBase,
    "/api/v1/auth/csrf",
    { headers: { "X-Tenant-Id": tenantId } },
    jar,
  );
  const csrfToken = challenge?.data?.csrf_token;
  if (!csrfToken) fail("登录前的 CSRF 响应缺少令牌");
  const response = await requestJson(
    apiBase,
    "/api/v1/auth/login",
    {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        "X-Tenant-Id": tenantId,
        "X-CSRF-Token": csrfToken,
      },
      body: JSON.stringify({ username, password: "123456" }),
    },
    jar,
  );
  const token = response?.data?.access_token;
  if (!token) fail("登录响应缺少访问令牌");
  const current = await requestJson(
    apiBase,
    "/api/v1/auth/me",
    { headers: authHeaders(token, null, tenantId) },
    jar,
  );
  const userId = String(current?.data?.id ?? "");
  if (!/^\d+$/.test(userId) || userId === "0") fail("当前用户响应缺少有效用户标识");
  return { jar, token, userId, tenantId };
}

async function issueTicket(apiBase, token, locale, tenantId = "system") {
  const response = await requestJson(apiBase, "/api/v1/auth/ws-ticket", {
    method: "POST",
    headers: authHeaders(token, locale, tenantId),
  });
  const ticket = response?.data?.ticket;
  const expiresIn = response?.data?.expires_in;
  if (typeof ticket !== "string" || ticket.length < 32) {
    fail(`语言 ${locale} 的 WebSocket 票据无效`);
  }
  if (!Number.isInteger(expiresIn) || expiresIn < 1) {
    fail(`语言 ${locale} 的 WebSocket 票据缺少有效期`);
  }
  return { ticket, expiresIn };
}

function websocketUrl(apiBase, ticket) {
  const url = new URL("/api/v1/ws", apiBase);
  url.protocol = url.protocol === "https:" ? "wss:" : "ws:";
  url.searchParams.set("ticket", ticket);
  return url;
}

async function rawUpgradeStatus(apiBase, ticket, origin = null) {
  if (apiBase.protocol !== "http:" || apiBase.hostname !== "127.0.0.1") {
    fail("原始 WebSocket 握手仅允许访问本机 HTTP 验收实例");
  }
  const port = Number(apiBase.port);
  if (!Number.isInteger(port) || port < 1 || port > 65_535) {
    fail("原始 WebSocket 握手缺少有效本机端口");
  }
  const pathname = websocketUrl(apiBase, ticket).pathname
    + websocketUrl(apiBase, ticket).search;
  const key = randomBytes(16).toString("base64");
  const headers = [
    `GET ${pathname} HTTP/1.1`,
    `Host: 127.0.0.1:${port}`,
    "Connection: Upgrade",
    "Upgrade: websocket",
    "Sec-WebSocket-Version: 13",
    `Sec-WebSocket-Key: ${key}`,
  ];
  if (origin) headers.push(`Origin: ${origin}`);
  const requestText = `${headers.join("\r\n")}\r\n\r\n`;

  return new Promise((resolve, reject) => {
    const socket = connect({ host: "127.0.0.1", port });
    let responseText = "";
    const timeout = setTimeout(() => {
      socket.destroy();
      reject(new Error("等待原始 WebSocket 握手响应超时"));
    }, 5_000);
    socket.setEncoding("utf8");
    socket.on("connect", () => socket.write(requestText));
    socket.on("data", (chunk) => {
      responseText += chunk;
      if (!responseText.includes("\r\n\r\n")) return;
      clearTimeout(timeout);
      socket.destroy();
      const match = /^HTTP\/1\.1\s+(\d{3})\b/.exec(responseText);
      if (!match) {
        reject(new Error(`无法解析 WebSocket 握手响应：${responseText.slice(0, 200)}`));
        return;
      }
      resolve(Number(match[1]));
    });
    socket.on("error", (error) => {
      clearTimeout(timeout);
      reject(error);
    });
  });
}

function createSocketProbe(apiBase, ticket, expectedLocale, label) {
  const state = {
    label,
    expectedLocale,
    hello: null,
    target: null,
    targetMessageIds: new Set(),
    targetRawFrameCount: 0,
    pendingMessages: [],
    allMessageIds: [],
    acknowledgedIds: new Set(),
    acknowledgementRequestedIds: new Set(),
    acknowledgeTargetImmediately: false,
    error: null,
    closing: false,
    closed: false,
  };
  const acceptTargetMessage = (message) => {
    if (!state.target || message?.id !== state.target.id) return;
    if (message.title !== state.target.text || message.content !== state.target.text) {
      fail(`${label} 的本地化消息不正确：${JSON.stringify(message)}`);
    }
    state.targetRawFrameCount += 1;
    state.targetMessageIds.add(state.target.id);
    if (
      state.acknowledgeTargetImmediately
      && !state.acknowledgementRequestedIds.has(state.target.id)
    ) {
      state.acknowledgementRequestedIds.add(state.target.id);
      socket.send(JSON.stringify({ v: 1, type: "ack", ids: [state.target.id] }));
    }
  };
  const socket = new WebSocket(websocketUrl(apiBase, ticket));
  socket.addEventListener("message", (event) => {
    try {
      const frame = JSON.parse(String(event.data));
      if (frame?.v !== 1 || typeof frame?.type !== "string") {
        fail(`${label} 收到无效 WebSocket 帧`);
      }
      if (frame.type === "hello") {
        if (state.hello) fail(`${label} 重复收到 hello 帧`);
        if (frame.locale !== expectedLocale) {
          fail(`${label} hello 语言为 ${frame.locale}，预期为 ${expectedLocale}`);
        }
        state.hello = frame;
        return;
      }
      if (frame.type === "message") {
        state.allMessageIds.push(String(frame.message?.id ?? ""));
        if (state.target) acceptTargetMessage(frame.message);
        else state.pendingMessages.push(frame.message);
        return;
      }
      if (frame.type === "ack" && Array.isArray(frame.ids)) {
        for (const id of frame.ids) state.acknowledgedIds.add(String(id));
        return;
      }
    } catch (error) {
      state.error = error;
    }
  });
  socket.addEventListener("error", () => {
    if (!state.closing && !state.closed && !state.error) {
      state.error = new Error(`${label} WebSocket 连接错误`);
    }
  });
  socket.addEventListener("close", () => {
    state.closed = true;
    if (!state.closing && !state.error) state.error = new Error(`${label} 意外关闭`);
  });
  return { socket, state, acceptTargetMessage };
}

function armProbeTarget(probe, target, acknowledgeImmediately = false) {
  if (probe.state.target) fail(`${probe.state.label} 已设置目标消息`);
  probe.state.target = target;
  probe.state.acknowledgeTargetImmediately = acknowledgeImmediately;
  const pendingMessages = probe.state.pendingMessages;
  probe.state.pendingMessages = [];
  for (const pending of pendingMessages) probe.acceptTargetMessage(pending);
}

async function waitForHealthyProbes(probes) {
  const deadline = Date.now() + 15_000;
  while (Date.now() < deadline) {
    const error = probeTerminalError(probes);
    if (error) throw error;
    if (probes.every((probe) => probe.state.hello)) return;
    await sleep(50);
  }
  fail("等待全部 WebSocket hello 帧超时");
}

async function consumeTicketAndClose(apiBase, grant, locale, label) {
  const probe = createSocketProbe(apiBase, grant.ticket, locale, label);
  await waitForHealthyProbes([probe]);
  await closeProbes([probe]);
  return probe.state.hello;
}

async function assertTicketAndOriginGuards(apiBase, token) {
  const expired = await issueTicket(apiBase, token, "zh-CN");
  await sleep((expired.expiresIn + 1) * 1_000);
  const expiredStatus = await rawUpgradeStatus(apiBase, expired.ticket);
  if (expiredStatus !== 401) {
    fail(`过期 WebSocket 票据握手返回 ${expiredStatus}，预期为 401`);
  }

  const origin = await issueTicket(apiBase, token, "en-US");
  const wrongOriginStatus = await rawUpgradeStatus(
    apiBase,
    origin.ticket,
    "https://untrusted.invalid",
  );
  if (wrongOriginStatus !== 403) {
    fail(`错误 Origin 握手返回 ${wrongOriginStatus}，预期为 403`);
  }
  const hello = await consumeTicketAndClose(
    apiBase,
    origin,
    "en-US",
    "Origin 拒绝后的有效连接",
  );
  if (hello?.locale !== "en-US") {
    fail("错误 Origin 拒绝后，同一票据未能由合法连接消费");
  }
  const replayStatus = await rawUpgradeStatus(apiBase, origin.ticket);
  if (replayStatus !== 401) {
    fail(`已消费 WebSocket 票据重放返回 ${replayStatus}，预期为 401`);
  }
  return {
    expired_status: expiredStatus,
    wrong_origin_status: wrongOriginStatus,
    rejected_origin_preserved_ticket: true,
    replay_status: replayStatus,
  };
}

async function publishLiteralMessage(
  apiBase,
  token,
  userId,
  title,
  content,
  sourceType,
  sourceId,
) {
  const response = await requestJson(apiBase, "/api/v1/system/messages", {
    method: "POST",
    headers: {
      ...authHeaders(token, "zh-CN"),
      "Content-Type": "application/json",
    },
    body: JSON.stringify({
      topic: "runtime-acceptance",
      title,
      content,
      severity: "info",
      source_type: sourceType,
      source_id: sourceId,
      audiences: [{ kind: "user", target_id: userId }],
    }),
  });
  const messageId = String(response?.data?.message?.id ?? "");
  if (
    !/^\d+$/.test(messageId)
    || response?.data?.recipient_count !== 1
    || response?.data?.inserted !== true
  ) {
    fail(`发布验收消息失败：${JSON.stringify(response?.data)}`);
  }
  return messageId;
}

async function waitForSlowConsumerClose(apiBase, ticket) {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(websocketUrl(apiBase, ticket));
    const timeout = setTimeout(() => {
      socket.close();
      reject(new Error("等待慢消费者关闭超时"));
    }, 15_000);
    socket.addEventListener("close", (event) => {
      clearTimeout(timeout);
      if (event.code !== 1013) {
        reject(new Error(`慢消费者关闭码为 ${event.code}，预期为 1013`));
        return;
      }
      resolve({ code: event.code, reason: event.reason });
    });
    socket.addEventListener("error", () => {
      // 服务端关闭握手仍由 close 事件给出最终代码。
    });
  });
}

async function assertSlowConsumer(apiBase, token, userId, marker) {
  const backlogIds = [];
  const content = `慢消费者验收 ${marker} ${"载荷".repeat(4_000)}`;
  for (let index = 0; index < 16; index += 1) {
    backlogIds.push(await publishLiteralMessage(
      apiBase,
      token,
      userId,
      `慢消费者验收 ${index}`,
      content,
      "runtime_acceptance_0_7_slow_consumer",
      `${marker}-${index}`,
    ));
  }
  const grant = await issueTicket(apiBase, token, "zh-CN");
  const close = await waitForSlowConsumerClose(apiBase, grant.ticket);
  const persisted = await waitFor("慢消费者积压消息完整持久化", 5_000, async () => {
    const records = await inboxRecords(apiBase, token);
    const matched = records.filter((record) => backlogIds.includes(record?.id));
    return matched.length === backlogIds.length ? matched : null;
  });
  const marked = await requestJson(apiBase, "/api/v1/system/messages/read-all", {
    method: "PUT",
    headers: authHeaders(token, "zh-CN"),
  });
  if (!Number.isInteger(marked?.data) || marked.data < 0) {
    fail(`慢消费者积压清理响应无效：${JSON.stringify(marked)}`);
  }
  const readBack = await waitFor("慢消费者积压消息全部回读为已读", 5_000, async () => {
    const records = await inboxRecords(apiBase, token);
    const matched = records.filter(
      (record) => backlogIds.includes(record?.id) && record?.read_at && record?.acked_at,
    );
    return matched.length === backlogIds.length ? matched : null;
  });
  return {
    close_code: close.code,
    backlog_count: backlogIds.length,
    persisted_count: persisted.length,
    read_back_count: readBack.length,
    marked_read_count: marked.data,
  };
}

function metricValue(text, metric, labels = {}) {
  for (const rawLine of text.split(/\r?\n/)) {
    const line = rawLine.trim();
    if (!line.startsWith(metric)) continue;
    const separator = line.lastIndexOf(" ");
    if (separator < 1) continue;
    const head = line.slice(0, separator);
    const labelsMatch = Object.entries(labels).every(
      ([name, value]) => head.includes(`${name}="${value}"`),
    );
    if (!labelsMatch) continue;
    const value = Number(line.slice(separator + 1));
    if (Number.isFinite(value)) return value;
  }
  return 0;
}

async function metrics(apiBase) {
  const response = await request(apiBase, "/api/v1/monitor/metrics");
  if (!response.ok) fail(`指标端点返回 HTTP ${response.status}`);
  const text = await response.text();
  return {
    connections: metricValue(text, "ryframe_ws_connections"),
    replaySuccess: metricValue(text, "ryframe_message_replay_query_total", { result: "success" }),
    delivered: metricValue(text, "ryframe_message_delivery_total", { result: "delivered" }),
  };
}

async function alignReplayBaseline(apiBase, connectionCount, probes = []) {
  const terminalError = probes.length > 0 ? () => probeTerminalError(probes) : null;
  await waitFor("WebSocket 连接指标", 10_000, async () => {
    const current = await metrics(apiBase);
    return current.connections === connectionCount ? current : null;
  }, terminalError);

  let stable = await metrics(apiBase);
  let stableSince = Date.now();
  await waitFor("连接触发的补拉查询完成", 10_000, async () => {
    const current = await metrics(apiBase);
    if (current.replaySuccess !== stable.replaySuccess) {
      stable = current;
      stableSince = Date.now();
    }
    return Date.now() - stableSince >= 1_000 ? current : null;
  }, terminalError);

  const starting = await metrics(apiBase);
  await waitFor("下一次周期共享补拉查询", 25_000, async () => {
    const current = await metrics(apiBase);
    return current.replaySuccess > starting.replaySuccess ? current : null;
  }, terminalError);
  await sleep(250);
  return metrics(apiBase);
}

async function settledMetrics(apiBase, expectedConnections) {
  let stable = await metrics(apiBase);
  let stableSince = Date.now();
  return waitFor("实例指标稳定", 5_000, async () => {
    const current = await metrics(apiBase);
    if (
      current.connections !== expectedConnections
      || current.replaySuccess !== stable.replaySuccess
      || current.delivered !== stable.delivered
    ) {
      stable = current;
      stableSince = Date.now();
      return null;
    }
    return Date.now() - stableSince >= 500 ? current : null;
  });
}

function assertTargetProbesStable(probes, label) {
  const failedProbe = probes.find((probe) => probe.state.error);
  if (failedProbe) throw failedProbe.state.error;
  const invalidProbe = probes.find((probe) => (
    probe.state.targetMessageIds.size !== 1 || probe.state.targetRawFrameCount < 1
  ));
  if (invalidProbe) {
    fail(`${label}：${invalidProbe.state.label} 的逻辑目标消息数量不是 1`);
  }
  return probes.map((probe) => ({
    label: probe.state.label,
    locale: probe.state.expectedLocale,
    logical_message_count: probe.state.targetMessageIds.size,
    raw_frame_count: probe.state.targetRawFrameCount,
  }));
}

function assertProbeCountsUnchanged(probes, expectedCounts, label) {
  const currentCounts = assertTargetProbesStable(probes, label);
  if (
    currentCounts.length !== expectedCounts.length
    || currentCounts.some((current, index) => (
      current.label !== expectedCounts[index].label
      || current.logical_message_count !== expectedCounts[index].logical_message_count
      || current.raw_frame_count !== expectedCounts[index].raw_frame_count
    ))
  ) {
    fail(`${label}：ACK 后仍出现目标消息原始帧`);
  }
  return currentCounts;
}

async function settleSingleProbeDelivery(apiBase, baseline, probe, label) {
  let stableSignature = null;
  let stableSince = Date.now();
  return waitFor(label, 8_000, async () => {
    const current = await metrics(apiBase);
    const probeCounts = assertTargetProbesStable([probe], label);
    const rawFrameCount = probeCounts[0].raw_frame_count;
    const replayDelta = current.replaySuccess - baseline.replaySuccess;
    const deliveryDelta = current.delivered - baseline.delivered;
    if (replayDelta < 1 || deliveryDelta !== rawFrameCount) {
      stableSignature = null;
      stableSince = Date.now();
      return null;
    }
    const signature = `${current.replaySuccess}:${current.delivered}:${rawFrameCount}`;
    if (signature !== stableSignature) {
      stableSignature = signature;
      stableSince = Date.now();
      return null;
    }
    return Date.now() - stableSince >= 500
      ? { current, probeCounts, rawFrameCount, replayDelta, deliveryDelta }
      : null;
  }, () => probeTerminalError([probe]));
}

async function waitForReplayAdvance(
  instance,
  baseline,
  probes,
  expectedProbeCounts,
  label,
) {
  return waitFor(`${label} ${instance.name} 补拉推进`, 8_000, async () => {
    assertProbeCountsUnchanged(probes, expectedProbeCounts, label);
    const current = await metrics(instance.apiBase);
    if (current.connections !== instance.connectionCount) {
      fail(
        `${label} ${instance.name} 连接数异常：${current.connections}，预期 ${instance.connectionCount}`,
      );
    }
    if (current.delivered !== baseline.delivered) {
      fail(`${label} ${instance.name} 在去重稳定窗口内发生了额外投递`);
    }
    return current.replaySuccess > baseline.replaySuccess ? current : null;
  }, () => probeTerminalError(probes));
}

async function assertReplayDeduplicationWindow(instances, probes, expectedProbeCounts, label) {
  assertProbeCountsUnchanged(probes, expectedProbeCounts, label);
  const starting = await Promise.all(instances.map((instance) => metrics(instance.apiBase)));
  for (let index = 0; index < instances.length; index += 1) {
    if (starting[index].connections !== instances[index].connectionCount) {
      fail(`${label} ${instances[index].name} 未保持预期连接数`);
    }
  }

  const aligned = await Promise.all(instances.map((instance, index) => (
    waitForReplayAdvance(
      instance,
      starting[index],
      probes,
      expectedProbeCounts,
      `${label} 首次对齐`,
    )
  )));
  const completed = await Promise.all(instances.map((instance, index) => (
    waitForReplayAdvance(
      instance,
      aligned[index],
      probes,
      expectedProbeCounts,
      `${label} 完整周期`,
    )
  )));
  await sleep(250);
  const finalMetrics = await Promise.all(instances.map((instance) => metrics(instance.apiBase)));
  const probeCounts = assertProbeCountsUnchanged(
    probes,
    expectedProbeCounts,
    `${label} 最终检查`,
  );
  const instanceMetrics = {};
  for (let index = 0; index < instances.length; index += 1) {
    const instance = instances[index];
    const final = finalMetrics[index];
    if (
      final.connections !== instance.connectionCount
      || final.delivered !== starting[index].delivered
      || completed[index].replaySuccess <= aligned[index].replaySuccess
    ) {
      fail(`${label} ${instance.name} 未通过完整补拉周期去重检查`);
    }
    instanceMetrics[instance.name] = {
      replay_query_delta: final.replaySuccess - aligned[index].replaySuccess,
      total_replay_query_delta: final.replaySuccess - starting[index].replaySuccess,
      delivery_delta: final.delivered - starting[index].delivered,
      connection_count: final.connections,
    };
  }
  return {
    full_replay_cycle_observed: true,
    error_count: 0,
    probe_counts: probeCounts,
    instance_metrics: instanceMetrics,
  };
}

async function assertAcknowledgedMessageAbsentAcrossReplayCycles(
  apiBase,
  token,
  messageId,
) {
  const grant = await issueTicket(apiBase, token, "zh-CN");
  const probe = createSocketProbe(
    apiBase,
    grant.ticket,
    "zh-CN",
    "ACK 持久化验证连接",
  );
  await waitForHealthyProbes([probe]);
  const connected = await metrics(apiBase);
  if (connected.connections !== 1) fail("ACK 持久化验证连接数不是 1");
  const alignmentBaseline = await alignReplayBaseline(apiBase, 1, [probe]);
  const alignmentMessageCount = probe.state.allMessageIds.filter((id) => id === messageId).length;
  if (alignmentMessageCount !== 0 || alignmentBaseline.delivered !== connected.delivered) {
    fail("ACK 持久化对齐建连补拉时仍收到已确认消息");
  }
  const starting = alignmentBaseline;

  const waitForCycle = (baseline, label) => waitFor(label, 8_000, async () => {
    const postAckMessageCount = probe.state.allMessageIds.filter((id) => id === messageId).length;
    if (postAckMessageCount !== 0) {
      fail(`ACK 确认后的新连接仍收到消息 ${messageId}`);
    }
    const current = await metrics(apiBase);
    if (current.connections !== 1) fail("ACK 持久化验证连接意外断开");
    if (current.delivered !== starting.delivered) {
      fail("ACK 确认后的新连接出现额外投递");
    }
    return current.replaySuccess > baseline.replaySuccess ? current : null;
  }, () => probeTerminalError([probe]));

  const firstCompleted = await waitForCycle(starting, "ACK 持久化第一完整补拉周期");
  const secondCompleted = await waitForCycle(firstCompleted, "ACK 持久化第二完整补拉周期");
  await sleep(250);
  const final = await metrics(apiBase);
  const postAckMessageCount = probe.state.allMessageIds.filter((id) => id === messageId).length;
  if (
    postAckMessageCount !== 0
    || final.connections !== 1
    || final.delivered !== starting.delivered
    || secondCompleted.replaySuccess <= firstCompleted.replaySuccess
  ) {
    fail("ACK 持久化未通过新连接的两个完整补拉周期检查");
  }
  await closeProbes([probe]);
  const closed = await settledMetrics(apiBase, 0);
  return {
    verified_across_new_connection: true,
    full_replay_cycles: 2,
    post_ack_message_count: postAckMessageCount,
    alignment_replay_query_delta: starting.replaySuccess - connected.replaySuccess,
    replay_query_delta: final.replaySuccess - starting.replaySuccess,
    delivery_delta: final.delivered - starting.delivered,
    final_connections: closed.connections,
  };
}

async function assertOfflineReconnect(
  primaryApiBase,
  secondaryApiBase,
  token,
  userId,
  marker,
) {
  const initial = await Promise.all([
    settledMetrics(primaryApiBase, 0),
    settledMetrics(secondaryApiBase, 0),
  ]);
  const disconnectGrant = await issueTicket(primaryApiBase, token, "zh-CN");
  await consumeTicketAndClose(
    primaryApiBase,
    disconnectGrant,
    "zh-CN",
    "断线窗口初始连接",
  );
  const offlineBaselines = await Promise.all([
    settledMetrics(primaryApiBase, 0),
    settledMetrics(secondaryApiBase, 0),
  ]);
  const messageId = await publishLiteralMessage(
    primaryApiBase,
    token,
    userId,
    `断线窗口 ${marker}`,
    `断线窗口 ${marker}`,
    "runtime_acceptance_0_7_offline",
    marker,
  );
  await sleep(500);
  const whileOffline = await Promise.all([
    metrics(primaryApiBase),
    metrics(secondaryApiBase),
  ]);
  for (let index = 0; index < whileOffline.length; index += 1) {
    if (whileOffline[index].connections !== 0) fail("断线窗口发布时仍存在活动连接");
    if (whileOffline[index].delivered !== offlineBaselines[index].delivered) {
      fail("断线窗口发布期间出现了实时投递");
    }
  }

  const reconnectGrant = await issueTicket(secondaryApiBase, token, "zh-CN");
  const reconnectProbe = createSocketProbe(
    secondaryApiBase,
    reconnectGrant.ticket,
    "zh-CN",
    "断线后 API-B 重连",
  );
  await waitForHealthyProbes([reconnectProbe]);
  armProbeTarget(
    reconnectProbe,
    { id: messageId, text: `断线窗口 ${marker}` },
    true,
  );
  await waitFor(
    "断线后重连补拉",
    5_000,
    () => reconnectProbe.state.targetMessageIds.size === 1,
    () => probeTerminalError([reconnectProbe]),
  );
  await waitFor(
    "断线补拉确认",
    5_000,
    () => reconnectProbe.state.acknowledgedIds.has(messageId),
    () => probeTerminalError([reconnectProbe]),
  );
  const deliveryEvidence = await settleSingleProbeDelivery(
    secondaryApiBase,
    offlineBaselines[1],
    reconnectProbe,
    "断线重连投递指标与客户端帧稳定",
  );
  const reconnectedMetrics = deliveryEvidence.current;
  const primaryAfterReconnect = await metrics(primaryApiBase);
  const replayDelta = deliveryEvidence.replayDelta;
  const deliveryDelta = deliveryEvidence.deliveryDelta;
  if (
    primaryAfterReconnect.replaySuccess !== offlineBaselines[0].replaySuccess
    || primaryAfterReconnect.delivered !== offlineBaselines[0].delivered
  ) {
    fail("断线重连期间 API-A 不应执行补拉或投递");
  }
  const deliveryProbeCounts = deliveryEvidence.probeCounts;
  await closeProbes([reconnectProbe]);
  await settledMetrics(secondaryApiBase, 0);
  const ackPersistence = await assertAcknowledgedMessageAbsentAcrossReplayCycles(
    secondaryApiBase,
    token,
    messageId,
  );
  await requestJson(secondaryApiBase, `/api/v1/system/messages/${messageId}/read`, {
    method: "PUT",
    headers: authHeaders(token, "zh-CN"),
  });
  return {
    message_id: messageId,
    disconnected_instance: "api_a",
    reconnected_instance: "api_b",
    published_while_offline: true,
    raw_frame_count: deliveryEvidence.rawFrameCount,
    logical_message_count: reconnectProbe.state.targetMessageIds.size,
    replay_query_delta: replayDelta,
    delivery_delta: deliveryDelta,
    initial_connections: initial.map((item) => item.connections),
    final_secondary_connections: ackPersistence.final_connections,
    delivery_probe_counts: deliveryProbeCounts,
    ack_persistence: ackPersistence,
  };
}

async function assertInboxRendering(apiBase, token, messageId, locale, expectedText) {
  const response = await requestJson(
    apiBase,
    "/api/v1/system/messages?limit=100",
    { headers: authHeaders(token, locale) },
  );
  const records = response?.data?.records;
  if (!Array.isArray(records)) fail(`${locale} 收件箱响应缺少 records`);
  const matched = records.filter((record) => record?.id === messageId);
  if (matched.length !== 1) fail(`${locale} 收件箱中的消息 ${messageId} 数量为 ${matched.length}`);
  if (matched[0].title !== expectedText || matched[0].content !== expectedText) {
    fail(`${locale} 收件箱本地化不正确：${JSON.stringify(matched[0])}`);
  }
  return matched[0];
}

async function inboxRecords(apiBase, token, tenantId = "system") {
  const records = [];
  const seenCursors = new Set();
  let cursor = null;
  for (let page = 0; page < 100; page += 1) {
    const query = new URLSearchParams({ limit: "100" });
    if (cursor) query.set("cursor", cursor);
    const response = await requestJson(
      apiBase,
      `/api/v1/system/messages?${query}`,
      { headers: authHeaders(token, "zh-CN", tenantId) },
    );
    const pageRecords = response?.data?.records;
    if (!Array.isArray(pageRecords)) fail("收件箱响应缺少 records");
    records.push(...pageRecords);
    const nextCursor = response?.data?.next_cursor;
    if (nextCursor === null || nextCursor === undefined) return records;
    const next = String(nextCursor);
    if (!/^\d+$/.test(next) || seenCursors.has(next)) fail("收件箱游标无效或重复");
    seenCursors.add(next);
    cursor = next;
  }
  fail("收件箱分页超过安全上限");
}

async function inboxRecord(apiBase, token, messageId, tenantId = "system") {
  const records = await inboxRecords(apiBase, token, tenantId);
  const matched = records.filter((record) => record?.id === messageId);
  if (matched.length > 1) fail(`收件箱中的消息 ${messageId} 出现重复记录`);
  return matched[0] ?? null;
}

async function assertAckAndReadPersistence(primaryApiBase, secondaryApiBase, token, messageId) {
  const acknowledged = await waitFor("跨实例持久化确认状态", 5_000, async () => {
    const record = await inboxRecord(secondaryApiBase, token, messageId);
    return record?.acked_at && record?.read_at === null ? record : null;
  });
  await requestJson(secondaryApiBase, `/api/v1/system/messages/${messageId}/read`, {
    method: "PUT",
    headers: authHeaders(token, "zh-CN"),
  });
  const read = await waitFor("跨实例持久化已读状态", 5_000, async () => {
    const record = await inboxRecord(primaryApiBase, token, messageId);
    return record?.acked_at && record?.read_at ? record : null;
  });
  return {
    acked_at: acknowledged.acked_at,
    read_at: read.read_at,
    verified_across_instances: true,
  };
}

async function assertTenantIsolation(
  primaryApiBase,
  secondaryApiBase,
  systemToken,
  probes,
  fixture,
) {
  const messageId = String(fixture?.message_id ?? "");
  const tenantId = String(fixture?.tenant_id ?? "");
  const username = String(fixture?.username ?? "");
  const expectedText = String(fixture?.expected_text ?? "");
  if (!/^\d+$/.test(messageId) || !tenantId || !username || !expectedText) {
    fail(`租户隔离夹具无效：${JSON.stringify(fixture)}`);
  }
  await sleep(1_000);
  const systemConnectionCount = probes.filter(
    (probe) => probe.state.allMessageIds.includes(messageId),
  ).length;
  if (systemConnectionCount !== 0) {
    fail(`system 租户连接收到了隔离租户消息 ${messageId}`);
  }
  const systemInboxCount = await inboxRecord(primaryApiBase, systemToken, messageId) ? 1 : 0;
  if (systemInboxCount !== 0) {
    fail(`system 租户收件箱看到了隔离租户消息 ${messageId}`);
  }

  const isolated = await login(secondaryApiBase, tenantId, username);
  const isolatedRecord = await inboxRecord(
    secondaryApiBase,
    isolated.token,
    messageId,
    tenantId,
  );
  const isolatedInboxCount = isolatedRecord ? 1 : 0;
  if (
    isolatedRecord?.title !== expectedText
    || isolatedRecord?.content !== expectedText
  ) {
    fail(`隔离租户收件箱没有目标消息：${JSON.stringify(isolatedRecord)}`);
  }
  const grant = await issueTicket(
    secondaryApiBase,
    isolated.token,
    "zh-CN",
    tenantId,
  );
  const isolatedProbe = createSocketProbe(
    secondaryApiBase,
    grant.ticket,
    "zh-CN",
    "隔离租户连接",
  );
  await waitForHealthyProbes([isolatedProbe]);
  armProbeTarget(isolatedProbe, { id: messageId, text: expectedText }, true);
  await waitFor(
    "隔离租户连接收到自身消息",
    5_000,
    () => isolatedProbe.state.targetMessageIds.size === 1,
    () => probeTerminalError([isolatedProbe]),
  );
  await waitFor(
    "隔离租户消息确认",
    5_000,
    () => isolatedProbe.state.acknowledgedIds.has(messageId),
    () => probeTerminalError([isolatedProbe]),
  );
  await closeProbes([isolatedProbe]);
  return {
    tenant_id: tenantId,
    message_id: messageId,
    system_inbox_count: systemInboxCount,
    system_connection_count: systemConnectionCount,
    isolated_inbox_count: isolatedInboxCount,
    isolated_logical_message_count: isolatedProbe.state.targetMessageIds.size,
    isolated_raw_frame_count: isolatedProbe.state.targetRawFrameCount,
  };
}

async function publishRetentionCandidate(apiBase, token, userId, marker) {
  const messageId = await publishLiteralMessage(
    apiBase,
    token,
    userId,
    `90 天清理验收 ${marker}`,
    `90 天清理验收 ${marker}`,
    "runtime_acceptance_0_7_retention",
    marker,
  );
  const record = await inboxRecord(apiBase, token, messageId);
  if (!record) fail("90 天清理候选消息未进入持久化收件箱");
  const publishedAt = Date.parse(record.published_at);
  const expiresAt = Date.parse(record.expires_at);
  const expectedRetentionSeconds = 90 * 24 * 60 * 60;
  const retentionSeconds = Math.round((expiresAt - publishedAt) / 1_000);
  if (
    !Number.isFinite(publishedAt)
    || !Number.isFinite(expiresAt)
    || Math.abs(retentionSeconds - expectedRetentionSeconds) > 5
  ) {
    fail(`默认消息保留期限不是 90 天：${JSON.stringify(record)}`);
  }

  const overLimitResponse = await request(apiBase, "/api/v1/system/messages", {
    method: "POST",
    headers: {
      ...authHeaders(token, "zh-CN"),
      "Content-Type": "application/json",
    },
    body: JSON.stringify({
      topic: "runtime-acceptance",
      title: `超过保留上限 ${marker}`,
      content: `超过保留上限 ${marker}`,
      severity: "info",
      source_type: "runtime_acceptance_0_7_retention_over_limit",
      source_id: `${marker}-over-limit`,
      audiences: [{ kind: "user", target_id: userId }],
      expires_at: new Date(Date.now() + 91 * 24 * 60 * 60 * 1_000).toISOString(),
    }),
  });
  const overLimitText = await overLimitResponse.text();
  let overLimitBody = null;
  try {
    overLimitBody = JSON.parse(overLimitText);
  } catch {
    fail(`超过 90 天的发布响应不是有效 JSON：${overLimitText.slice(0, 300)}`);
  }
  if (overLimitResponse.status !== 400 || overLimitBody?.error_key !== "validation") {
    fail(`超过 90 天的消息未被拒绝：HTTP ${overLimitResponse.status} ${overLimitText}`);
  }
  return {
    message_id: messageId,
    default_retention_seconds: retentionSeconds,
    over_limit_status: overLimitResponse.status,
    over_limit_error_key: overLimitBody.error_key,
  };
}

async function assertRetentionCleanup(
  primaryApiBase,
  secondaryApiBase,
  token,
  messageId,
  evidence,
) {
  if (
    evidence?.status !== "passed"
    || String(evidence?.message_id ?? "") !== messageId
    || evidence?.retention_days !== 90
    || Math.abs(evidence?.default_retention_seconds - 90 * 24 * 60 * 60) > 5
    || evidence?.over_limit_status !== 400
    || evidence?.over_limit_error_key !== "validation"
    || evidence?.aged_days < 90
    || evidence?.message_rows !== 0
    || evidence?.audience_rows !== 0
    || evidence?.recipient_rows !== 0
    || evidence?.job_status !== "succeeded"
    || evidence?.job_attempts < 1
  ) {
    fail(`90 天清理 Worker 证据不足：${JSON.stringify(evidence)}`);
  }
  if (
    await inboxRecord(primaryApiBase, token, messageId)
    || await inboxRecord(secondaryApiBase, token, messageId)
  ) {
    fail(`90 天清理后的消息 ${messageId} 仍可从收件箱读取`);
  }
  return evidence;
}

async function closeProbes(probes) {
  for (const probe of probes) {
    probe.state.closing = true;
    probe.socket.close(1000, "验收完成");
  }
  await waitFor("WebSocket 连接关闭", 5_000, () => probes.every((probe) => probe.state.closed));
}

async function main() {
  const { apiBase, secondaryApiBase, controlDirectory } = parseArguments(process.argv.slice(2));
  if (typeof WebSocket !== "function") {
    fail("当前 Node.js 不提供内置 WebSocket，请使用 Node.js 22 或更高版本");
  }
  const readyPath = controlPath(controlDirectory, "client-ready.json");
  const tenantFixturePath = controlPath(controlDirectory, "tenant-fixture.json");
  const tenantResultPath = controlPath(controlDirectory, "tenant-result.json");
  const redisFaultFixturePath = controlPath(controlDirectory, "redis-fault-fixture.json");
  const deliveredPath = controlPath(controlDirectory, "client-delivered.json");
  const redisRestoredPath = controlPath(controlDirectory, "redis-restored.signal");
  const cleanupReadyPath = controlPath(controlDirectory, "cleanup-ready.json");
  const cleanupResultPath = controlPath(controlDirectory, "cleanup-result.json");
  const resultPath = controlPath(controlDirectory, "client-result.json");
  await assertPreflightFilesystem(controlDirectory, [
    readyPath,
    tenantResultPath,
    deliveredPath,
    cleanupReadyPath,
    resultPath,
  ]);
  const marker = `v07-${randomUUID()}`;
  const expectedZh = "欢迎 redis-fault-proof";
  const expectedEn = "Welcome redis-fault-proof";
  const primaryConnectionSpecifications = [
    { locale: "zh-CN", text: expectedZh, label: "中文连接一" },
    { locale: "en-US", text: expectedEn, label: "英文连接" },
    { locale: "zh-CN", text: expectedZh, label: "中文连接二" },
  ];
  const secondaryConnectionSpecification = {
    locale: "en-US",
    text: expectedEn,
    label: "API-B 英文连接",
  };
  const probes = [];
  let secondaryProbe = null;

  try {
    const { token, userId } = await login(apiBase);
    const ticketGuards = await assertTicketAndOriginGuards(apiBase, token);
    const slowConsumer = await assertSlowConsumer(apiBase, token, userId, marker);
    const offlineReconnect = await assertOfflineReconnect(
      apiBase,
      secondaryApiBase,
      token,
      userId,
      `${marker}-offline`,
    );
    const grants = await Promise.all(
      primaryConnectionSpecifications.map((item) => issueTicket(apiBase, token, item.locale)),
    );
    for (let index = 0; index < grants.length; index += 1) {
      const specification = primaryConnectionSpecifications[index];
      probes.push(createSocketProbe(
        apiBase,
        grants[index].ticket,
        specification.locale,
        specification.label,
      ));
    }
    const secondaryGrant = await issueTicket(
      secondaryApiBase,
      token,
      secondaryConnectionSpecification.locale,
    );
    secondaryProbe = createSocketProbe(
      secondaryApiBase,
      secondaryGrant.ticket,
      secondaryConnectionSpecification.locale,
      secondaryConnectionSpecification.label,
    );
    const allProbes = [...probes, secondaryProbe];
    await waitForHealthyProbes(allProbes);
    await writeJsonAtomically(readyPath, {
      status: "ready",
      tenant_id: "system",
      user_id: userId,
      primary_connection_count: probes.length,
      secondary_connection_count: 1,
      total_connection_count: allProbes.length,
      primary_locales: primaryConnectionSpecifications.map((item) => item.locale),
      secondary_locale: secondaryConnectionSpecification.locale,
      ticket_guards: ticketGuards,
      slow_consumer: slowConsumer,
      offline_reconnect: offlineReconnect,
    });

    await waitFor(
      "租户隔离夹具",
      30_000,
      () => fileExists(tenantFixturePath),
      () => probeTerminalError(allProbes),
    );
    const tenantFixture = await readJson(tenantFixturePath);
    const tenantIsolation = await assertTenantIsolation(
      apiBase,
      secondaryApiBase,
      token,
      allProbes,
      tenantFixture,
    );
    const [primaryBaseline, secondaryBaseline] = await Promise.all([
      alignReplayBaseline(apiBase, probes.length, probes),
      alignReplayBaseline(secondaryApiBase, 1, [secondaryProbe]),
    ]);
    const baselines = { api_a: primaryBaseline, api_b: secondaryBaseline };
    await writeJsonAtomically(tenantResultPath, {
      status: "passed",
      ...tenantIsolation,
      baselines,
    });

    await waitFor(
      "Redis 故障补拉夹具",
      30_000,
      () => fileExists(redisFaultFixturePath),
      () => probeTerminalError(allProbes),
    );
    const redisFaultFixture = await readJson(redisFaultFixturePath);
    const messageId = String(redisFaultFixture?.message_id ?? "");
    if (
      redisFaultFixture?.status !== "ready"
      || messageId !== "900000000000000105"
      || redisFaultFixture?.source_type !== "runtime_acceptance_0_7_redis_fault"
    ) {
      fail(`Redis 故障补拉夹具无效：${JSON.stringify(redisFaultFixture)}`);
    }
    for (let index = 0; index < probes.length; index += 1) {
      armProbeTarget(
        probes[index],
        { id: messageId, text: primaryConnectionSpecifications[index].text },
      );
    }
    armProbeTarget(
      secondaryProbe,
      { id: messageId, text: secondaryConnectionSpecification.text },
    );

    await waitFor(
      "两实例全部连接收到同一逻辑消息",
      30_000,
      () => allProbes.every((probe) => probe.state.targetMessageIds.size === 1),
      () => probeTerminalError(allProbes),
    );
    probes[0].state.acknowledgementRequestedIds.add(messageId);
    probes[0].socket.send(JSON.stringify({ v: 1, type: "ack", ids: [messageId] }));
    await waitFor(
      "WebSocket 确认响应",
      5_000,
      () => probes[0].state.acknowledgedIds.has(messageId),
      () => probeTerminalError(allProbes),
    );
    let stableDeliverySignature = null;
    let stableDeliverySince = Date.now();
    const deliveryEvidence = await waitFor("双实例消息投递指标与客户端帧稳定", 10_000, async () => {
      const [apiA, apiB] = await Promise.all([
        metrics(apiBase),
        metrics(secondaryApiBase),
      ]);
      const probeCounts = assertTargetProbesStable(allProbes, "双实例投递稳定检查");
      const primaryRawFrames = probeCounts
        .slice(0, probes.length)
        .reduce((total, probe) => total + probe.raw_frame_count, 0);
      const secondaryRawFrames = probeCounts[probes.length].raw_frame_count;
      const primaryDelta = apiA.delivered - primaryBaseline.delivered;
      const secondaryDelta = apiB.delivered - secondaryBaseline.delivered;
      if (
        apiA.replaySuccess - primaryBaseline.replaySuccess < 1
        || apiB.replaySuccess - secondaryBaseline.replaySuccess < 1
        || primaryDelta !== primaryRawFrames
        || secondaryDelta !== secondaryRawFrames
      ) {
        stableDeliverySignature = null;
        stableDeliverySince = Date.now();
        return null;
      }
      const signature = JSON.stringify([
        apiA.replaySuccess,
        apiA.delivered,
        apiB.replaySuccess,
        apiB.delivered,
        ...probeCounts.map((probe) => probe.raw_frame_count),
      ]);
      if (signature !== stableDeliverySignature) {
        stableDeliverySignature = signature;
        stableDeliverySince = Date.now();
        return null;
      }
      return Date.now() - stableDeliverySince >= 750
        ? {
            finalMetrics: { api_a: apiA, api_b: apiB },
            probeCounts,
            primaryRawFrames,
            secondaryRawFrames,
          }
        : null;
    }, () => probeTerminalError(allProbes));
    const finalMetrics = deliveryEvidence.finalMetrics;
    const deliveryProbeCounts = deliveryEvidence.probeCounts;
    const primaryReplayQueryDelta = finalMetrics.api_a.replaySuccess - primaryBaseline.replaySuccess;
    const primaryDeliveryDelta = finalMetrics.api_a.delivered - primaryBaseline.delivered;
    const secondaryReplayQueryDelta = finalMetrics.api_b.replaySuccess
      - secondaryBaseline.replaySuccess;
    const secondaryDeliveryDelta = finalMetrics.api_b.delivered - secondaryBaseline.delivered;
    const primaryRawFrameCount = deliveryEvidence.primaryRawFrames;
    const secondaryRawFrameCount = deliveryEvidence.secondaryRawFrames;
    if (primaryReplayQueryDelta < 1 || secondaryReplayQueryDelta < 1) {
      fail(
        `双实例共享补拉查询未推进：API-A ${primaryReplayQueryDelta}，API-B ${secondaryReplayQueryDelta}`,
      );
    }
    if (
      primaryDeliveryDelta !== primaryRawFrameCount
      || secondaryDeliveryDelta !== secondaryRawFrameCount
    ) {
      fail(`双实例投递增量不精确：API-A ${primaryDeliveryDelta}，API-B ${secondaryDeliveryDelta}`);
    }

    const delivered = {
      status: "delivered",
      tenant_id: "system",
      user_id: userId,
      message_id: messageId,
      fixture_source: "mysql",
      published_while_redis_unavailable: true,
      primary_connection_count: probes.length,
      secondary_connection_count: 1,
      total_connection_count: allProbes.length,
      primary_raw_frame_count: primaryRawFrameCount,
      secondary_raw_frame_count: secondaryRawFrameCount,
      total_raw_frame_count: primaryRawFrameCount + secondaryRawFrameCount,
      per_connection_counts: deliveryProbeCounts,
      instance_metrics: {
        api_a: {
          replay_query_delta: primaryReplayQueryDelta,
          delivery_delta: primaryDeliveryDelta,
        },
        api_b: {
          replay_query_delta: secondaryReplayQueryDelta,
          delivery_delta: secondaryDeliveryDelta,
        },
      },
      websocket_ack_received: probes[0].state.acknowledgedIds.has(messageId),
      ticket_guards: ticketGuards,
      slow_consumer: slowConsumer,
      offline_reconnect: offlineReconnect,
      tenant_isolation: tenantIsolation,
      baselines,
      final_metrics: finalMetrics,
    };
    await writeJsonAtomically(deliveredPath, delivered);

    await waitFor(
      "Redis 恢复信号",
      30_000,
      () => fileExists(redisRestoredPath),
      () => probeTerminalError(allProbes),
    );
    await sleep(1_000);
    const recoveryProbeCounts = assertProbeCountsUnchanged(
      allProbes,
      deliveryProbeCounts,
      "Redis 恢复后检查",
    );
    const [recoveryApiA, recoveryApiB] = await Promise.all([
      metrics(apiBase),
      metrics(secondaryApiBase),
    ]);
    if (
      recoveryApiA.delivered !== finalMetrics.api_a.delivered
      || recoveryApiB.delivered !== finalMetrics.api_b.delivered
    ) {
      fail("Redis 恢复后出现额外目标消息投递");
    }
    const redisRecoveryStability = {
      raw_frame_counts_unchanged: true,
      api_a_delivery_delta: recoveryApiA.delivered - finalMetrics.api_a.delivered,
      api_b_delivery_delta: recoveryApiB.delivered - finalMetrics.api_b.delivered,
      probe_counts: recoveryProbeCounts,
    };
    await assertInboxRendering(apiBase, token, messageId, "zh-CN", expectedZh);
    await assertInboxRendering(apiBase, token, messageId, "en-US", expectedEn);
    const persistedState = await assertAckAndReadPersistence(
      apiBase,
      secondaryApiBase,
      token,
      messageId,
    );
    const deduplicationStability = await assertReplayDeduplicationWindow(
      [
        { name: "api_a", apiBase, connectionCount: probes.length },
        { name: "api_b", apiBase: secondaryApiBase, connectionCount: 1 },
      ],
      allProbes,
      deliveryProbeCounts,
      "双实例去重稳定窗口",
    );
    const retentionPolicy = await publishRetentionCandidate(
      apiBase,
      token,
      userId,
      `${marker}-retention`,
    );
    const retentionMessageId = retentionPolicy.message_id;
    await writeJsonAtomically(cleanupReadyPath, {
      status: "ready",
      tenant_id: "system",
      message_id: retentionMessageId,
      source_type: "runtime_acceptance_0_7_retention",
      default_retention_seconds: retentionPolicy.default_retention_seconds,
      over_limit_status: retentionPolicy.over_limit_status,
      over_limit_error_key: retentionPolicy.over_limit_error_key,
    });
    await waitFor(
      "90 天清理 Worker 证据",
      45_000,
      () => fileExists(cleanupResultPath),
      () => probeTerminalError(allProbes),
    );
    const retentionCleanup = await assertRetentionCleanup(
      apiBase,
      secondaryApiBase,
      token,
      retentionMessageId,
      await readJson(cleanupResultPath),
    );
    const finalProbeCounts = assertProbeCountsUnchanged(
      allProbes,
      deliveryProbeCounts,
      "关闭前统一检查",
    );
    await closeProbes(allProbes);
    await writeJsonAtomically(resultPath, {
      ...delivered,
      status: "passed",
      persisted_state: persistedState,
      redis_recovery_stability: redisRecoveryStability,
      deduplication_stability: {
        ...deduplicationStability,
        final_probe_counts: finalProbeCounts,
      },
      retention_cleanup: retentionCleanup,
    });
    console.log("消息中心安全票据、租户隔离、持久状态、慢消费者、保留清理与故障恢复验收通过");
  } catch (error) {
    const activeProbes = secondaryProbe ? [...probes, secondaryProbe] : probes;
    for (const probe of activeProbes) {
      try {
        probe.state.closing = true;
        probe.socket.close();
      } catch {
      }
    }
    await writeJsonAtomically(resultPath, {
      status: "failed",
      error: error instanceof Error ? error.message : String(error),
    }).catch(() => {});
    throw error;
  }
}


main().catch(async (error) => {
  console.error(`消息中心运行验收失败：${error.message}`);
  process.exitCode = 1;
});
