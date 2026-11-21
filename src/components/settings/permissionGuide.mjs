// macOS 更新后权限失效的引导：文案与判定（不依赖 Vue，供 tests/test_permission_guide.mjs 锁定）。
//
// 原因：落墨目前用临时（ad-hoc）签名发布，「输入监控」「辅助功能」的授权跟着代码签名走，
// 应用内更新后旧授权不再生效，系统设置里的旧条目却仍显示开启——要删掉旧条目再重新添加。
// 后端：get_update_permission_notice → { show, previous_version, current_version, items }（更新后首次启动才有 items）。

/** 为什么会失效（引导弹窗与权限页共用） */
export const UPDATE_PERMISSION_REASON =
  '落墨目前用临时签名发布，macOS 的「输入监控」「辅助功能」授权跟着签名走：应用更新后旧授权不再生效，系统设置里的旧条目却仍显示开启。'

/** 怎么恢复（两项都一样） */
export const UPDATE_PERMISSION_STEPS = Object.freeze([
  '点「打开系统设置」，在列表里选中「落墨」，点下方的「−」删掉旧条目（只关掉开关不够）',
  '点「+」，从「应用程序」里重新添加「落墨」并打开开关',
  '回到落墨点「重新检测」；输入监控要退出并重新打开落墨才生效'
])

/** 各项没生效时的影响 */
const IMPACTS = Object.freeze({
  input_monitoring: '全局快捷键（录音、优化选中文本、剪贴板悬浮框）不响应',
  accessibility: '自动粘贴、优化选中文本无法把文字输出到其他应用'
})

/**
 * 后端载荷 → 弹窗模型：{ show, versionText, items: [{ id, label, impact }] }。
 * 没有要处理的项（不是更新后首次启动、权限都正常）时 show 为 false。
 */
export function updatePermissionGuide(notice) {
  const items = (Array.isArray(notice?.items) ? notice.items : [])
    .filter((item) => item?.id && item.state !== 'granted' && item.state !== 'not_required')
    .map((item) => ({ id: item.id, label: item.label || item.id, impact: IMPACTS[item.id] || '' }))
  const previous = String(notice?.previous_version || '').trim()
  const current = String(notice?.current_version || '').trim()
  const versionText = current
    ? (previous ? `落墨已从 ${previous} 更新到 ${current}。` : `落墨已更新到 ${current}。`)
    : '落墨刚完成更新。'
  return { show: Boolean(notice?.show) && items.length > 0, versionText, items }
}

/**
 * 重新检测：引导里的各项按最新权限状态过滤，返回仍没生效的项。
 * statusItems：get_permission_status 的 items（[{ id, state }]）；查不到状态的项按仍未生效处理。
 */
export function stillMissing(guideItems = [], statusItems = []) {
  const states = new Map((Array.isArray(statusItems) ? statusItems : []).map((item) => [item?.id, item?.state]))
  return (Array.isArray(guideItems) ? guideItems : []).filter((item) => states.get(item.id) !== 'granted')
}

/** 都恢复后的提示：输入监控要重启应用才真正生效 */
export function restoredMessage(guideItems = []) {
  return guideItems.some((item) => item.id === 'input_monitoring')
    ? '权限已恢复：退出并重新打开落墨后快捷键才会生效'
    : '权限已恢复'
}
