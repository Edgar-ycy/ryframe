const test = require("node:test");
const assert = require("node:assert/strict");

const { parseUploadedFile } = require("./smoke-test.js");

test("上传响应严格接受当前平铺契约并返回同源下载地址", () => {
  const response = {
    code: 200,
    data: [{
      file_id: "42",
      file_name: "smoke.txt",
      file_path: "2026/08/smoke.txt",
      file_url: "/api/v1/common/file/download?bucket=uploads&path=2026%2F08%2Fsmoke%2Etxt",
    }],
  };

  assert.deepEqual(
    parseUploadedFile(response, "smoke.txt", "http://127.0.0.1:28080"),
    {
      file: response.data[0],
      downloadUrl: "http://127.0.0.1:28080/api/v1/common/file/download?bucket=uploads&path=2026%2F08%2Fsmoke%2Etxt",
    },
  );
});

test("上传响应不接受旧 file_info 嵌套结构", () => {
  const legacyResponse = {
    code: 200,
    data: [{ file_info: { file_path: "legacy/smoke.txt" } }],
  };

  assert.throws(
    () => parseUploadedFile(legacyResponse, "smoke.txt"),
    /fields do not match the current contract/,
  );
});

test("上传响应拒绝非数组 data", () => {
  assert.throws(
    () => parseUploadedFile({ code: 200, data: { file_path: "invalid.txt" } }, "smoke.txt"),
    /must contain exactly one file/,
  );
});

test("上传响应拒绝跨源或参数不一致的下载地址", () => {
  const response = {
    code: 200,
    data: [{
      file_id: "42",
      file_name: "smoke.txt",
      file_path: "2026/08/smoke.txt",
      file_url: "https://example.test/api/v1/common/file/download?bucket=uploads&path=other.txt",
    }],
  };

  assert.throws(
    () => parseUploadedFile(response, "smoke.txt", "http://127.0.0.1:28080"),
    /invalid download URL/,
  );
});
