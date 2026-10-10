#!/usr/bin/env node

const fs = require('fs')
const path = require('path')
const { spawn } = require('child_process')

const projectRoot = path.resolve(__dirname, '..')
const pythonDistDir = path.join(projectRoot, 'python-dist')
const pyprojectPath = path.join(projectRoot, 'pyproject.toml')
const pythonVersionFile = path.join(projectRoot, '.python-version')

function log(message) {
  console.log(`[package] ${message}`)
}

function spawnOptions(stdio = 'inherit') {
  return {
    cwd: projectRoot,
    env: {
      ...process.env,
      UV_HTTP_TIMEOUT: process.env.UV_HTTP_TIMEOUT || '120'
    },
    shell: process.platform === 'win32',
    stdio
  }
}

function commandExists(command) {
  return new Promise((resolve) => {
    const child = spawn(command, ['--version'], spawnOptions('ignore'))
    child.on('error', () => resolve(false))
    child.on('close', (code) => resolve(code === 0))
  })
}

function run(command, args) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, spawnOptions('inherit'))

    child.on('error', reject)
    child.on('close', (code) => {
      if (code === 0) {
        resolve()
        return
      }

      reject(new Error(`${command} ${args.join(' ')} exited with code ${code}`))
    })
  })
}

function capture(command, args) {
  return new Promise((resolve, reject) => {
    let stdout = ''
    let stderr = ''
    const child = spawn(command, args, spawnOptions(['ignore', 'pipe', 'pipe']))

    child.stdout.on('data', (chunk) => {
      stdout += chunk.toString()
    })

    child.stderr.on('data', (chunk) => {
      stderr += chunk.toString()
    })

    child.on('error', reject)
    child.on('close', (code) => {
      if (code === 0) {
        resolve(stdout.trim())
        return
      }

      reject(new Error((stderr || stdout).trim() || `${command} exited with code ${code}`))
    })
  })
}

function compareVersions(left, right) {
  const leftParts = left.split('.').map(Number)
  const rightParts = right.split('.').map(Number)
  const maxLength = Math.max(leftParts.length, rightParts.length)

  for (let index = 0; index < maxLength; index += 1) {
    const leftValue = leftParts[index] || 0
    const rightValue = rightParts[index] || 0

    if (leftValue !== rightValue) {
      return leftValue - rightValue
    }
  }

  return 0
}

function normalizeRequestedVersion(rawVersion) {
  return rawVersion ? rawVersion.trim().replace(/^python\s+/i, '') : null
}

function getRequestedPythonVersion() {
  const cliVersionArg = process.argv.slice(2).find((arg) => arg.startsWith('--python-version='))
  if (cliVersionArg) {
    return normalizeRequestedVersion(cliVersionArg.split('=')[1])
  }

  if (fs.existsSync(pythonVersionFile)) {
    return normalizeRequestedVersion(fs.readFileSync(pythonVersionFile, 'utf8'))
  }

  return '3.12'
}

function getTargetFlag() {
  const args = process.argv.slice(2)

  if (args.includes('--win')) {
    return '--win'
  }

  if (args.includes('--mac')) {
    return '--mac'
  }

  if (args.includes('--linux')) {
    return '--linux'
  }

  if (process.platform === 'win32') {
    return '--win'
  }

  if (process.platform === 'darwin') {
    return '--mac'
  }

  return '--linux'
}

/**
 * 取 electron-builder 的目标参数。
 *
 * ``--nsis`` / ``--portable`` 用来单独重跑某个目标：整套 --win 会先产出
 * win-unpacked 再打安装包，单独重跑安装包时无需重复解包，便于定位安装器阶段的失败。
 */
function getBuilderTargetArgs() {
  const args = process.argv.slice(2)
  const targetFlag = getTargetFlag()

  if (args.includes('--nsis')) {
    return [targetFlag, 'nsis']
  }

  if (args.includes('--portable')) {
    return [targetFlag, 'portable']
  }

  return [targetFlag]
}

function getManagedInstallations(uvPythonDir) {
  const platformToken = process.platform === 'win32'
    ? 'windows'
    : process.platform === 'darwin'
      ? 'darwin'
      : 'linux'

  return fs.readdirSync(uvPythonDir, { withFileTypes: true })
    .filter((entry) => entry.isDirectory() && entry.name.startsWith('cpython-') && entry.name.includes(platformToken))
    .map((entry) => {
      const match = entry.name.match(/^cpython-([0-9]+\.[0-9]+\.[0-9]+)-/)
      return match ? {
        version: match[1],
        dir: path.join(uvPythonDir, entry.name)
      } : null
    })
    .filter(Boolean)
    .sort((left, right) => compareVersions(right.version, left.version))
}

function versionMatches(installedVersion, requestedVersion) {
  if (!requestedVersion) {
    return true
  }

  if (/^\d+\.\d+$/.test(requestedVersion)) {
    return installedVersion.startsWith(`${requestedVersion}.`)
  }

  return installedVersion === requestedVersion
}

