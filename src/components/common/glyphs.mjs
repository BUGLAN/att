// 主窗口的「文字图标」表（不依赖 Vue，供 tests/test_page_glyphs.mjs 锁定）。
//
// 图标统一用宋体粗体繁体字，与悬浮窗的印章是同一套语言：
// - 录音按钮就是一枚印章，四态与悬浮窗同源（SEALS：錄 待命 · 聽 录音中 · 轉 处理中 · 成 完成 · 敗 失败）；
//   托盘的录音 / 转写也用聽 / 轉，待命则是品牌标「墨」（见 scripts/generate_brand_icons.py）；
// - 导航、操作按钮、剪贴板类型都用单字，操作按钮的全称放在 title / aria-label 里。
// 改字只改这里，界面各处跟着变。

import { SEALS } from '../floating/floatingView.mjs'

export { SEALS }

/** 侧边栏导航 */
export const NAV_GLYPHS = Object.freeze({
  home: '錄',
  transcribe: '文',
  records: '集',
  clipboard: '貼',
  settings: '設'
})

/** 操作按钮 */
export const ACTION_GLYPHS = Object.freeze({
  play: '播',
  pause: '停',
  copy: '複',
  favorite: '藏',
  pin: '頂',
  delete: '刪',
  optimize: '潤',
  cancel: '止',
  edit: '改',
  export: '出',
  search: '尋',
  select: '選',
  refresh: '新',
  import: '入',
  file: '檔',
  settings: '設',
  // 看图器：放大 / 缩小 / 原始尺寸 / 适应窗口 / 关闭
  zoomIn: '放',
  zoomOut: '縮',
  actualSize: '原',
  fit: '適',
  close: '關'
})

/** 剪贴板条目类型（code 在界面上叫「长文本」） */
export const CLIP_TYPE_GLYPHS = Object.freeze({
  text: '文',
  code: '長',
  image: '圖',
  link: '鏈',
  file: '檔'
})

export function clipTypeGlyph(type) {
  return CLIP_TYPE_GLYPHS[type] || CLIP_TYPE_GLYPHS.text
}

/**
 * 首页录音印章的状态：录音中 > 处理中（转写 / 优化）> 刚结束的成 / 敗（短暂停留）> 待命。
 * flash 由调用方在一次会话结束时置为 'success' / 'failure'，停留 SUCCESS_HOLD_MS / FAILURE_HOLD_MS 后清空。
 */
export function recorderSealState({ recording = false, processing = false, flash = null } = {}) {
  if (recording) return 'listening'
  if (processing) return 'converting'
  if (flash === 'success' || flash === 'failure') return flash
  return 'idle'
}
