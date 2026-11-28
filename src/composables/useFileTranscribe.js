/**
 * 「转文字」页编排：选文件（可多选 / 一次拖入多个）→ 排队 → 逐个 POST /api/transcribe（async_job）
 * → 轮询任务进度 → 归一结果；转写中可以取消，单个失败或取消不影响后面排队的文件。
 *
 * 转写结果由后端存进记录集（`source_file`）；「最近转写」从记录集读，重启后仍在。
 * 导出 TXT / SRT / VTT 由 transcribe/subtitleExport.mjs 生成、Blob 下载。
 *
 * 纯逻辑全部在 transcribe/fileTranscribeModel.mjs（有 Node 用例），这里只做
 * Vue 状态与副作用的粘合；HTTP 走 api/client，桌面对话框走 useElectron。
 */
import { ref, shallowRef, watch } from 'vue'
import { api } from '@/api/client'
import { useElectron } from '@/composables/useElectron'
import { useSettingsStore } from '@/stores/settings'
import {
  buildTranscribeRequest,
  clearFinishedQueueItems,
  defaultTranscribeEngine,
  describeJobProgress,
  enqueuePaths,
  historyFromRecords,
  TRANSCRIBE_HISTORY_LIMIT,
  isAsyncJobAccepted,
  isSupportedMediaPath,
  nextQueuedItem,
  normalizeTranscribeResult,
  pollTranscribeJob,
  removeQueueItem,
  submitTranscribeRequest,
  updateQueueItem
} from '@/composables/transcribe/fileTranscribeModel.mjs'
import { buildTranscribeExport } from '@/composables/transcribe/subtitleExport.mjs'
import { normalizeLanguageForProvider } from '@/components/settings/asrLanguages.mjs'
import { downloadText } from '@/utils/exportDownload.mjs'
import { createLatestRequestGuard, runLatest } from '@/utils/latestRequest.mjs'
const UNSUPPORTED_MESSAGE = '暂不支持该文件类型：请选择音视频文件（mp4 / flv / webm / wav / mp3 / m4a 等）'

