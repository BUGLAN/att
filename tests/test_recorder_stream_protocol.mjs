// 流式协议模块（src/composables/recorder/streamProtocol.mjs）单元测试。
// 覆盖正常路径 + 边界：非法 JSON、未知消息类型、缺失字段、关闭动作三种迁移。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  DEFAULT_STREAM_LANGUAGE,
  STREAMING_ASR_PROVIDERS,
  STREAM_CLOSE_ACTIONS,
  STREAM_ERROR_FALLBACK_MESSAGE,
  STREAM_MESSAGE_TYPES,
  STREAM_START_TYPE,
  classifyStreamMessage,
  deriveStreamCloseAction,
  deriveStreamErrorMessage,
  deriveStreamFinishedPayload,
  deriveStreamFinishPayload,
  deriveStreamPartialText,
  deriveStreamReadySessionId,
  deriveStreamStartPayload,
  deriveStreamingProviderLabel,
  isKnownStreamMessage,
  isStreamingProviderSupported,
  parseStreamMessage
} from '../src/composables/recorder/streamProtocol.mjs'

test('parseStreamMessage：合法 JSON 解析成功', () => {
  const parsed = parseStreamMessage('{"type":"stream_ready","data":{"session_id":"s1"}}')
  assert.equal(parsed.ok, true)
  assert.equal(parsed.message.type, 'stream_ready')
  assert.equal(parsed.message.data.session_id, 's1')
})

test('parseStreamMessage：非法 JSON 返回失败且不抛错', () => {
  const parsed = parseStreamMessage('not-json')
  assert.equal(parsed.ok, false)
  assert.ok(parsed.error instanceof SyntaxError)
})

test('parseStreamMessage：空字符串与二进制负载都走失败分支', () => {
  assert.equal(parseStreamMessage('').ok, false)
  assert.equal(parseStreamMessage(new ArrayBuffer(4)).ok, false)
})

test('classifyStreamMessage：五种已知类型都能识别', () => {
  assert.equal(classifyStreamMessage({ type: 'stream_ready' }), STREAM_MESSAGE_TYPES.READY)
  assert.equal(classifyStreamMessage({ type: 'stream_partial' }), STREAM_MESSAGE_TYPES.PARTIAL)
  assert.equal(classifyStreamMessage({ type: 'stream_finished' }), STREAM_MESSAGE_TYPES.FINISHED)
  assert.equal(classifyStreamMessage({ type: 'stream_cancelled' }), STREAM_MESSAGE_TYPES.CANCELLED)
  assert.equal(classifyStreamMessage({ type: 'stream_error' }), STREAM_MESSAGE_TYPES.ERROR)
})

test('classifyStreamMessage：非法 / 缺失类型归为 unknown', () => {
  assert.equal(classifyStreamMessage({ type: 'stream_unknown' }), 'unknown')
  assert.equal(classifyStreamMessage({}), 'unknown')
  assert.equal(classifyStreamMessage({ type: 123 }), 'unknown')
  assert.equal(classifyStreamMessage('stream_ready'), 'unknown')
})

test('classifyStreamMessage：null 消息体仍抛 TypeError（与原实现失败语义一致）', () => {
  assert.throws(() => classifyStreamMessage(null), TypeError)
})

test('isKnownStreamMessage：区分已知与非法消息类型', () => {
  assert.equal(isKnownStreamMessage({ type: 'stream_partial' }), true)
  assert.equal(isKnownStreamMessage({ type: 'stream_unknown' }), false)
})

test('deriveStreamStartPayload：语言为空时回退中文', () => {
  assert.deepEqual(deriveStreamStartPayload('en'), { type: STREAM_START_TYPE, language: 'en' })
  assert.deepEqual(deriveStreamStartPayload(''), { type: STREAM_START_TYPE, language: DEFAULT_STREAM_LANGUAGE })
  assert.deepEqual(deriveStreamStartPayload(undefined), {
    type: STREAM_START_TYPE,
    language: DEFAULT_STREAM_LANGUAGE
  })
})

test('deriveStreamFinishPayload：每次返回独立对象', () => {
  const first = deriveStreamFinishPayload()
  const second = deriveStreamFinishPayload()
  assert.deepEqual(first, { type: 'stream_finish' })
  assert.notEqual(first, second)
})

test('deriveStreamReadySessionId：缺失或空会话 id 回退 null', () => {
  assert.equal(deriveStreamReadySessionId({ data: { session_id: 's1' } }), 's1')
  assert.equal(deriveStreamReadySessionId({ data: {} }), null)
  assert.equal(deriveStreamReadySessionId({ data: { session_id: '' } }), null)
  assert.equal(deriveStreamReadySessionId({}), null)
})

test('deriveStreamPartialText：缺失文本回退空串', () => {
  assert.equal(deriveStreamPartialText({ data: { text: '你好' } }), '你好')
  assert.equal(deriveStreamPartialText({ data: {} }), '')
  assert.equal(deriveStreamPartialText({}), '')
})

