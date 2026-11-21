// macOS 更新后权限引导（src/components/settings/permissionGuide.mjs）单元测试。
// 覆盖：只在更新后首次启动且有没生效的项时弹出、版本文案、重新检测后过滤、恢复后的提示。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  restoredMessage,
  stillMissing,
  updatePermissionGuide,
  UPDATE_PERMISSION_STEPS
} from '../src/components/settings/permissionGuide.mjs'

const notice = {
  show: true,
  previous_version: '2.8.0',
  current_version: '2.9.0',
  items: [
    { id: 'input_monitoring', label: '输入监控', state: 'denied' },
    { id: 'accessibility', label: '辅助功能', state: 'denied' }
  ]
}

test('更新后首次启动且有没生效的项：弹出引导并说明版本与影响', () => {
  const guide = updatePermissionGuide(notice)
  assert.equal(guide.show, true)
  assert.equal(guide.versionText, '落墨已从 2.8.0 更新到 2.9.0。')
  assert.deepEqual(guide.items.map((item) => item.id), ['input_monitoring', 'accessibility'])
  assert.match(guide.items[0].impact, /快捷键/)
  assert.match(guide.items[1].impact, /自动粘贴/)
  assert.ok(UPDATE_PERMISSION_STEPS.some((step) => step.includes('删掉旧条目')), '步骤要写明先删旧条目')
})

test('不是更新后首次启动、浏览器模式、权限都正常：不弹', () => {
  assert.equal(updatePermissionGuide(null).show, false)
  assert.equal(updatePermissionGuide({ show: false, items: [] }).show, false)
  assert.equal(updatePermissionGuide({ ...notice, items: [{ id: 'accessibility', state: 'granted' }] }).show, false)
})

test('从没记过版本号的旧版本升级上来：版本文案只写新版本', () => {
  assert.equal(updatePermissionGuide({ ...notice, previous_version: '' }).versionText, '落墨已更新到 2.9.0。')
})

test('重新检测：只留下仍没生效的项，查不到状态按未生效', () => {
  const guide = updatePermissionGuide(notice)
  assert.deepEqual(
    stillMissing(guide.items, [{ id: 'input_monitoring', state: 'granted' }, { id: 'accessibility', state: 'denied' }])
      .map((item) => item.id),
    ['accessibility']
  )
  assert.deepEqual(stillMissing(guide.items, []).map((item) => item.id), ['input_monitoring', 'accessibility'])
  assert.deepEqual(stillMissing(guide.items, [
    { id: 'input_monitoring', state: 'granted' },
    { id: 'accessibility', state: 'granted' }
  ]), [])
})

test('恢复提示：输入监控要重启才生效', () => {
  assert.match(restoredMessage([{ id: 'input_monitoring' }]), /重新打开/)
  assert.equal(restoredMessage([{ id: 'accessibility' }]), '权限已恢复')
})
