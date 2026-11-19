// 设置表单与「服务端设置」的同步判定（纯逻辑，便于单测）。
//
// 背景（真实踩坑）：设置页的 watch(settings) 会在任何 store 设置变更时用服务端值
// 整体重建表单，并顺手刷新「已保存快照」。如果此刻用户刚改过表单、而 500ms 防抖的
// 保存还没发出，这次重建会：
//   ① 把用户的改动冲回旧值；
//   ② 让待保存内容与快照一致，于是 handleSave 判成「无变化」直接返回。
// 表现就是「勾了开关但没生效，而且没有任何提示」。
//
// 与之配套的另一半保护在组件里：初始化完成前禁用表单，避免用户在
// isInitialized 仍为 false 时改动被 watch(form) 的守卫直接丢弃。

/**
 * 当前表单是否存在「尚未提交」的编辑。
 *
 * @param {object} params
 * @param {boolean} params.initialized 设置页是否已完成初始化（未完成时表单尚未与服务端对齐）
 * @param {string} params.draftSnapshot 当前表单的序列化结果
 * @param {string} params.savedSnapshot 最近一次提交 / 对齐后的表单序列化结果
 * @returns {boolean} true 表示有未提交改动，调用方不应再用服务端值覆盖表单
 */
export function hasPendingSettingsEdits({ initialized, draftSnapshot, savedSnapshot }) {
  // 初始化完成前表单还没与服务端对齐，快照无意义，一律允许对齐
  if (!initialized) {
    return false
  }

  // 快照为空说明还没建立基线（首次对齐中），同样允许对齐
  if (!savedSnapshot) {
    return false
  }

  return draftSnapshot !== savedSnapshot
}

/**
 * 只由服务端维护、不进提交载荷的表单字段：密钥「是否已保存」。
 *
 * 背景（真实踩坑）：保存带 Token 的设置时，store 写回服务端结果会先于 handleSave 清空明文 Token，
 * 此时表单与快照不一致，watch(settings) 按「有未提交改动」跳过重建，而这两个字段又不在快照里，
 * 之后再也不会被刷新——卡片一直显示「未配置」，直到重启应用。
 */
export const SERVER_OWNED_FIELDS = Object.freeze([
  'doubao_asr_api_key_set',
  'chatgpt_asr_api_key_set',
  'deepseek_asr_api_key_set'
])

/**
 * 从服务端设置取出 {@link SERVER_OWNED_FIELDS} 的最新值（缺字段按「未保存」处理）。
 * 有未提交改动时也要照常写回表单：它们不影响快照，不会触发多余的保存。
 *
 * @param {object} serverSettings GET / PUT /api/settings 的响应（或 store 里的 settings）
 * @returns {Record<string, boolean>}
 */
export function serverOwnedPatch(serverSettings) {
  return Object.fromEntries(SERVER_OWNED_FIELDS.map((key) => [key, Boolean(serverSettings?.[key])]))
}

/**
 * 只存在于表单草稿、不进「已保存快照」的凭据字段。
 * 保存成功后要清掉明文 / 清除标记，否则下一次保存会把同一份 Token 再提交一遍。
 */
export const TRANSIENT_CREDENTIAL_FIELDS = Object.freeze([
  'doubao_asr_api_key',
  'doubao_asr_api_key_clear',
  'chatgpt_asr_api_key',
  'chatgpt_asr_api_key_clear',
  'deepseek_asr_api_key',
  'deepseek_asr_api_key_clear'
])

/**
 * 把「这次实际提交的载荷」转成已保存快照。
 *
 * 不能保存成功后再用当前表单重算：保存期间用户继续修改时，当前表单已经包含
 * 下一笔尚未提交的改动，重算会把它们误标成「已保存」，随后补存又被判成无变化。
 *
 * @param {object} payload 本次提交给后端的设置载荷
 * @returns {string} 用于与最新表单比较的序列化快照
 */
export function serializeSavedSettingsPayload(payload) {
  const saved = { ...(payload || {}) }
  for (const field of TRANSIENT_CREDENTIAL_FIELDS) {
    delete saved[field]
  }
  return JSON.stringify(saved)
}

/**
 * 保存请求返回后清掉本次已提交的凭据草稿。
 *
 * 只清「值仍等于提交时」的字段：如果用户在慢保存期间又输入了新 Token /
 * 重新勾了清除，这些新值必须保留给补存，不能一并被清空。
 *
 * @param {object} current 当前表单值
 * @param {object} submitted 提交时的凭据字段值
 * @returns {Record<string, string | boolean>} 需要合并回表单的清理补丁
 */
export function submittedCredentialClearPatch(current, submitted) {
  const patch = {}

  for (const field of TRANSIENT_CREDENTIAL_FIELDS) {
    if (current?.[field] !== submitted?.[field]) {
      continue
    }
    patch[field] = field.endsWith('_clear') ? false : ''
  }

  return patch
}
