const utf8EnvironmentNames = new Set(['PYTHONUTF8', 'PYTHONIOENCODING'])

function isUtf8Environment(name) {
  return utf8EnvironmentNames.has(name.toUpperCase())
}

export function pythonInvocation({
  script,
  arguments: scriptArguments = [],
  environment = process.env,
  noBytecode = false,
}) {
  const argv = [...(noBytecode ? ['-B'] : []), '-X', 'utf8', script, ...scriptArguments]
  const env = Object.fromEntries(
    Object.entries(environment).filter(([name]) => !isUtf8Environment(name)),
  )
  env.PYTHONUTF8 = '1'
  env.PYTHONIOENCODING = 'utf-8'
  return { argv, env }
}
