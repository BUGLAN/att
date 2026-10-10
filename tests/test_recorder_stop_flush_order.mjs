// 静态断言：流式录音停止时必须"先冲刷尾包，再收口采集"。
//
// 为什么只能做静态断言：`stopStreamingRecording` 的这段顺序编排依赖 WebSocket / AudioContext
// 全局对象，Node 侧无法直接执行；但顺序本身是纯文本可判定的，而这正是一个真实踩过的坑——
// `cleanupStreamingAudio()` 内部会 `resetStreamingBuffer()` 清空积压队列，若把
// `sendStreamingSamples(true)` 写在它之后，尾包永远发不出去，每次停止都静默丢掉不足一个
// STREAM_FLUSH_SAMPLES 块（最多 ≈200ms）的语音（与"按下热键开头丢字"对称的缺陷，实测一次
// 录音丢掉 192 采样）。
//
// 边界说明：这是**文本顺序**护栏，不是行为保证（可达性/条件正确性仍需 code review）。
// 已用变异实测确认：把冲刷挪到 cleanup 之后会失败；把冲刷挪出 `if (socketReadyForFinish)`
// 分支或换掉该条件也会失败。

import test from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const recorderSource = fs.readFileSync(
  path.join(repoRoot, 'src', 'composables', 'useRecorder.js'),
  'utf8'
)

test('流式停止：尾包冲刷必须早于 cleanupStreamingAudio()', () => {
  const start = recorderSource.indexOf('async function stopStreamingRecording')
  assert.ok(start > -1, 'useRecorder.js 必须保留 stopStreamingRecording')

  // 只截取该函数体：下一个顶层 function 之前的内容
  const rest = recorderSource.slice(start + 1)
  const nextFunction = rest.search(/\n(async )?function /)
  const body = nextFunction === -1 ? rest : rest.slice(0, nextFunction)

  // 匹配"整行就是这条调用"的语句，避免把注释里提到的函数名也算进来
  const flushMatch = /^[ \t]*sendStreamingSamples\(true\)[ \t]*$/m.exec(body)
  const cleanupMatch = /^[ \t]*cleanupStreamingAudio\(\)[ \t]*$/m.exec(body)

  assert.ok(flushMatch, '停止路径必须强制冲刷积压的尾包（sendStreamingSamples(true)）')
  assert.ok(cleanupMatch, '停止路径必须收口采集（cleanupStreamingAudio()）')
  assert.ok(
    flushMatch.index < cleanupMatch.index,
    'sendStreamingSamples(true) 必须在 cleanupStreamingAudio() 之前——后者会 resetStreamingBuffer() 清空积压，顺序反了尾包会被静默丢掉'
  )

  // 光有顺序还不够：冲刷必须真的挂在"socket 已 OPEN"这个条件分支里，
  // 否则改成恒假条件（或挪到分支外）时上面的顺序断言依然成立。
  const guardIndex = body.indexOf('if (socketReadyForFinish) {')
  assert.ok(
    body.includes('const socketReadyForFinish = Boolean('),
    '必须用 socketReadyForFinish 缓存停止瞬间的 OPEN 判断，供冲刷与 stream_finish 复用'
  )
  assert.ok(guardIndex > -1, '尾包冲刷必须挂在 if (socketReadyForFinish) 分支下')
  assert.ok(
    flushMatch.index > guardIndex,
    'sendStreamingSamples(true) 必须位于 if (socketReadyForFinish) { 之后（同一同步段内）'
  )

  const guardBlockEnd = body.indexOf('\n  }', guardIndex)
  const guardBlock = guardBlockEnd === -1 ? body.slice(guardIndex) : body.slice(guardIndex, guardBlockEnd)
  assert.ok(
    /^[ \t]*sendStreamingSamples\(true\)[ \t]*$/m.test(guardBlock),
    'sendStreamingSamples(true) 必须位于 if (socketReadyForFinish) 分支体内'
  )
})
