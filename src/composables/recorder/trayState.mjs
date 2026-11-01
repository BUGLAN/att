// 托盘图标状态推导（纯函数，无副作用，可被 Node 直接单测）。
//
// 背景：托盘图标原来只按「录音 / 转写」两个标志位切换，后端断连时图标仍停在
// "就绪"，从托盘上完全看不出服务已经掉了。这里把三个输入收敛成一个派生状态：
//
//   录音 > 转写 > 后端未连接 > 就绪
//
// 录音排在最前是有意的：正在监听时哪怕后端刚断，图标也该先表示"在听"，
// 等录音结束、仍是未连接状态时才会落到「斷」。
//
// 状态字符串必须与托盘图标文件名一一对应
// （见 src-tauri/icons/tray/ 下的 win-*.png / macos-*.png）。

export const TRAY_ICON_STATES = Object.freeze([
  'idle',
  'recording',
  'transcribing',
  'disconnected'
])

/**
 * 推导当前该显示哪个托盘图标。
 *
 * connected 只认显式 false：undefined / null 视为"未知"，按就绪处理，
 * 避免以后接入新的连接状态来源时误报「斷」。
 *
 * @param {{ recording?: boolean, transcribing?: boolean, connected?: boolean }} state
 * @returns {'idle' | 'recording' | 'transcribing' | 'disconnected'}
 */
export function deriveTrayIconState({ recording = false, transcribing = false, connected = true } = {}) {
  if (recording) {
    return 'recording'
  }

  if (transcribing) {
    return 'transcribing'
  }

  if (connected === false) {
    return 'disconnected'
  }

  return 'idle'
}
