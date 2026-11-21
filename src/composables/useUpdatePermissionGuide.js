import { ref } from 'vue'
import { useAppStore } from '@/stores/app'
import { useElectron } from './useElectron'
import { restoredMessage, stillMissing, updatePermissionGuide } from '@/components/settings/permissionGuide.mjs'

// 当前这次启动的引导内容（App 启动时检查一次，弹窗读同一份）
const guide = ref({ show: false, versionText: '', items: [] })

/**
 * macOS 更新后首次启动的权限引导：启动时问后端要不要提示，要就弹一次；
 * 重新检测都恢复了就关掉，关掉后本次启动内不再提示。
 */
export function useUpdatePermissionGuide() {
  const appStore = useAppStore()
  const { getUpdatePermissionNotice, dismissUpdatePermissionNotice, getPermissionStatus } = useElectron()

  async function checkOnStartup() {
    try {
      const next = updatePermissionGuide(await getUpdatePermissionNotice())
      if (next.show) {
        guide.value = next
        appStore.openModal('updatePermissionGuide')
      }
    } catch (error) {
      console.warn('[Permissions] 读取更新后的权限引导失败：', error)
    }
  }

  /** 按最新权限状态重新检测；都恢复了就关掉引导并提示，返回是否都已恢复 */
  async function recheck() {
    const status = await getPermissionStatus().catch(() => null)
    const missing = stillMissing(guide.value.items, status?.items)
    if (missing.length) {
      guide.value = { ...guide.value, items: missing }
      return false
    }
    const message = restoredMessage(guide.value.items)
    await dismiss()
    appStore.showToast(message, { tone: 'success', duration: 6000 })
    return true
  }

  /** 关掉引导（close=false：弹窗已经在关，只通知后端本次启动内不再提示） */
  async function dismiss({ close = true } = {}) {
    if (close) {
      appStore.closeModal('updatePermissionGuide')
    }
    await dismissUpdatePermissionNotice().catch(() => {})
  }

  return { guide, checkOnStartup, recheck, dismiss }
}
