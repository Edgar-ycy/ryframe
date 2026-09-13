import test from 'node:test'
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { pythonInvocation } from '../python_process.mjs'

test('Python 调用计划固定 UTF-8 且按 Windows 语义清理环境名', () => {
  const source = {
    PATH: 'fixture-path',
    PyThOnUtF8: '0',
    pythonioencoding: 'ascii:strict',
    RYFRAME_FIXTURE: 'kept',
  }
  const invocation = pythonInvocation({
    script: 'fixture.py',
    arguments: ['--value'],
    environment: source,
  })
  assert.deepEqual(invocation.argv, ['-X', 'utf8', 'fixture.py', '--value'])
  assert.deepEqual(invocation.env, {
    PATH: 'fixture-path',
    RYFRAME_FIXTURE: 'kept',
    PYTHONUTF8: '1',
    PYTHONIOENCODING: 'utf-8',
  })
  assert.equal(source.PyThOnUtF8, '0')
  assert.deepEqual(pythonInvocation({ script: 'fixture.py', noBytecode: true }).argv, [
    '-B',
    '-X',
    'utf8',
    'fixture.py',
  ])
})

test('真实 Python 子进程只看到固定编码环境并保留禁止字节码参数', () => {
  const executable = process.env.RYFRAME_PYTHON?.trim() || 'python'
  const script = [
    'import json, os, sys',
    "names = sorted(name for name in os.environ if name.upper() in {'PYTHONUTF8',",
    "    'PYTHONIOENCODING'})",
    "print(json.dumps({'utf8': sys.flags.utf8_mode,",
    "    'bytecode': sys.dont_write_bytecode, 'names': names,",
    "    'mode': os.environ.get('PYTHONUTF8'),",
    "    'encoding': os.environ.get('PYTHONIOENCODING')}))",
  ].join('\n')
  const invocation = pythonInvocation({
    script: '-c',
    arguments: [script],
    environment: {
      ...process.env,
      PyThOnUtF8: '0',
      pYtHoNiOeNcOdInG: 'ascii:strict',
    },
    noBytecode: true,
  })
  const result = spawnSync(executable, invocation.argv, {
    encoding: 'utf8',
    env: invocation.env,
    windowsHide: true,
  })
  assert.equal(result.status, 0, result.stderr)
  assert.deepEqual(JSON.parse(result.stdout), {
    utf8: 1,
    bytecode: true,
    names: ['PYTHONIOENCODING', 'PYTHONUTF8'],
    mode: '1',
    encoding: 'utf-8',
  })
})
