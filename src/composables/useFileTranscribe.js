/**
 * 「转文字」页编排：选文件 → POST /api/transcribe →（202 时）轮询任务 → 归一结果。
 *
 * 纯逻辑全部在 transcribe/fileTranscribeModel.mjs（有 Node 用例），这里只做
 * Vue 状态与副作用的粘合；HTTP 走 api/client，桌面对话框走 useElectron。
 */
import { ref, shallowRef } from 'vue'
import { api } from '@/api/client'
import { useElectron } from '@/composables/useElectron'
import {
  buildTranscribeRequest,
  isAsyncJobAccepted,
  isJobPollExpired,
  isSupportedMediaPath,
  nextJobPollDelay,
  normalizeTranscribeResult
} from '@/composables/transcribe/fileTranscribeModel.mjs'

const HISTORY_LIMIT = 20

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

export function useFileTranscribe() {
  const { openMediaFile, filePathForFile, clipboard, isDesktop } = useElectron()

  const filePath = ref('')
  const language = ref('zh')
  const wantSegments = ref(false)
  // M2：引擎与 LLM 校对（设计 §7：UI 不暴露 auto，默认显式 whisper；校对默认关）
  // 桌面端（Tauri）暂无本地 whisper/qwen 引擎，缺省用豆包（未配凭据时卡片置灰并提示）
  const engine = ref(isDesktop ? 'doubao_asr' : 'whisper')
  const proofreadOn = ref(false)
  const proofreadModelId = ref('')
  const proofreadBatchSegments = ref(40)
  const proofreadContextChars = ref(200)
  const llmModels = shallowRef([])        // [{id, label}]：LLM v2 模型条目（校对面板下拉）
  const isBusy = ref(false)
  const statusText = ref('')
  const stage = ref('')                   // asr | proofreading（202 阶段展示）
  const progress = shallowRef(null)       // {window_done, window_total}
  const error = ref('')
  const result = shallowRef(null)
  const history = shallowRef([])

  /** 懒加载 LLM v2 模型条目（校对面板第一次展开时拉一次）。 */
  async function loadLlmModels() {
    if (llmModels.value.length) {
      return llmModels.value
    }
    try {
      const state = await api.getLLMState()
      llmModels.value = (state?.models || []).map((item) => ({
        id: item.id,
        label: item.label || item.model || item.id
      }))
    } catch (err) {
      // 拉不到就让面板空着，开启校对时后端会给 409 的可读提示
      llmModels.value = []
    }
    return llmModels.value
  }

  // 每次 startTranscribe 自增；轮询循环发现 runId 变化即自行退出，防止旧轮询污染新请求
  let runSeq = 0

  async function chooseFile() {
    error.value = ''
    const picked = await openMediaFile()
    if (!picked) {
      // 浏览器环境（vite dev 无 electronAPI）或用户取消：取消不算错误，保持安静
      return null
    }
    if (!isSupportedMediaPath(picked)) {
      error.value = '暂不支持该文件类型：请选择音视频文件（mp4 / flv / webm / wav / mp3 / m4a 等）'
      return null
    }
    filePath.value = picked
    return picked
  }

  /**
   * 拖拽入口：File 对象 → 本地路径校验 → 直接开始转写。
   * 拿不到路径（浏览器环境）或类型不支持时只置 error，不发起请求。
   */
  async function ingestDroppedFile(file) {
    if (!file) {
      return false
    }

    return ingestDroppedPath(filePathForFile(file))
  }

  /** 桌面端原生拖放直接给出本地路径。 */
  async function ingestDroppedPath(path) {
    if (!path) {
      error.value = '拿不到拖入文件的本地路径：请在 App 内拖入，或改用粘贴路径'
      return false
    }
    if (!isSupportedMediaPath(path)) {
      error.value = '暂不支持该文件类型：请选择音视频文件（mp4 / flv / webm / wav / mp3 / m4a 等）'
      return false
    }

    error.value = ''
    filePath.value = path
    await startTranscribe()
    return true
  }

  function pushHistory(entry) {
    history.value = [entry, ...history.value.filter((item) => item.path !== entry.path)]
      .slice(0, HISTORY_LIMIT)
  }

  async function pollJobResult(jobId, currentRun) {
    const startedAt = Date.now()
    let attempt = 0

    while (true) {
      attempt += 1
      await sleep(nextJobPollDelay(attempt))
      if (currentRun !== runSeq) {
        return null // 已被更新的转写请求接管
      }

      const job = await api.getTranscribeJob(jobId)

      // 阶段与窗口进度（M2：asr → proofreading x/y）
      stage.value = String(job.stage || '')
      progress.value = job.progress || null
      if (stage.value === 'proofreading' && progress.value) {
        statusText.value = 'LLM 校对中 · 窗口 ' + progress.value.window_done + '/' + progress.value.window_total
      } else {
        statusText.value = '转写进行中，已等待 ' + Math.round((Date.now() - startedAt) / 1000) + ' 秒…'
      }

      if (job.status === 'done') {
        return normalizeTranscribeResult(job.result, { path: filePath.value })
      }
      if (job.status === 'failed') {
        throw new Error(job.error || '转写任务失败')
      }
      if (isJobPollExpired(Date.now() - startedAt)) {
        throw new Error('转写任务超过 15 分钟未完成，已停止等待；任务仍会在后台继续，可稍后重试同一文件（会命中缓存）')
      }
    }
  }

  async function startTranscribe() {
    const request = buildTranscribeRequest({
      path: filePath.value,
      language: language.value,
      wantSegments: wantSegments.value,
      engine: engine.value,
      proofread: proofreadOn.value,
      proofreadModelId: proofreadModelId.value || null,
      proofreadBatchSegments: proofreadBatchSegments.value,
      proofreadContextChars: proofreadContextChars.value
    })

    if (!request) {
      error.value = '请先选择或粘贴一个本地文件路径'
      return
    }
    if (!isSupportedMediaPath(request.path)) {
      error.value = '暂不支持该文件类型：请选择音视频文件（mp4 / flv / webm / wav / mp3 / m4a 等）'
      return
    }

    const currentRun = ++runSeq
    isBusy.value = true
    error.value = ''
    result.value = null
    stage.value = ''
    progress.value = null
    statusText.value = '正在提交转写…'

    try {
      const payload = await api.transcribeFile(request)
      if (currentRun !== runSeq) {
        return
      }

      if (isAsyncJobAccepted(payload)) {
        statusText.value = proofreadOn.value ? '已转入后台任务（ASR + LLM 校对）…' : '文件较长，已转入后台任务…'
        const polled = await pollJobResult(payload.job_id, currentRun)
        if (polled == null) {
          return
        }
        result.value = polled
        pushHistory(polled)
        return
      }

      const normalized = normalizeTranscribeResult(payload, { path: request.path })
      if (normalized == null) {
        throw new Error('后端响应格式异常：缺少转写文本')
      }
      result.value = normalized
      pushHistory(normalized)
    } catch (err) {
      if (currentRun === runSeq) {
        error.value = err?.message || '转写失败，请稍后重试'
      }
    } finally {
      if (currentRun === runSeq) {
        isBusy.value = false
        statusText.value = ''
        stage.value = ''
        progress.value = null
      }
    }
  }

  async function copyResult() {
    const text = result.value?.text || ''
    if (!text) {
      return false
    }
    await clipboard.writeText(text)
    return true
  }

  function reset() {
    runSeq += 1 // 使进行中的轮询失效
    filePath.value = ''
    isBusy.value = false
    statusText.value = ''
    error.value = ''
    result.value = null
  }

  return {
    filePath,
    language,
    wantSegments,
    engine,
    proofreadOn,
    proofreadModelId,
    proofreadBatchSegments,
    proofreadContextChars,
    llmModels,
    loadLlmModels,
    isBusy,
    statusText,
    stage,
    progress,
    error,
    result,
    history,
    chooseFile,
    ingestDroppedFile,
    ingestDroppedPath,
    startTranscribe,
    copyResult,
    selectHistoryItem: (item) => {
      result.value = item
      filePath.value = item.path
    },
    reset
  }
}
