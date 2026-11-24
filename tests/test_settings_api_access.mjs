// 设置 → 诊断 →「外部 API」纯逻辑单测（src/components/settings/apiAccess.mjs）。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  addAllowedDirectory,
  apiTokenDisplay,
  isAbsoluteDirectoryPath,
  normalizeDirectoryEntry
} from '../src/components/settings/apiAccess.mjs'

test('目录输入去除空白与「复制为路径」的引号', () => {
  assert.equal(normalizeDirectoryEntry('  "D:\\audio"  '), 'D:\\audio')
  assert.equal(normalizeDirectoryEntry(' /Users/me/audio '), '/Users/me/audio')
  assert.equal(normalizeDirectoryEntry('"unpaired'), '"unpaired')
})

test('只接受绝对路径（盘符 / UNC / POSIX），拒绝相对路径与空值', () => {
  for (const ok of ['D:\\audio', 'c:/media', '\\\\nas\\share\\audio', '/Users/me/audio']) {
    assert.equal(isAbsoluteDirectoryPath(ok), true, ok)
  }
  for (const bad of ['', '  ', 'audio', './audio', '../audio', 'C:', '\\\\nas']) {
    assert.equal(isAbsoluteDirectoryPath(bad), false, bad)
  }
})

test('添加目录：规整、去重、返回新数组且不原地修改', () => {
  const list = ['D:\\audio']
  const duplicated = addAllowedDirectory(list, ' d:/audio/ ')
  assert.equal(duplicated.error, '该目录已在列表中')
  assert.deepEqual(duplicated.list, ['D:\\audio'])
  assert.deepEqual(list, ['D:\\audio'], '原数组不能被改写')

  const added = addAllowedDirectory(list, ' "E:\\recordings" ')
  assert.equal(added.error, '')
  assert.deepEqual(added.list, ['D:\\audio', 'E:\\recordings'])
})

test('添加目录：空值与相对路径给出可照做的中文提示', () => {
  assert.equal(addAllowedDirectory([], '   ').error, '请输入目录路径')
  const relative = addAllowedDirectory([], 'recordings')
  assert.match(relative.error, /绝对路径/)
  assert.deepEqual(relative.list, [])
})

test('令牌显示：按开关 / 是否生成 / 是否点开切换', () => {
  assert.equal(apiTokenDisplay({ enabled: false, hasToken: false, revealedToken: '' }), '未开启')
  assert.equal(apiTokenDisplay({ enabled: true, hasToken: false, revealedToken: '' }), '正在生成…')
  assert.equal(apiTokenDisplay({ enabled: true, hasToken: true, revealedToken: '' }), 'att_••••••••••••••••')
  assert.equal(
    apiTokenDisplay({ enabled: true, hasToken: true, revealedToken: 'att_secret' }),
    'att_secret'
  )
})
