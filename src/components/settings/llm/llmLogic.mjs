/**
 * LLM 设置页纯逻辑（不依赖 Vue / 浏览器全局，供 tests/test_llm_logic.mjs 锁定）。
 *
 * 设计对应 docs/LLM配置模块重设计.md §7：
 * - 模型名组装规则必须与后端 app/llm/registry.compose_model_string 一致；
 * - 错误分级文案给出可执行建议（P7）；
 * - 状态徽章与延迟格式化是纯展示映射。
 */

// 与后端 app/llm/errors.py 的错误分级对齐
const ERROR_TITLES = {
  ok: '验证通过',
  auth_failed: '鉴权失败',
  model_not_found: '模型不存在',
  quota_exceeded: '配额不足',
  timeout: '请求超时',
  network_error: '网络错误',
  provider_error: '上游故障',
  unsupported_provider: '不支持的服务商',
  unknown: '验证失败'
}

const FALLBACK_HINTS = {
  auth_failed: '检查 API Key 是否复制完整、是否被撤销或过期、账户是否欠费',
  model_not_found: '检查模型名是否正确（含服务商前缀），或从模型列表重新拉取',
  quota_exceeded: '账户欠费或触发限流，请充值或稍后重试',
  timeout: '端点不可达或响应过慢，请检查网络/代理，或调大超时',
  network_error: '网络连接失败，请检查网络与代理设置',
  provider_error: '上游服务故障，请稍后重试',
  unsupported_provider: '可在档案编辑里改用「自定义 OpenAI 兼容」端点',
  unknown: '请查看日志获取详情'
}

/**
 * 模型串组装（有真实前缀的服务商）：裸名按前缀组装，含 / 的完整串原样透传。
 * 注意：custom（自定义 OpenAI 兼容）档案不要用这个函数——本地网关的模型 ID
 * 常自带斜杠（group/xxx），必须用 composeCustomModelString 整串包进 openai/。
 */
export function composeModelString(litellmPrefix, modelName) {
  const name = String(modelName || '').trim()
  if (!name) {
    return ''
  }
  if (name.includes('/')) {
    return name
  }
  const prefix = String(litellmPrefix || 'openai').trim() || 'openai'
  return prefix + '/' + name
}

/**
 * custom 档案的模型串组装（与后端 compose_model_string 的 custom 分支一致）：
 * 恒为 openai/<完整模型ID>，对已是 openai/ 开头的 ID 幂等。
 * 本地网关的模型 ID 常自带斜杠（group/deepseek-v4.1-flash），若原样透传，
 * LiteLLM 会把首段误当服务商前缀（实测报 "LLM Provider NOT provided"）。
 */
export function composeCustomModelString(modelName) {
  const name = String(modelName || '').trim()
  if (!name) {
    return ''
  }
  return 'openai/' + (name.startsWith('openai/') ? name.slice('openai/'.length) : name)
}

/** 模型名合法性：与后端 is_valid_model_string 同一字符集。 */
export function isValidModelName(name) {
  const value = String(name || '').trim()
  if (!value) {
    return false
  }
  return /^[A-Za-z0-9._:/-]+$/.test(value)
}

/**
 * 解析「手动输入模型名」：支持逗号（中英文）、分号、换行分隔，
 * 去空、去重、丢弃非法名并返回非法项。
 */
export function parseManualModelInput(raw) {
  const parts = String(raw || '').split(/[,，;；\n]+/)
  const valid = []
  const invalid = []
  for (const part of parts) {
    const name = part.trim()
    if (!name) {
      continue
    }
    if (isValidModelName(name) && !valid.includes(name)) {
      valid.push(name)
    } else if (!isValidModelName(name) && !invalid.includes(name)) {
      invalid.push(name)
    }
  }
  return { valid, invalid }
}

/** 错误分级 → 弹窗/横幅文案。message 优先（后端已脱敏并附建议），缺失时回退内置提示。 */
export function errorCopy(errorCode, message) {
  const code = ERROR_TITLES[errorCode] ? errorCode : 'unknown'
  return {
    title: ERROR_TITLES[code],
    detail: (message && String(message).trim()) || FALLBACK_HINTS[code]
  }
}

/** 验证状态 → BaseBadge tone。 */
export function statusTone(state) {
  if (state === 'ok') {
    return 'ok'
  }
  if (state === 'failed' || state === 'decrypt_failed') {
    return 'err'
  }
  return 'neutral'
}

export const STATUS_LABELS = {
  ok: '可用',
  failed: '失败',
  decrypt_failed: '密钥无法解密',
  unknown: '未验证'
}

/** 延迟展示：<1s 用毫秒，≥1s 用秒（一位小数）。 */
export function formatLatency(latencyMs) {
  const value = Number(latencyMs)
  if (!Number.isFinite(value) || value < 0) {
    return ''
  }
  if (value < 1000) {
    return Math.round(value) + 'ms'
  }
  return (value / 1000).toFixed(1) + 's'
}

/** 模型参数摘要行。 */
export function describeParams(params) {
  if (!params) {
    return ''
  }
  const maxTokens = Number(params.max_tokens ?? 0)
  const maxText = maxTokens > 0 ? maxTokens.toLocaleString('en-US') : '-'
  return [
    'temp ' + (params.temperature ?? '-'),
    'max ' + maxText,
    '超时 ' + (params.timeout_s ?? '-') + 's'
  ].join(' · ')
}

/** 密钥展示文案：有尾号提示时展示掩码，否则按是否必需给提示。 */
export function keyDisplay(hint, requiresKey) {
  if (hint) {
    return '密钥已保存（' + hint + '）'
  }
  return requiresKey ? '未设置密钥' : '无需密钥'
}