export function useFileTranscribe() {
  const { openMediaFiles, allowTranscribePath, isDesktop, clipboard } = useElectron()
  const settingsStore = useSettingsStore()

  const filePath = ref('')
  // 引擎默认跟随设置里的识别引擎（文件转写不支持的退到豆包）；用户在本页手动选过就不再跟随。
  // Qwen / DeepSeek 只能自动判定，初始语言也按引擎归一；校对默认关。
  const initialEngine = defaultTranscribeEngine(settingsStore.settings.asr_provider)
  const language = ref(normalizeLanguageForProvider(initialEngine, 'zh').language)
  const wantSegments = ref(false)
  const engine = ref(initialEngine)
  let enginePicked = false
  const proofreadOn = ref(false)
  const proofreadModelId = ref('')
  const proofreadBatchSegments = ref(40)
  const proofreadContextChars = ref(200)
  const llmModels = shallowRef([])        // [{id, label}]：LLM v2 模型条目（校对面板下拉）
  const isBusy = ref(false)
  const statusText = ref('')
  const progressInfo = shallowRef({ percent: null, label: '' }) // 进度条与文案
  const cancelRequested = ref(false)
  const queue = shallowRef([])            // [{id, path, fileName, status, error}]
  const error = ref('')
  const result = shallowRef(null)
  const history = shallowRef([])
  const historyRequest = createLatestRequestGuard()

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

  let queueRunning = false
  let currentJobId = ''
  // 轮询等待可被「取消」提前唤醒
  let wakePoll = null

  function pollDelay(ms) {
    return new Promise((resolve) => {
      const timer = setTimeout(done, ms)
      function done() {
        clearTimeout(timer)
        wakePoll = null
        resolve()
      }
      wakePoll = done
    })
  }

  // 设置是异步加载的，之后用户也可能去设置页换引擎：没手动选过就一直跟随
  watch(() => settingsStore.settings.asr_provider, (provider) => {
    if (!enginePicked) {
      const next = defaultTranscribeEngine(provider)
      engine.value = next
      const adjusted = normalizeLanguageForProvider(next, language.value)
      if (adjusted.changed) {
        language.value = adjusted.language
      }
    }
  })

  // 换引擎或换文件后，上一次的报错已经不适用，立即清掉。
  // 同步触发：startTranscribe 在同一轮里先改路径再写新错误时，不能被延后的清理盖掉
  watch([engine, filePath], () => {
    error.value = ''
  }, { flush: 'sync' })

  /** 用户在下拉里手动选引擎：之后不再跟随设置。 */
  function selectEngine(value) {
    enginePicked = true
    engine.value = value
  }

  /**
   * 把一批路径排进队列并开始（已在跑就接在后面）。
   * 返回 { added, unsupported, duplicate }；全部不支持时置 error。
   */
  function enqueue(paths) {
    const outcome = enqueuePaths(queue.value, paths)
    queue.value = outcome.queue
    if (!outcome.added.length) {
      if (outcome.unsupported.length) {
        error.value = UNSUPPORTED_MESSAGE
      }
      return outcome
    }
    if (!isBusy.value) {
      error.value = ''
    }
    void runQueue()
    return outcome
  }

  /**
   * 「选择文件」：单选时只填进路径框（用户调好选项再点开始）；多选或转写中直接排队。
   * 返回排进队列 / 填进路径框的路径列表。
   */
  async function chooseFile() {
    if (!isBusy.value) {
      error.value = ''
    }
    const picked = (await openMediaFiles()) || []
    if (!picked.length) {
      // 浏览器环境（vite dev 无桌面壳）或用户取消：取消不算错误，保持安静
      return []
    }
    if (picked.length === 1 && !isBusy.value) {
      if (!isSupportedMediaPath(picked[0])) {
        error.value = UNSUPPORTED_MESSAGE
        return []
      }
      filePath.value = picked[0]
      return picked
    }
    return enqueue(picked).added.map((item) => item.path)
  }

  /** 桌面端原生拖放直接给出本地路径：按拖入顺序排队，立即开始。 */
  async function ingestDroppedPaths(paths) {
    const list = (paths || []).filter(Boolean)
    if (!list.length) {
      error.value = '拿不到拖入文件的本地路径：请在 App 内拖入，或改用粘贴路径'
      return false
    }
    return enqueue(list).added.length > 0
  }

  // 本次运行转出来的完整结果（带分段与路径），按记录 id 存：回看历史时优先用它，字幕才能导出
  const sessionResults = new Map()

  function withSessionResult(item) {
    return (item.recordId && sessionResults.get(item.recordId)) || item
  }

  /** 从记录集读最近的文件转写（source=file）；读不到时保留现有列表。 */
  async function loadHistory() {
    await runLatest(
      historyRequest,
      () => api.searchRecords({ source: 'file', page: 1, page_size: TRANSCRIBE_HISTORY_LIMIT }),
      {
        onResult: (page) => {
          history.value = historyFromRecords(page?.items).map(withSessionResult)
        }
        // 数据面暂时不可用：历史保持原样，不打扰正在进行或刚完成的转写
      }
    )
  }

  function pushHistory(entry) {
    if (entry.recordId) {
      sessionResults.set(entry.recordId, entry)
    }
    // 先放进列表立刻可见，再以记录集为准刷新（存记录失败时也能在本次运行里回看）
    const key = (item) => item.recordId || item.path
    history.value = [entry, ...history.value.filter((item) => key(item) !== key(entry))]
      .slice(0, TRANSCRIBE_HISTORY_LIMIT)
    if (entry.recordId) {
      void loadHistory()
    }
  }

  /** 导出当前结果：TXT / SRT / VTT；返回 { filename } 或 { error }（字幕缺分段等）。 */
  function exportResult(format) {
    const built = buildTranscribeExport(result.value, format)
    if (built.error) {
      return { error: built.error }
    }
    downloadText(built.content, built.filename, built.mime)
    return { filename: built.filename }
  }

  void loadHistory()

  function applyJobState(job) {
    progressInfo.value = describeJobProgress(job, { cancelRequested: cancelRequested.value })
    statusText.value = progressInfo.value.label
  }

  /** 轮询到任务结束：{ status: 'done', result } | { status: 'cancelled' }；失败抛错。 */
  async function pollJobResult(jobId, path) {
    const job = await pollTranscribeJob({
      jobId,
      getJob: api.getTranscribeJob,
      onState: applyJobState,
      onPollError: () => {
        statusText.value = '暂时无法读取转写进度，正在重试…'
      },
      cancelJob: sendCancel,
      cancelRequested: () => cancelRequested.value,
      delay: pollDelay,
      onCancel: (reason) => {
        cancelRequested.value = true
        progressInfo.value = { ...progressInfo.value, label: '正在取消…' }
        statusText.value = reason === 'stalled' ? '长时间没有进展，正在取消任务…' : '正在取消…'
      }
    })
    if (job.status === 'done') {
      return { status: 'done', result: normalizeTranscribeResult(job.result, { path }) }
    }
    if (job.status === 'cancelled') {
      return { status: 'cancelled' }
    }
    throw new Error(job.error || '转写任务失败')
  }

  async function sendCancel(jobId) {
    try {
      await api.cancelTranscribeJob(jobId)
    } catch (err) {
      // 任务已结束或过期（404）：轮询会拿到最终状态，这里不再报错
    }
  }

  /** 取消正在转写的文件：任务在下一个检查点退出，排队中的文件随后继续。 */
  async function cancelCurrent() {
    if (!isBusy.value || cancelRequested.value) {
      return
    }
    cancelRequested.value = true
    progressInfo.value = { ...progressInfo.value, label: '正在取消…' }
    statusText.value = '正在取消…'
    if (currentJobId) {
      await sendCancel(currentJobId)
    }
    wakePoll?.()
  }

  /** 从队列移除一个还没开始的文件。 */
  function removeFromQueue(id) {
    queue.value = removeQueueItem(queue.value, id)
  }

  /** 清掉队列里已结束的项。 */
  function clearFinished() {
    queue.value = clearFinishedQueueItems(queue.value)
  }

  /** 逐个转写队列里的文件；单个失败 / 取消只记在该项上，接着转下一个。 */
  async function runQueue() {
    if (queueRunning) {
      return
    }
    queueRunning = true
    isBusy.value = true
    try {
      let item = nextQueuedItem(queue.value)
      while (item) {
        const { id, path } = item
        queue.value = updateQueueItem(queue.value, id, { status: 'running', error: '' })
        const outcome = await transcribeOne(path)
        queue.value = updateQueueItem(queue.value, id, outcome)
        item = nextQueuedItem(queue.value)
      }
    } finally {
      queueRunning = false
      isBusy.value = false
      statusText.value = ''
      progressInfo.value = { percent: null, label: '' }
      cancelRequested.value = false
      currentJobId = ''
    }
  }

  /** 「开始转写」/ 路径框回车：校验路径框里的文件后排队。 */
  async function startTranscribe() {
    const path = String(filePath.value ?? '').trim()
    if (!path) {
      error.value = '请先选择或粘贴一个本地文件路径'
      return
    }
    if (!isSupportedMediaPath(path)) {
      error.value = UNSUPPORTED_MESSAGE
      return
    }
    enqueue([path])
  }

  /**
   * 转写一个文件（队列逐个调用）：返回 { status: 'done' | 'failed' | 'cancelled', error }。
   */
  async function transcribeOne(path) {
    filePath.value = path
    error.value = ''
    cancelRequested.value = false
    currentJobId = ''
    progressInfo.value = { percent: null, label: '正在提交转写…' }
    statusText.value = '正在提交转写…'

    const request = buildTranscribeRequest({
      path: filePath.value,
      language: language.value,
      wantSegments: wantSegments.value,
      engine: engine.value,
      proofread: proofreadOn.value,
      proofreadModelId: proofreadModelId.value || null,
      proofreadBatchSegments: proofreadBatchSegments.value,
      proofreadContextChars: proofreadContextChars.value,
      asyncJob: true
    })

    const fail = (message) => {
      error.value = message
      return { status: 'failed', error: message }
    }

    // 界面内先确认文件存在（数据面只认令牌，这里只是防误操作）
    if (isDesktop) {
      try {
        await allowTranscribePath(request.path)
      } catch (err) {
        return fail(`无法转写该文件：${err?.message || err}`)
      }
    }

    try {
      const payload = await submitTranscribeRequest({
        submit: () => api.transcribeFile(request),
        cancelRequested: () => cancelRequested.value,
        delay: async (ms) => {
          statusText.value = '转写引擎忙，正在等待重试…'
          await pollDelay(ms)
        }
      })
      if (payload?.cancelled) {
        return { status: 'cancelled', error: '' }
      }
      if (isAsyncJobAccepted(payload)) {
        currentJobId = payload.job_id
        // 提交期间（算哈希、探测时长）已经点了取消：拿到任务号立刻取消
        if (cancelRequested.value) {
          await sendCancel(currentJobId)
        }
        const polled = await pollJobResult(payload.job_id, request.path)
        if (polled.status === 'cancelled') {
          return { status: 'cancelled', error: '' }
        }
        result.value = polled.result
        pushHistory(polled.result)
        return { status: 'done', error: '' }
      }

      // 缓存命中：直接给结果
      const normalized = normalizeTranscribeResult(payload, { path: request.path })
      if (normalized == null) {
        throw new Error('后端响应格式异常：缺少转写文本')
      }
      result.value = normalized
      pushHistory(normalized)
      return { status: 'done', error: '' }
    } catch (err) {
      return fail(err?.message || '转写失败，请稍后重试')
    } finally {
      currentJobId = ''
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

  return {
    filePath,
    language,
    wantSegments,
    engine,
    selectEngine,
    proofreadOn,
    proofreadModelId,
    proofreadBatchSegments,
    proofreadContextChars,
    llmModels,
    loadLlmModels,
    isBusy,
    statusText,
    progressInfo,
    cancelRequested,
    queue,
    error,
    result,
    history,
    chooseFile,
    enqueue,
    ingestDroppedPaths,
    startTranscribe,
    cancelCurrent,
    removeFromQueue,
    clearFinished,
    copyResult,
    exportResult,
    loadHistory,
    selectHistoryItem: (item) => {
      result.value = withSessionResult(item)
      if (result.value.path) {
        filePath.value = result.value.path
      }
    }
  }
}
