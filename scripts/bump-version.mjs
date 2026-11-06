#!/usr/bin/env node
// 版本号一次改齐：package.json / package-lock.json / src-tauri/Cargo.toml / src-tauri/tauri.conf.json /
// src-tauri/Cargo.lock（工作区内 att-* 包）。只做定点文本替换，不重排文件格式。
//
// 用法：
//   npm run version:bump -- 2.8.0     # 改版本号
//   node scripts/bump-version.mjs --check   # 只校验各处是否一致（tests/test_version_sync.mjs 也会校验）

import { readFileSync, writeFileSync } from 'node:fs'
import path from 'node:path'
import process from 'node:process'
import { fileURLToPath, pathToFileURL } from 'node:url'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')

export const VERSION_FILES = Object.freeze({
  packageJson: 'package.json',
  packageLock: 'package-lock.json',
  cargoToml: 'src-tauri/Cargo.toml',
  tauriConf: 'src-tauri/tauri.conf.json',
  cargoLock: 'src-tauri/Cargo.lock'
})

const SEMVER = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/

// 每个文件里「属于本应用的版本号」位置：正则的第 1、3 组是前后文，第 2 组是版本号
const PATTERNS = Object.freeze({
  // 顶层第一个 version 字段
  packageJson: [/^(\{[\s\S]*?\n  "version": ")([^"]+)(")/],
  packageLock: [
    /^(\{[\s\S]*?\n  "version": ")([^"]+)(")/,
    /("packages": \{\s*"": \{[^}]*?"version": ")([^"]+)(")/
  ],
  cargoToml: [/(\[workspace\.package\][^[]*?\nversion = ")([^"]+)(")/],
  tauriConf: [/^(\{[\s\S]*?\n  "version": ")([^"]+)(")/],
  // 工作区成员都叫 att-*，与第三方包不重名
  cargoLock: [/(\[\[package\]\]\nname = "att-[a-z-]+"\nversion = ")([^"]+)(")/g]
})

/** 读出各文件里的本应用版本号：{ key: [版本号…] }。 */
export function readVersions(texts) {
  const versions = {}
  for (const [key, patterns] of Object.entries(PATTERNS)) {
    versions[key] = []
    for (const pattern of patterns) {
      const matches = pattern.global ? [...texts[key].matchAll(pattern)] : [texts[key].match(pattern)].filter(Boolean)
      if (matches.length === 0) {
        throw new Error(`${VERSION_FILES[key]} 里找不到版本号（格式变了？）`)
      }
      versions[key].push(...matches.map((match) => match[2]))
    }
  }
  return versions
}

/** 把各文件里的本应用版本号替换为 version，返回新文本。 */
export function applyVersion(texts, version) {
  if (!SEMVER.test(version)) {
    throw new Error(`版本号格式不对：${version}（应形如 2.8.0）`)
  }
  readVersions(texts) // 先确认每处都能定位到
  const next = {}
  for (const [key, patterns] of Object.entries(PATTERNS)) {
    next[key] = patterns.reduce((text, pattern) => text.replace(pattern, `$1${version}$3`), texts[key])
  }
  return next
}

export function loadTexts(root = repoRoot) {
  return Object.fromEntries(
    Object.entries(VERSION_FILES).map(([key, file]) => [key, readFileSync(path.join(root, file), 'utf8')])
  )
}

function main(args) {
  const texts = loadTexts()
  if (args[0] === '--check' || args.length === 0) {
    const versions = readVersions(texts)
    const distinct = new Set(Object.values(versions).flat())
    for (const [key, list] of Object.entries(versions)) {
      console.log(`${VERSION_FILES[key].padEnd(28)} ${[...new Set(list)].join(', ')}`)
    }
    if (distinct.size !== 1) {
      console.error('版本号不一致，用 npm run version:bump -- <版本号> 改齐')
      process.exit(1)
    }
    if (args.length === 0) {
      console.log('\n用法：npm run version:bump -- <版本号>')
    }
    return
  }

  const version = args[0].replace(/^v/, '')
  const next = applyVersion(texts, version)
  for (const [key, text] of Object.entries(next)) {
    if (text !== texts[key]) {
      writeFileSync(path.join(repoRoot, VERSION_FILES[key]), text)
      console.log(`已更新 ${VERSION_FILES[key]}`)
    }
  }
  console.log(`版本号 → ${version}`)
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main(process.argv.slice(2))
}
