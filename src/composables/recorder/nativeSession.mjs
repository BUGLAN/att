// 桌面端（Tauri）原生录音会话 → 界面状态的映射（纯函数，无副作用）。
//
// 背景：Tauri 版的采集、识别、记录落库、复制粘贴全部在 Rust 录音 Actor 内完成
// （src-tauri/src/recording.rs），全局快捷键也直达 Actor。前端只订阅
// `recording:state` 事件并据此刷新 appStore / 实时转写区 / 计时器。
// 这里把「事件载荷 → 界面补丁」收敛成纯查表，便于 Node 直接单测。
//
// 载荷形状：{ phase, mode, message?, text?, raw_text?, record?, copied?, pasted?, degraded?,
//           optimized?, optimization_cancelled?, optimize_error? }
// phase ∈ idle | connecting | recording | finalizing | optimizing | completed | error | cancelled
//
// 自动优化（auto_optimize）：转写落库后进入 optimizing，增量走 `recording:optimization`；
// completed 的 text 是最终粘贴的文本（优化成功为优化文，失败/取消为原文）。

import { deriveCompletionToast } from './statusCopy.mjs'

export const NATIVE_PHASES = Object.freeze([
  'idle',
  'connecting',
  'recording',
  'finalizing',
  'optimizing',
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
 * - toast / toastTone：轻提示文案与色调（error 为失败色）
 * - optimization：优化流面板动作 'begin' | 'finish' | 'fail' | 'cancel' | null
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
    case 'optimizing':
      return {
        livePhase: 'optimizing',
        statusText: message || '转写完成，正在优化',
        recording: false,
        // 单段模式优化期间保持「处理中」（新录音会被吞掉）；流式允许直接开始下一段
        transcribing: !streaming,
        clock: 'stop',
        transcript: payload.text || '',
        record: payload.record || null,
        optimization: 'begin',
        trayIcon: 'transcribing'
      }
    case 'completed': {
      const text = payload.text || ''
      const copyResult = {
        attempted: Boolean(payload.copied) || Boolean(payload.copy_error),
        copied: Boolean(payload.copied),
        pasted: Boolean(payload.pasted)
      }
      const optimization = payload.optimized
        ? 'finish'
        : payload.optimization_cancelled
          ? 'cancel'
          : payload.optimize_error
            ? 'fail'
            : null
      let toast
      let toastTone
      if (!text) {
        toast = message || '没有识别到语音内容'
      } else if (optimization === 'fail') {
        // 自动优化失败兜底粘贴原文，保证「总有输出」；已流式输入部分优化文时由后端说明去向（copy_error）。
        // 完整原因在记录详情的 error 字段
        toast = copyResult.pasted
          ? '优化失败，已粘贴转写文本'
          : (copyResult.copied && payload.copy_error) || '优化失败，转写文本已保留在记录集中'
        toastTone = 'error'
      } else if (optimization === 'cancel') {
        toast = '已取消优化，保留原始文本'
      } else if (payload.degraded) {
        toast = message
      } else {
        toast = deriveCompletionToast(
          optimization === 'finish' ? 'optimization' : streaming ? 'streaming' : 'transcription',
          copyResult
        )
      }
      return {
        livePhase: 'completed',
        statusText: text ? (message || '录音已完成') : toast,
        recording: false,
        transcribing: false,
        clock: 'stop',
        transcript: text,
        record: payload.record || null,
        toast,
        toastTone,
        optimization,
        optimizeError: payload.optimize_error || '',
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
