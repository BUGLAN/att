// Qwen 真流式 WebSocket 协议层的纯逻辑：消息解析、类型归类、字段派生与关闭时的迁移判定。
//
// 本模块不依赖 Vue / 浏览器全局对象（不 new WebSocket、不碰 AudioContext），
// 只处理"拿到一条消息后应该怎么判"的纯判断，便于 Node 直接单测。
//
// 对照原 useRecorder.js 中 connectStreamingSocket 的内联实现：
//   JSON.parse(event.data) 解析、stream_ready / stream_partial / stream_finished /
//   stream_cancelled / stream_error 分支、onclose 的两段判定与 stream_start / stream_finish 报文。

// 服务端 -> 客户端的消息类型
export const STREAM_MESSAGE_TYPES = Object.freeze({
  READY: 'stream_ready',
  PARTIAL: 'stream_partial',
  FINISHED: 'stream_finished',
  CANCELLED: 'stream_cancelled',
  ERROR: 'stream_error'
})

// 客户端 -> 服务端的消息类型
export const STREAM_START_TYPE = 'stream_start'
export const STREAM_FINISH_TYPE = 'stream_finish'

// onclose 的三种迁移动作
export const STREAM_CLOSE_ACTIONS = Object.freeze({
  // 连接建立阶段就关闭：直接让 connect Promise 失败
  FAIL: 'fail',
  // 录音过程中意外断开：需要完整收口（停麦克风 / 停计时器 / 复位悬浮窗）
  UNEXPECTED: 'unexpected-close',
  // 正常收尾或已复位：什么都不做
  IGNORE: 'ignore'
})

// 无法识别的消息类型
export const UNKNOWN_STREAM_MESSAGE = 'unknown'

// 默认转写语言（原实现里的 `language || 'zh'`）
export const DEFAULT_STREAM_LANGUAGE = 'zh'

// 服务端未给出错误详情时的兜底文案
export const STREAM_ERROR_FALLBACK_MESSAGE = '流式转写失败'

// 支持真流式的 ASR provider：
// - qwen_asr：本地/远端桥接的 Qwen3-ASR
// - doubao_asr：火山引擎豆包双向流式（在线，无需加载本地模型）
export const STREAMING_ASR_PROVIDERS = Object.freeze(['qwen_asr', 'doubao_asr'])

// provider 在录音界面上的展示名（文案不再写死 Qwen）
const STREAMING_PROVIDER_LABELS = Object.freeze({
  qwen_asr: 'Qwen 真流式',
  doubao_asr: '豆包在线流式'
})

/**
 * 当前 provider 是否支持真流式录音。
 */
export function isStreamingProviderSupported(provider) {
  return STREAMING_ASR_PROVIDERS.includes(provider)
}

/**
 * 真流式 provider 的展示名；未知 provider 回落到通用文案。
 */
export function deriveStreamingProviderLabel(provider) {
  return STREAMING_PROVIDER_LABELS[provider] || '真流式'
}

/**
 * 解析一条 WS 文本消息。解析失败时返回 { ok: false, error }，由调用方决定是否抛出，
 * 从而保持原 try/catch 的收口路径不变。
 */
export function parseStreamMessage(raw) {
  try {
    return { ok: true, message: JSON.parse(raw) }
  } catch (error) {
    return { ok: false, error }
  }
}

/**
 * 归类消息类型。无法识别返回 UNKNOWN_STREAM_MESSAGE。
 *
 * 注意：这里刻意直接读取 `message.type`（不用可选链），
 * 与原先 `message.type === ...` 的失败语义保持一致——消息体为 null 时同样抛 TypeError。
 */
export function classifyStreamMessage(message) {
  switch (message.type) {
    case STREAM_MESSAGE_TYPES.READY:
      return STREAM_MESSAGE_TYPES.READY
    case STREAM_MESSAGE_TYPES.PARTIAL:
      return STREAM_MESSAGE_TYPES.PARTIAL
    case STREAM_MESSAGE_TYPES.FINISHED:
      return STREAM_MESSAGE_TYPES.FINISHED
    case STREAM_MESSAGE_TYPES.CANCELLED:
      return STREAM_MESSAGE_TYPES.CANCELLED
    case STREAM_MESSAGE_TYPES.ERROR:
      return STREAM_MESSAGE_TYPES.ERROR
    default:
      return UNKNOWN_STREAM_MESSAGE
  }
}

/**
 * 是否为已知消息类型。
 */
export function isKnownStreamMessage(message) {
  return classifyStreamMessage(message) !== UNKNOWN_STREAM_MESSAGE
}

/**
 * 构造 stream_start 报文。语言为空时回退中文。
 */
export function deriveStreamStartPayload(language) {
  return {
    type: STREAM_START_TYPE,
    language: language || DEFAULT_STREAM_LANGUAGE
  }
}

/**
 * 构造 stream_finish 报文（每次返回新对象，避免共享可变引用）。
 */
export function deriveStreamFinishPayload() {
  return { type: STREAM_FINISH_TYPE }
}

/**
 * 从 stream_ready 消息中取会话 id（对应原 `message.data?.session_id || null`）。
 */
export function deriveStreamReadySessionId(message) {
  return message.data?.session_id || null
}

/**
 * 从 stream_partial 消息中取中间转写文本（对应原 `message.data?.text || ''`）。
 */
export function deriveStreamPartialText(message) {
  return message.data?.text || ''
}

/**
 * 从 stream_finished 消息中派生落库所需字段。
 * finalText 的优先级链与原实现逐字一致：
 *   payload.optimized_text -> payload.raw_text -> record.optimized_text -> record.raw_text -> ''
 */
export function deriveStreamFinishedPayload(message) {
  const payload = message.data || {}
  const record = payload.record || null
  const finalText =
    payload.optimized_text ||
    payload.raw_text ||
    record?.optimized_text ||
    record?.raw_text ||
    ''

  return { payload, record, finalText }
}

/**
 * 从 stream_error 消息中取错误文案（对应原 `message.data?.error || '流式转写失败'`）。
 */
export function deriveStreamErrorMessage(message) {
  return message.data?.error || STREAM_ERROR_FALLBACK_MESSAGE
}

/**
 * onclose 时的迁移判定，等价于原实现：
 *   if (!settled && state === 'connecting') -> fail
 *   else if (!awaitingFinish && state !== 'idle') -> 意外断开收口
 *   else -> 忽略
 */
export function deriveStreamCloseAction({ settled, awaitingFinish, connectionState }) {
  if (!settled && connectionState === 'connecting') {
    return STREAM_CLOSE_ACTIONS.FAIL
  }

  if (!awaitingFinish && connectionState !== 'idle') {
    return STREAM_CLOSE_ACTIONS.UNEXPECTED
  }

  return STREAM_CLOSE_ACTIONS.IGNORE
}
