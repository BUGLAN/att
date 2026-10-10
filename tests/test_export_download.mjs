// 测试：导出文件名构造（src/utils/exportDownload.mjs）
import assert from 'node:assert/strict'
import { buildExportFilename } from '../src/utils/exportDownload.mjs'

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

  console.log('[export-download] 全部断言通过')
}

main()
