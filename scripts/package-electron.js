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
  log(`Running electron-builder ${targetFlag}`)
  await run('npx', ['electron-builder', targetFlag])

  log('Packaging completed. Output is in release/')
}

main().catch((error) => {
  console.error(`[package] ${error.message}`)
  process.exit(1)
})
