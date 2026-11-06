// 版本号一致性（scripts/bump-version.mjs）：五处版本号必须一致，改版本号只动本应用的字段。
import test from 'node:test'
import assert from 'node:assert/strict'

import { applyVersion, loadTexts, readVersions } from '../scripts/bump-version.mjs'

test('仓库里各处版本号一致', () => {
  const versions = readVersions(loadTexts())
  const distinct = new Set(Object.values(versions).flat())
  assert.equal(distinct.size, 1, `版本号不一致：${JSON.stringify(versions)}`)
  assert.equal(versions.cargoLock.length, 7, 'Cargo.lock 里应有 7 个工作区成员')
  assert.equal(versions.packageLock.length, 2, 'package-lock.json 顶层与 packages[""] 各一处')
})

test('applyVersion：只替换本应用的版本号，其余内容原样保留', () => {
  const texts = loadTexts()
  const next = applyVersion(texts, '9.9.9')
  const versions = readVersions(next)
  assert.deepEqual([...new Set(Object.values(versions).flat())], ['9.9.9'])

  for (const key of Object.keys(texts)) {
    const before = texts[key].split('\n')
    const after = next[key].split('\n')
    assert.equal(after.length, before.length, `${key} 行数不应变化`)
    const changed = before.filter((line, index) => line !== after[index])
    assert.ok(changed.every((line) => /version/.test(line)), `${key} 只应改版本号所在行：${changed.join(' | ')}`)
    assert.equal(changed.length, versions[key].length, `${key} 改动行数应等于版本号处数`)
  }
})

test('applyVersion：拒绝不合法的版本号', () => {
  const texts = loadTexts()
  assert.throws(() => applyVersion(texts, '2.8'), /版本号格式不对/)
  assert.throws(() => applyVersion(texts, 'latest'), /版本号格式不对/)
})