async function resolveManagedPythonDir(requestedVersion) {
  const uvPythonDir = await capture('uv', ['python', 'dir'])

  const selectInstallation = () => {
    const installations = getManagedInstallations(uvPythonDir)
    return installations.find((installation) => versionMatches(installation.version, requestedVersion)) || null
  }

  let installation = selectInstallation()
  if (installation) {
    return installation
  }

  if (!requestedVersion) {
    throw new Error('No uv-managed Python installation is available')
  }

  log(`Installing uv-managed Python ${requestedVersion}`)
  await run('uv', ['python', 'install', requestedVersion])

  installation = selectInstallation()
  if (!installation) {
    throw new Error(`Unable to locate uv-managed Python ${requestedVersion} after installation`)
  }

  return installation
}

function getBundledPythonExecutable() {
  return process.platform === 'win32'
    ? path.join(pythonDistDir, 'python.exe')
    : path.join(pythonDistDir, 'bin', 'python3')
}

async function prepareBundledPython() {
  const requestedVersion = getRequestedPythonVersion()
  const installation = await resolveManagedPythonDir(requestedVersion)

  log(`Using uv-managed Python ${installation.version}`)

  fs.rmSync(pythonDistDir, { recursive: true, force: true })
  fs.cpSync(installation.dir, pythonDistDir, {
    force: true,
    recursive: true
  })

  const externallyManagedFile = path.join(pythonDistDir, 'Lib', 'EXTERNALLY-MANAGED')
  if (fs.existsSync(externallyManagedFile)) {
    fs.rmSync(externallyManagedFile, { force: true })
  }

  const pythonExecutable = getBundledPythonExecutable()
  if (!fs.existsSync(pyprojectPath)) {
    throw new Error('pyproject.toml is required for packaging')
  }

  log(`Installing backend dependencies from ${path.basename(pyprojectPath)}`)
  await run('uv', [
    'pip',
    'install',
    '--python',
    pythonExecutable,
    '--link-mode',
    'copy',
    '--compile-bytecode',
    '-r',
    pyprojectPath
  ])
}

/**
 * 打包后自检：确认产物里「该有的有、不该有的没有」。
 *
 * 这条检查是硬闸门——一旦 data/（含 .encryption_key）或 models/ 被打进安装包，
 * 就等于把解密密钥和密文一起分发（API Key 明文泄漏），必须在打包阶段直接失败。
 */
function verifyPackagedResources() {
  const unpackedDir = path.join(projectRoot, 'release', 'win-unpacked')
  if (!fs.existsSync(unpackedDir)) {
    throw new Error(`打包自检失败：未找到 ${unpackedDir}（NSIS/portable 产物应包含 win-unpacked）`)
  }

  const resourcesDir = path.join(unpackedDir, 'resources')
  const forbidden = ['data', 'models']
  const problems = []

  for (const name of forbidden) {
    const target = path.join(resourcesDir, name)
    if (fs.existsSync(target)) {
      problems.push(`resources/${name} 不应存在（安装包不得携带用户数据/模型）`)
    }
  }

  const appDir = path.join(resourcesDir, 'app')
  if (!fs.existsSync(path.join(appDir, 'main.py'))) {
    problems.push('resources/app/main.py 缺失：后端源码未随包分发')
  }

  const promptFiles = fs.existsSync(path.join(resourcesDir, 'prompts'))
    ? fs.readdirSync(path.join(resourcesDir, 'prompts'), { recursive: true })
      .filter((name) => String(name).toLowerCase().endsWith('.txt'))
    : []
  if (promptFiles.length === 0) {
    problems.push('resources/prompts/ 下没有任何 .txt 提示词预设：打包版会出现零预设')
  }

  const pythonExe = path.join(resourcesDir, 'python', 'python.exe')
  const pythonExePosix = path.join(resourcesDir, 'python', 'bin', 'python3')
  if (!fs.existsSync(pythonExe) && !fs.existsSync(pythonExePosix)) {
    problems.push('resources/python 下没有 Python 解释器：后端无法启动')
  }

  if (problems.length > 0) {
    throw new Error(`打包自检失败：\n  - ${problems.join('\n  - ')}`)
  }

  log(`打包自检通过：resources/data 与 resources/models 均不存在；`
    + `提示词预设 ${promptFiles.length} 个；后端源码与 Python 运行时齐备`)
  return promptFiles.length
}

async function main() {
  if (!await commandExists('uv')) {
    throw new Error('uv is not available in PATH')
  }

  if (!await commandExists('npm')) {
    throw new Error('npm is not available in PATH')
  }

  await prepareBundledPython()

  log('Building renderer bundle')
  await run('npm', ['run', 'build'])

  const targetFlag = getTargetFlag()
  const builderTargetArgs = getBuilderTargetArgs()
  log(`Running electron-builder ${builderTargetArgs.join(' ')}`)
  await run('npx', ['electron-builder', ...builderTargetArgs])

  verifyPackagedResources()

  log('Packaging completed. Output is in release/')
}

main().catch((error) => {
  console.error(`[package] ${error.message}`)
  process.exit(1)
})