test('deriveStreamFinishedPayload：finalText 优先级链与原实现一致', () => {
  // 1) payload.optimized_text 优先
  assert.equal(
    deriveStreamFinishedPayload({
      data: { optimized_text: 'A', raw_text: 'B', record: { optimized_text: 'C', raw_text: 'D' } }
    }).finalText,
    'A'
  )
  // 2) 退到 payload.raw_text
  assert.equal(
    deriveStreamFinishedPayload({ data: { raw_text: 'B', record: { optimized_text: 'C' } } }).finalText,
    'B'
  )
  // 3) 再退到 record.optimized_text
  assert.equal(
    deriveStreamFinishedPayload({ data: { record: { optimized_text: 'C', raw_text: 'D' } } }).finalText,
    'C'
  )
  // 4) 再退到 record.raw_text
  assert.equal(deriveStreamFinishedPayload({ data: { record: { raw_text: 'D' } } }).finalText, 'D')
  // 5) 全空回退空串
  assert.equal(deriveStreamFinishedPayload({ data: {} }).finalText, '')
  assert.equal(deriveStreamFinishedPayload({}).finalText, '')
})

test('deriveStreamFinishedPayload：record 缺失时为 null，data 非法时按空对象处理', () => {
  assert.equal(deriveStreamFinishedPayload({ data: {} }).record, null)
  assert.equal(deriveStreamFinishedPayload({ data: 'oops' }).finalText, '')
  assert.deepEqual(deriveStreamFinishedPayload({}).payload, {})
})

test('deriveStreamErrorMessage：缺失 / 空错误回退兜底文案', () => {
  assert.equal(deriveStreamErrorMessage({ data: { error: '模型超时' } }), '模型超时')
  assert.equal(deriveStreamErrorMessage({ data: {} }), STREAM_ERROR_FALLBACK_MESSAGE)
  assert.equal(deriveStreamErrorMessage({ data: { error: '' } }), STREAM_ERROR_FALLBACK_MESSAGE)
  assert.equal(deriveStreamErrorMessage({}), STREAM_ERROR_FALLBACK_MESSAGE)
})

test('deriveStreamCloseAction：连接建立阶段关闭 -> fail', () => {
  assert.equal(
    deriveStreamCloseAction({ settled: false, awaitingFinish: false, connectionState: 'connecting' }),
    STREAM_CLOSE_ACTIONS.FAIL
  )
})

test('deriveStreamCloseAction：录音过程中意外断开 -> 完整收口', () => {
  // 连接已 settle 但状态仍停在 connecting（onopen 之后未被 ready 推进）也算意外断开
  assert.equal(
    deriveStreamCloseAction({ settled: true, awaitingFinish: false, connectionState: 'connecting' }),
    STREAM_CLOSE_ACTIONS.UNEXPECTED
  )
  assert.equal(
    deriveStreamCloseAction({ settled: true, awaitingFinish: false, connectionState: 'streaming' }),
    STREAM_CLOSE_ACTIONS.UNEXPECTED
  )
  assert.equal(
    deriveStreamCloseAction({ settled: true, awaitingFinish: false, connectionState: 'error' }),
    STREAM_CLOSE_ACTIONS.UNEXPECTED
  )
})

test('deriveStreamCloseAction：等待收尾时断连 -> finish-interrupted（结果永远到不了，必须收口）', () => {
  assert.equal(
    deriveStreamCloseAction({ settled: true, awaitingFinish: true, connectionState: 'streaming' }),
    STREAM_CLOSE_ACTIONS.FINISH_INTERRUPTED
  )
  assert.equal(
    deriveStreamCloseAction({ settled: true, awaitingFinish: true, connectionState: 'connecting' }),
    STREAM_CLOSE_ACTIONS.FINISH_INTERRUPTED
  )
})

test('deriveStreamCloseAction：已复位 -> ignore', () => {
  assert.equal(
    deriveStreamCloseAction({ settled: false, awaitingFinish: false, connectionState: 'idle' }),
    STREAM_CLOSE_ACTIONS.IGNORE
  )
  assert.equal(
    deriveStreamCloseAction({ settled: true, awaitingFinish: false, connectionState: 'idle' }),
    STREAM_CLOSE_ACTIONS.IGNORE
  )
})

test('isStreamingProviderSupported：Qwen 与豆包都支持真流式', () => {
  assert.deepEqual([...STREAMING_ASR_PROVIDERS], ['qwen_asr', 'doubao_asr'])
  assert.equal(isStreamingProviderSupported('qwen_asr'), true)
  assert.equal(isStreamingProviderSupported('doubao_asr'), true)
})

test('isStreamingProviderSupported：非流式 provider 与空值都不支持', () => {
  assert.equal(isStreamingProviderSupported('whisper'), false)
  assert.equal(isStreamingProviderSupported(''), false)
  assert.equal(isStreamingProviderSupported(undefined), false)
  assert.equal(isStreamingProviderSupported(null), false)
})

test('deriveStreamingProviderLabel：按 provider 给出展示名，未知回退通用文案', () => {
  assert.equal(deriveStreamingProviderLabel('qwen_asr'), 'Qwen 真流式')
  assert.equal(deriveStreamingProviderLabel('doubao_asr'), '豆包在线流式')
  assert.equal(deriveStreamingProviderLabel('whisper'), '真流式')
  assert.equal(deriveStreamingProviderLabel(undefined), '真流式')
})
