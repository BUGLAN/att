import { watch } from 'vue'
import { useAppStore } from '@/stores/app'
import { useElectron } from '@/composables/useElectron'
import { deriveTrayIconState } from '@/composables/recorder/trayState.mjs'

/**
 * 把托盘图标收敛到「录音 / 转写 / 后端连接」的派生状态上。
 *
 * 录音状态事件到达时 useRecorder 会直接切图标（开始录音→监听中、停止→转写中…），
 * 这里补的是它覆盖不到的那条边：后端断连。Vue 的 watch 回调在微任务里执行，
 * 晚于同一轮同步代码中的 setIcon 调用，所以「断开」不会被随后的 idle 顶掉。
 *
 * 应用刚启动、健康检查还没回包时 isConnected 也是 false，托盘会先显示「斷」——
 * 这恰好反映"后端尚未就绪"，与侧边栏的"未连接后端"是同一件事。
 */
export function useTrayIcon() {
  const appStore = useAppStore()
  const { tray } = useElectron()

  watch(
    () => deriveTrayIconState({
      recording: appStore.isRecording,
      transcribing: appStore.isTranscribing,
      connected: appStore.isConnected
    }),
    (state) => {
      tray.setIcon(state)
    },
    { immediate: true }
  )
}
