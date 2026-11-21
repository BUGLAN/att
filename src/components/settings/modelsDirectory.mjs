// 本地模型目录设置页的展示状态（纯逻辑，便于 Node 测试）。

function normalizePath(value) {
  return String(value || '').trim()
}

/**
 * @param {object} input
 * @param {string} input.savedDir 已持久化的目录
 * @param {string} input.draftDir 表单当前目录
 * @param {string} input.effectiveDir 当前进程实际使用的目录
 * @param {string} input.nextDir 下次启动将使用的目录
 * @param {boolean} input.envOverride 是否由 ATT_MODELS_DIR 锁定
 * @param {boolean} input.desktop 是否在桌面壳中
 * @param {boolean} input.picking 是否正在打开目录选择器
 * @param {boolean} input.restarting 是否正在重启
 */
export function modelsDirectoryView({
  savedDir = '',
  draftDir = '',
  effectiveDir = '',
  nextDir = '',
  envOverride = false,
  desktop = false,
  picking = false,
  restarting = false
} = {}) {
  const saved = normalizePath(savedDir)
  const draft = normalizePath(draftDir)
  const effective = normalizePath(effectiveDir)
  const next = normalizePath(nextDir) || effective
  const pendingSave = draft !== saved

  if (envOverride) {
    return {
      path: effective || next || '',
      message: '已由环境变量 ATT_MODELS_DIR 指定',
      canChange: false,
      canReset: false,
      pendingSave: false,
      restartPending: false,
      canRestart: false
    }
  }

  const target = pendingSave ? (draft || next || effective) : (next || draft || effective)
  const restartPending = !pendingSave && Boolean(effective) && Boolean(target) && effective !== target

  let message = ''
  if (pendingSave) {
    message = '正在保存模型目录…'
  } else if (restartPending) {
    message = desktop
      ? '已保存，重启应用后生效。旧模型不会自动搬移，可手动移动或在新目录重新下载'
      : '目录已更新，重启桌面应用后生效。旧模型不会自动搬移'
  } else if (!desktop) {
    message = '可在桌面版中选择其他目录'
  }

  return {
    path: target || '',
    message,
    canChange: desktop,
    canReset: desktop && Boolean(saved),
    pendingSave,
    restartPending,
    canRestart: desktop && restartPending && !picking && !restarting
  }
}
