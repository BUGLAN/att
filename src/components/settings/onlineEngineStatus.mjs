// 识别引擎可用性状态的纯逻辑（不依赖 Vue，供 tests/test_settings_online_engine.mjs 锁定）。
//
// 「填了 Token」不等于「能用」：保存后后端会做一次真实握手（见 att_core::stt::verify），
// 卡片据此区分「已配置 / 验证中 / 可用 / 验证失败 / 未能验证」。
// 豆包网页语音匿名访问、无需密钥；微信输入法语音只做本机探活，两者都不参与凭据验证。

export const WETYPE_PROVIDER = 'wetype_asr'
export const ONLINE_PROVIDERS = Object.freeze([
  'doubao_asr',
  'doubao_web_asr',
  'doubao_ime_asr',
  'chatgpt_asr',
  'deepseek_asr',
  WETYPE_PROVIDER
])

/** 需要用户填写凭据并做后端握手的在线引擎。 */
export const CREDENTIAL_PROVIDERS = Object.freeze(['doubao_asr', 'chatgpt_asr', 'deepseek_asr'])

export function isOnlineProvider(provider) {
  return ONLINE_PROVIDERS.includes(provider)
}

/** 是否需要配置并验证凭据；豆包网页语音无需密钥。 */
export function requiresCredential(provider) {
  return CREDENTIAL_PROVIDERS.includes(provider)
}

export function isWetypeProvider(provider) {
  return provider === WETYPE_PROVIDER
}

/**
 * 卡片上的状态点。
 *
 * @param {object} params
 * @param {boolean} params.keySet 是否已保存凭据
 * @param {{ status: 'checking'|'ok'|'invalid'|'unknown', message?: string } | null} [params.check]
 *   该引擎当前凭据的验证结论；没验过为 null
 * @param {boolean} [params.requiresCredentials] 是否需要凭据；豆包网页语音传 false
 * @returns {{ chip: string, tone: 'ok'|'warn'|'err'|'accent'|'neutral', loading: boolean }}
 */
export function onlineEngineChip({ keySet, check = null, requiresCredentials = true }) {
  if (!requiresCredentials) {
    return { chip: '在线 · 无需密钥', tone: 'ok', loading: false }
  }
  if (!keySet) {
    return { chip: '在线 · 未配置', tone: 'warn', loading: false }
  }
  switch (check?.status) {
    case 'checking':
      return { chip: '在线 · 验证中', tone: 'accent', loading: true }
    case 'ok':
      return { chip: '在线 · 可用', tone: 'ok', loading: false }
    case 'invalid':
      return { chip: '在线 · 验证失败', tone: 'err', loading: false }
    case 'unknown':
      return { chip: '在线 · 未能验证', tone: 'warn', loading: false }
    default:
      return { chip: '在线 · 已配置', tone: 'ok', loading: false }
  }
}

/**
 * 接入配置区里的提示条：只在验证失败 / 没验成时出现。
 * 返回 { tone: 'err'|'warn', text } 或 null。
 */
export function credentialNote(check) {
  if (check?.status === 'invalid') {
    return { tone: 'err', text: check.message || '凭据验证失败，请检查后重新填写。' }
  }
  if (check?.status === 'unknown') {
    const reason = check.message ? `${check.message}。` : ''
    return { tone: 'warn', text: `${reason}暂时没能验证，录音时仍会照常尝试；再点一下上方的引擎卡片可重新验证。` }
  }
  return null
}

/**
 * 微信输入法语音卡片状态。非 Windows 明确不可用；Windows 上按进程/attach 检测结果展示。
 * @param {{ platform?: string, check?: { status: string, message?: string } | null }} params
 */
export function wetypeEngineChip({ platform = 'win32', check = null } = {}) {
  if (platform !== 'win32') {
    return { chip: '仅 Windows', tone: 'neutral', loading: false }
  }
  switch (check?.status) {
    case 'checking':
      return { chip: '检测中', tone: 'accent', loading: true }
    case 'ok':
      return { chip: '可用', tone: 'ok', loading: false }
    case 'invalid':
      return { chip: '未就绪', tone: 'warn', loading: false }
    case 'unknown':
      return { chip: '未能检测', tone: 'warn', loading: false }
    default:
      return { chip: '本机注入', tone: 'neutral', loading: false }
  }
}

/**
 * 保存设置后是否需要重新验证当前在线引擎：换了引擎、填了新 Token，或改了 Resource ID / 接入点。
 *
 * @param {object} payload 本次提交的设置载荷（明文 Token 只在用户输入时出现）
 * @param {object} previous 上次已保存的服务端设置
 */
export function needsCredentialCheck(payload, previous = {}) {
  const provider = payload?.asr_provider
  if (!isOnlineProvider(provider)) {
    return false
  }
  if (provider === WETYPE_PROVIDER) {
    if (provider !== previous?.asr_provider) {
      return true
    }
    return payload.wetype_asr_pid !== previous?.wetype_asr_pid ||
      payload.wetype_asr_timeout_seconds !== previous?.wetype_asr_timeout_seconds
  }
  if (!requiresCredential(provider)) {
    return false
  }
  if (provider !== previous?.asr_provider) {
    return true
  }
  if (provider === 'doubao_asr') {
    return Boolean(payload.doubao_asr_api_key) ||
      payload.doubao_asr_resource_id !== previous?.doubao_asr_resource_id ||
      payload.doubao_asr_endpoint !== previous?.doubao_asr_endpoint
  }
  if (provider === 'deepseek_asr') {
    return Boolean(payload.deepseek_asr_api_key) ||
      payload.deepseek_asr_endpoint !== previous?.deepseek_asr_endpoint
  }
  return Boolean(payload.chatgpt_asr_api_key) || payload.chatgpt_asr_endpoint !== previous?.chatgpt_asr_endpoint
}
