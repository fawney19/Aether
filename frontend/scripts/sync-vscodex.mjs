import { spawnSync } from 'node:child_process'
import { cpSync, existsSync, mkdirSync, readFileSync, rmSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'

const frontendRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const moduleRoot = resolve(frontendRoot, '..', 'aether-vscodex')
const webRoot = resolve(moduleRoot, 'web')
const webPackagePath = resolve(webRoot, 'package.json')
const webLockPath = resolve(webRoot, 'package-lock.json')
const webPackage = JSON.parse(readFileSync(webPackagePath, 'utf8'))
const webLock = JSON.parse(readFileSync(webLockPath, 'utf8'))
const npmCommand = process.platform === 'win32' ? 'npm.cmd' : 'npm'
const requiredPackages = Object.keys({
  ...webPackage.dependencies,
  ...webPackage.devDependencies
})
const requiredCommands = ['vite', 'vue-tsc']
const commandSuffix = process.platform === 'win32' ? '.cmd' : ''

function dependencyMatchesLock(dependency) {
  const installedPackagePath = resolve(
    webRoot,
    'node_modules',
    ...dependency.split('/'),
    'package.json'
  )
  if (!existsSync(installedPackagePath)) {
    return false
  }

  const lockedVersion = webLock.packages?.[`node_modules/${dependency}`]?.version
  const installedVersion = JSON.parse(readFileSync(installedPackagePath, 'utf8')).version
  return typeof lockedVersion === 'string' && installedVersion === lockedVersion
}

const hasWebDependencies =
  requiredPackages.every(dependencyMatchesLock) &&
  requiredCommands.every((command) =>
    existsSync(resolve(webRoot, 'node_modules', '.bin', `${command}${commandSuffix}`))
  )

function runNpm(args, action) {
  const result = spawnSync(npmCommand, ['--prefix', webRoot, ...args], {
    stdio: 'inherit'
  })

  if (result.error) {
    throw new Error(`Failed to ${action}: ${result.error.message}`)
  }
  if (result.status !== 0) {
    throw new Error(`${action} failed with exit status ${result.status ?? 'unknown'}`)
  }
}

if (!hasWebDependencies) {
  console.log('=> aether-vscodex Web dependencies are missing; installing from package-lock.json...')
  runNpm(
    ['ci', '--include=dev', '--no-audit', '--no-fund'],
    'install aether-vscodex Web dependencies'
  )
}

runNpm(['run', 'build'], 'build aether-vscodex Web')

const vueBuild = resolve(webRoot, 'dist')
const destination = resolve(frontendRoot, 'public', 'aether-vscodex')

if (!existsSync(resolve(vueBuild, 'index.html'))) {
  throw new Error(`aether-vscodex Vue build was not found at ${vueBuild}`)
}

rmSync(destination, { recursive: true, force: true })
mkdirSync(destination, { recursive: true })
cpSync(vueBuild, destination, { recursive: true })
