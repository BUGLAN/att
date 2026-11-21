// 测试：导出文件名构造（src/utils/exportDownload.mjs）
import assert from 'node:assert/strict'
import {
  buildBatchExportFilename,
  buildExportFilename,
  buildRecordsExportQuery,
  EXPORT_FORMATS
} from '../src/utils/exportDownload.mjs'

function main() {
  // 正常时间戳 + uuid id
  const name = buildExportFilename(
    { id: 'a1b2c3d4-e5f6-7890-abcd-ef0123456789', created_at: '2026-10-12T09:05:00Z' },
    'txt'
  )
  assert.match(name, /^att-\d{8}-\d{4}-a1b2c3d4\.txt$/, `文件名格式不符：${name}`)

  // JSON 扩展名
  const jsonName = buildExportFilename({ id: 'zz', created_at: '2026-01-02T03:04:00Z' }, 'json')
  assert.ok(jsonName.endsWith('.json'), `JSON 导出扩展名不对：${jsonName}`)

  // 缺失 created_at 时回落 export 占位
  const noTime = buildExportFilename({ id: 'abcdef12' }, 'txt')
  assert.match(noTime, /^att-export-abcdef12\.txt$/, `缺失时间应回落：${noTime}`)

  // 非法时间字符串同样回落
  const badTime = buildExportFilename({ id: 'x', created_at: 'not-a-date' }, 'txt')
  assert.match(badTime, /^att-export-x\.txt$/, `非法时间应回落：${badTime}`)

  // 批量导出：选中的 id 与筛选条件二选一
  const byIds = new URLSearchParams(buildRecordsExportQuery({ ids: ['a', 'b'], format: 'md' }))
  assert.equal(byIds.get('ids'), 'a,b')
  assert.equal(byIds.get('format'), 'md')
  assert.equal(byIds.get('use_optimized'), 'true')
  assert.equal(byIds.get('status'), null, '按 id 导出时不带筛选条件')

  const byFilter = new URLSearchParams(buildRecordsExportQuery({
    filter: { q: '会议', search_type: 'all', favorite_only: true, status: 'failed', date_from: '2026-11-01', date_to: '2026-11-21' },
    format: 'csv',
    useOptimized: false
  }))
  assert.equal(byFilter.get('ids'), null)
  assert.equal(byFilter.get('q'), '会议')
  assert.equal(byFilter.get('search_type'), null, '默认范围不重复传')
  assert.equal(byFilter.get('favorite_only'), 'true')
  assert.equal(byFilter.get('status'), 'failed')
  assert.equal(byFilter.get('date_from'), '2026-11-01')
  assert.equal(byFilter.get('date_to'), '2026-11-21')
  assert.equal(byFilter.get('use_optimized'), 'false')

  assert.equal(buildBatchExportFilename('csv', 12, new Date(2026, 10, 21, 9, 5)), 'att-records-20261121-0905-12.csv')
  assert.deepEqual(EXPORT_FORMATS.map((item) => item.value), ['md', 'txt', 'json', 'csv'])

  console.log('[export-download] 全部断言通过')
}

main()
