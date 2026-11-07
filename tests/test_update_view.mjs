// 设置页「关于」检查更新展示逻辑（src/components/settings/updateView.mjs）单元测试。
// 覆盖：有新版本 / 已是最新 / 检查失败三种反馈、版本号标记与行内说明，下载进度与可否下载，发布说明摘要，
// 以及应用内更新（下好后「重启并更新」、安装中、失败退回下载安装包）。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  canFallbackToInstaller,
  canInstall,
  canStartDownload,
  describeCheck,
  describeCheckLine,
  versionBadge,
  describeDownload,
  downloadPercent,
  formatBytes,
  formatDateTime,
  summarizeNotes
} from '../src/components/settings/updateView.mjs'

const AVAILABLE = {
  status: 'available',
  current_version: '2.5.1',
  latest_version: '2.6.0',
  published_at: '2026-11-04T13:23:57Z',
  asset: { name: 'Luomo_2.6.0_aarch64.dmg', url: 'https://x', size: 7491339 }
}

test('检查结果：三种情况都有明确反馈，失败带原因', () => {
  assert.equal(describeCheck(null), null, '没检查过不显示')

  const available = describeCheck(AVAILABLE)
  assert.equal(available.tone, 'accent')
  assert.equal(available.title, '发现新版本 v2.6.0')
  assert.match(available.detail, /当前 v2\.5\.1/)
  assert.match(available.detail, /安装包 7\.1 MB/)

  const noAsset = describeCheck({ ...AVAILABLE, asset: null })
  assert.match(noAsset.detail, /没有适合本机的安装包/)

  assert.deepEqual(describeCheck({ status: 'latest', current_version: '2.5.1' }), {
    tone: 'ok', title: '已是最新版本（v2.5.1）', detail: ''
  })

  const failed = describeCheck({ status: 'error', current_version: '2.5.1', error: '找不到发布信息（HTTP 404）' })
  assert.equal(failed.tone, 'err')
  assert.equal(failed.title, '检查更新失败')
  assert.equal(failed.detail, '找不到发布信息（HTTP 404）')
  assert.equal(describeCheck({ status: 'error' }).detail, '未知原因')
})

test('版本号标记与检查更新行：已是最新只给标记，失败原因写在行内', () => {
  assert.equal(versionBadge(null), null, '没检查过不显示')
  assert.deepEqual(versionBadge({ status: 'latest' }), { tone: 'ok', text: '已是最新' })
  assert.deepEqual(versionBadge(AVAILABLE), { tone: 'accent', text: '可更新至 v2.6.0' })
  assert.equal(versionBadge({ status: 'error', error: 'x' }), null)

  assert.deepEqual(describeCheckLine(null), {
    error: '', meta: ''
  })
  const latest = describeCheckLine({ status: 'latest', checked_at: '2026-11-04T07:22:00Z' })
  assert.equal(latest.error, '')
  assert.match(latest.meta, /^上次检查：2026-11-0\d \d{2}:\d{2}$/)
  assert.deepEqual(describeCheckLine({ status: 'error', error: 'HTTP 404' }), {
    error: '检查失败：HTTP 404', meta: ''
  })
})

test('下载：进度、完成、失败文案与能否开始下载', () => {
  assert.equal(describeDownload({ status: 'idle' }), '')
  const downloading = { status: 'downloading', received: 512 * 1024, total: 2 * 1024 * 1024 }
  assert.equal(downloadPercent(downloading), 25)
  assert.equal(describeDownload(downloading), '正在下载… 25%（512.0 KB / 2.0 MB）')
  assert.equal(downloadPercent({ status: 'downloading', received: 10 }), null, '总大小未知')
  assert.equal(describeDownload({ status: 'downloading', received: 10 }), '正在下载… 10 B')
  assert.match(describeDownload({ status: 'done', version: '2.6.0', file_name: 'a.dmg' }), /已下载 v2\.6\.0：a\.dmg/)
  assert.equal(describeDownload({ status: 'error', error: '下载地址返回 HTTP 404' }), '下载失败：下载地址返回 HTTP 404')

  assert.equal(canStartDownload(AVAILABLE, { status: 'idle' }), true)
  assert.equal(canStartDownload(AVAILABLE, { status: 'downloading' }), false)
  assert.equal(canStartDownload(AVAILABLE, { status: 'done', version: '2.6.0' }), false, '同版本已下载')
  assert.equal(canStartDownload(AVAILABLE, { status: 'done', version: '2.5.9' }), true, '下载的是旧的新版本')
  assert.equal(canStartDownload(AVAILABLE, { status: 'error' }), true, '失败后可重试')
  assert.equal(canStartDownload({ ...AVAILABLE, asset: null }, { status: 'idle' }), false)
  assert.equal(canStartDownload({ status: 'latest' }, { status: 'idle' }), false)
})

test('格式化：字节、时间与发布说明摘要', () => {
  assert.equal(formatBytes(0), '0 B')
  assert.equal(formatBytes(1536), '1.5 KB')
  assert.equal(formatDateTime('not a date'), '')
  assert.match(formatDateTime('2026-11-04T13:23:57Z'), /^2026-11-0\d \d{2}:\d{2}$/)
  assert.equal(summarizeNotes('a\n\nb\r\nc', 2), 'a\nb\n…')
  assert.equal(summarizeNotes(null), '')
})

test('应用内更新：不依赖安装包、下好后可重启更新、安装中不能再下载', () => {
  const noAsset = { ...AVAILABLE, asset: null }
  assert.doesNotMatch(describeCheck(noAsset, { inApp: true }).detail, /安装包/, '应用内更新不提安装包')
  assert.equal(canStartDownload(noAsset, { status: 'idle' }, { inApp: true }), true, '没有本机安装包也能应用内更新')

  const done = { status: 'done', mode: 'in_app', version: '2.6.0' }
  assert.equal(describeDownload(done), 'v2.6.0 已下载，点「重启并更新」即可完成更新')
  assert.equal(canInstall(done), true)
  assert.equal(canInstall({ ...done, mode: 'installer' }), false, '完整安装包要用户自己打开')
  assert.equal(canStartDownload(AVAILABLE, done, { inApp: true }), false, '同版本已下好')

  const installing = { status: 'installing', mode: 'in_app', version: '2.6.0' }
  assert.equal(describeDownload(installing), '正在安装更新，应用会自动重启…')
  assert.equal(canInstall(installing), false)
  assert.equal(canStartDownload(AVAILABLE, installing, { inApp: true }), false)
})

test('应用内更新失败：可重试，发布里有本机安装包时还可改为下载安装包', () => {
  const failed = { status: 'error', mode: 'in_app', error: '读取更新清单失败：HTTP 404' }
  assert.equal(describeDownload(failed), '更新失败：读取更新清单失败：HTTP 404')
  assert.equal(canStartDownload(AVAILABLE, failed, { inApp: true }), true, '可重试')
  assert.equal(canFallbackToInstaller(AVAILABLE, failed), true)
  assert.equal(canFallbackToInstaller({ ...AVAILABLE, asset: null }, failed), false, '没有本机安装包')
  assert.equal(canFallbackToInstaller(AVAILABLE, { status: 'error', mode: 'installer' }), false, '已经是安装包下载')
  assert.equal(canFallbackToInstaller(AVAILABLE, { status: 'downloading', mode: 'in_app' }), false)
})
