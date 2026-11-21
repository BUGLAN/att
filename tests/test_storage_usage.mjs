// 「存储」展示模型（src/components/settings/storageUsage.mjs）单元测试。
// 覆盖：字节格式、占用表行、孤儿音频判定、保留天数选项与确认文案。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  audioRetentionConfirm,
  audioRetentionOptions,
  displayPath,
  formatBytes,
  hasOrphans,
  isTighter,
  storageRows
} from '../src/components/settings/storageUsage.mjs'

test('字节格式：1024 进制、一位小数、整数去掉 .0', () => {
  assert.equal(formatBytes(0), '0 B')
  assert.equal(formatBytes(1023), '1023 B')
  assert.equal(formatBytes(1024), '1 KB')
  assert.equal(formatBytes(1536), '1.5 KB')
  assert.equal(formatBytes(319 * 1024 * 1024), '319 MB')
  assert.equal(formatBytes(3.2 * 1024 ** 3), '3.2 GB')
  assert.equal(formatBytes(undefined), '0 B')
})

test('占用表：每项的大小与文件数，空目录标出来', () => {
  const report = {
    items: [
      { key: 'audio', label: '录音音频', path: '/data/audio/records', files: 12, bytes: 5 * 1024 * 1024 },
      { key: 'orphans', label: '孤儿音频', path: '/data/audio/orphans', files: 0, bytes: 0 }
    ],
    total_bytes: 5 * 1024 * 1024
  }
  assert.deepEqual(storageRows({ ...report, data_dir: '/data' })[0], {
    key: 'audio',
    label: '录音音频',
    path: 'audio/records',
    fullPath: '/data/audio/records',
    size: '5 MB',
    files: '12 个文件',
    empty: false
  })
  assert.equal(storageRows(report)[1].empty, true)
  assert.equal(hasOrphans(report), false)
  assert.equal(hasOrphans({ items: [{ key: 'orphans', files: 3 }] }), true)
  assert.deepEqual(storageRows(null), [])
})

test('路径显示：数据目录内的显示相对路径，目录外的显示完整路径', () => {
  assert.equal(displayPath('/Users/a/data/audio/records', '/Users/a/data'), 'audio/records')
  assert.equal(displayPath('C:\\att\\data\\logs', 'C:\\att\\data'), 'logs')
  assert.equal(displayPath('/Users/a/models', '/Users/a/data'), '/Users/a/models')
  assert.equal(displayPath('/Users/a/data-old/x', '/Users/a/data'), '/Users/a/data-old/x', '前缀相同但不是子目录')
  assert.equal(displayPath('/x', ''), '/x')
})

test('录音音频保留天数：永久在首项，改紧才确认', () => {
  assert.deepEqual(audioRetentionOptions(0)[0], { value: 0, label: '永久保留' })
  assert.ok(audioRetentionOptions(0).some((option) => option.label === '30 天后删除音频'))
  assert.ok(audioRetentionOptions(45).some((option) => option.value === 45))
  assert.equal(isTighter(0, 30), true)
  assert.equal(isTighter(30, 90), false)
  const copy = audioRetentionConfirm(30)
  assert.equal(copy.title, '只保留 30 天内的录音音频？')
  assert.match(copy.description, /记录和文字都保留/)
  assert.match(copy.description, /收藏的记录不受影响/)
})
