// 设置 → 诊断 →「外部 API」纯逻辑：令牌显示。
// 与 Vue 无关，便于 tests/test_settings_api_access.mjs 锁定行为。

/** 令牌显示：未开启 / 正在生成 / 已脱敏 / 用户点了「显示」后的明文。 */
export function apiTokenDisplay({ enabled, hasToken, revealedToken }) {
  if (!enabled) {
    return '未开启'
  }
  if (!hasToken) {
    return '正在生成…'
  }
  return revealedToken || 'att_••••••••••••••••'
}
