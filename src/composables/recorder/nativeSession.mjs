// 桌面端（Tauri）原生录音会话 → 界面状态的映射（纯函数，无副作用）。
//
// 背景：Tauri 版的采集、识别、记录落库、复制粘贴全部在 Rust 录音 Actor 内完成
// （src-tauri/src/recording.rs），全局快捷键也直达 Actor。前端只订阅
// `recording:state` 事件并据此刷新 appStore / 实时转写区 / 计时器。
// 这里把「事件载荷 → 界面补丁」收敛成纯查表，便于 Node 直接单测。
//
// 载荷形状：{ phase, mode, message?, text?, record?, copied?, pasted?, degraded? }
// phase ∈ idle | connecting | recording | finalizing | completed | error | cancelled

import { deriveCompletionToast } from './statusCopy.mjs'

export const NATIVE_PHASES = Object.freeze([
  'idle',
  'connecting',
  'recording',
  'finalizing',
  'completed',
  'error',
  'cancelled'
])

/**
 * 计算一次状态事件对界面的影响。返回 null 表示忽略（未知相位）。
 *
 * 字段：
 * - livePhase / statusText：实时转写区相位与文案
 * - recording / transcribing：appStore 两个互斥标志
 * - clock：'start' | 'stop' | null（录音计时器）
 * - resetTranscript：是否清空上一轮实时文本
 * - transcript：最终文本（completed 时覆盖实时文本）
 * - record：新建记录（需同步进记录列表）
 * - toast：轻提示文案
 * - trayIcon：托盘图标状态
 */
export function deriveNativeSessionPatch(payload = {}) {
  const phase = payload.phase
  const streaming = payload.mode === 'streaming'
  const message = payload.message || ''

  switch (phase) {
    case 'connecting':
      return {
        livePhase: 'connecting',
        statusText: message || '正在连接豆包语音服务',
        recording: true,
        transcribing: false,
        clock: 'start',
        resetTranscript: true,
        trayIcon: 'recording'
      }
    case 'recording':
      return {
        livePhase: streaming ? 'streaming' : 'recording',
        statusText: message || (streaming ? '豆包语音转写中' : '正在录音'),
        recording: true,
        transcribing: false,
        // 流式在 connecting 时已起表；单段直接进 recording 时起表
        clock: streaming ? null : 'start',
        resetTranscript: !streaming,
        trayIcon: 'recording'
      }
    case 'finalizing':
      return {
        livePhase: 'finalizing',
        statusText: message || '正在完成转写',
        recording: false,
        transcribing: true,
        clock: 'stop',
        trayIcon: 'transcribing'
      }
    case 'completed': {
      const text = payload.text || ''
      const toast = text
        ? (payload.degraded
            ? message
            : deriveCompletionToast(streaming ? 'streaming' : 'transcription', {
                attempted: Boolean(payload.copied) || Boolean(payload.copy_error),
                copied: Boolean(payload.copied),
                pasted: Boolean(payload.pasted)
              }))
        : (message || '没有识别到语音内容')
      return {
        livePhase: 'completed',
        statusText: text ? (message || '录音已完成') : toast,
        recording: false,
        transcribing: false,
        clock: 'stop',
        transcript: text,
        record: payload.record || null,
        toast,
        trayIcon: 'idle'
      }
    }
    case 'error':
      return {
        livePhase: 'error',
        statusText: message || '录音失败',
        recording: false,
        transcribing: false,
        clock: 'stop',
        record: payload.record || null,
        toast: message || '录音失败',
        trayIcon: 'idle'
      }
    case 'cancelled':
      return {
        livePhase: 'idle',
        statusText: message || '已取消本次录音',
        recording: false,
        transcribing: false,
        clock: 'stop',
        toast: message || '已取消本次录音',
        trayIcon: 'idle'
      }
    case 'idle':
      return {
        livePhase: 'idle',
        statusText: '',
        recording: false,
        transcribing: false,
        clock: 'stop',
        trayIcon: 'idle'
      }
    default:
      return null
  }
}
