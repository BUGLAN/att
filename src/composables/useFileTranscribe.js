/**
 * 「转文字」页编排：选文件（可多选 / 一次拖入多个）→ 排队 → 逐个 POST /api/transcribe（async_job）
 * → 轮询任务进度 → 归一结果；转写中可以取消，单个失败或取消不影响后面排队的文件。
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
  isAsyncJobAccepted,
  isJobPollExpired,
  isSupportedMediaPath,
  jobProgressSignature,
  nextJobPollDelay,
  nextQueuedItem,
  normalizeTranscribeResult,
  removeQueueItem,
  updateQueueItem
} from '@/composables/transcribe/fileTranscribeModel.mjs'
import { normalizeLanguageForProvider } from '@/components/settings/asrLanguages.mjs'

const HISTORY_LIMIT = 20
// 取消之后轮询改成短间隔：尽快看到任务真正退出、开始下一个
const CANCEL_POLL_DELAY_MS = 400
const UNSUPPORTED_MESSAGE = '暂不支持该文件类型：请选择音视频文件（mp4 / flv / webm / wav / mp3 / m4a 等）'

export function useFileTranscribe() {
  const { openMediaFiles, allowTranscribePath, isDesktop, filePathForFile, clipboard } = useElectron()
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
  const stage = ref('')                   // asr | proofreading（202 阶段展示）
  const progress = shallowRef(null)       // asr: {stage, done_seconds, total_seconds}；校对: {window_done, window_total}
  const progressInfo = shallowRef({ percent: null, label: '' }) // 进度条与文案
  const cancelRequested = ref(false)
  const queue = shallowRef([])            // [{id, path, fileName, status, error}]
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

  // 每个文件开始时自增；轮询循环发现 runId 变化即自行退出（reset 时让进行中的轮询失效）
  let runSeq = 0
  let queueRunning = false
  let queueGen = 0 // reset 时自增：旧的队列循环退出时不再改动新一轮的状态
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

  /**
   * 拖拽入口（浏览器 File 对象）：取本地路径后排队。
   * 拿不到路径（浏览器环境）或类型不支持时只置 error，不发起请求。
   */
  async function ingestDroppedFiles(files) {
    const list = Array.from(files || [])
    if (!list.length) {
      return false
    }
    return ingestDroppedPaths(list.map((file) => filePathForFile(file)))
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

  function pushHistory(entry) {
    history.value = [entry, ...history.value.filter((item) => item.path !== entry.path)]
      .slice(0, HISTORY_LIMIT)
  }

  function applyJobState(job) {
    stage.value = String(job?.stage || '')
    progress.value = job?.progress || null
    progressInfo.value = describeJobProgress(job, { cancelRequested: cancelRequested.value })
    statusText.value = progressInfo.value.label
  }

  /** 轮询到任务结束：{ status: 'done', result } | { status: 'cancelled' }；失败抛错；被 reset 接管返回 null。 */
  async function pollJobResult(jobId, currentRun, path) {
    let lastSignature = ''
    let lastAdvanceAt = Date.now()
    let attempt = 0

    while (true) {
      attempt += 1
      await pollDelay(cancelRequested.value ? CANCEL_POLL_DELAY_MS : nextJobPollDelay(attempt))
      if (currentRun !== runSeq) {
        return null // 已被 reset 接管
      }

      const job = await api.getTranscribeJob(jobId)
      applyJobState(job)

      if (job.status === 'done') {
        return { status: 'done', result: normalizeTranscribeResult(job.result, { path }) }
      }
      if (job.status === 'cancelled') {
        return { status: 'cancelled' }
      }
      if (job.status === 'failed') {
        throw new Error(job.error || '转写任务失败')
      }
      // 进度还在推进就一直等；停滞超过上限才放弃（任务仍在后台继续，可稍后重试同一文件命中缓存）
      const signature = jobProgressSignature(job)
      if (signature !== lastSignature) {
        lastSignature = signature
        lastAdvanceAt = Date.now()
      } else if (isJobPollExpired(Date.now() - lastAdvanceAt)) {
        throw new Error('转写任务 15 分钟没有进展，已停止等待；任务仍会在后台继续，可稍后重试同一文件（会命中缓存）')
      }
    }
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
    const gen = queueGen
    isBusy.value = true
    try {
      let item = nextQueuedItem(queue.value)
      while (item) {
        const { id, path } = item
        queue.value = updateQueueItem(queue.value, id, { status: 'running', error: '' })
        const outcome = await transcribeOne(path)
        if (outcome == null) {
          return // 被 reset 接管
        }
        queue.value = updateQueueItem(queue.value, id, outcome)
        item = nextQueuedItem(queue.value)
      }
    } finally {
      // 被 reset 接管时新一轮的状态由 reset / 新循环负责
      if (gen === queueGen) {
        queueRunning = false
        isBusy.value = false
        statusText.value = ''
        stage.value = ''
        progress.value = null
        progressInfo.value = { percent: null, label: '' }
        cancelRequested.value = false
        currentJobId = ''
      }
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
   * 转写一个文件（队列逐个调用）：返回 { status: 'done' | 'failed' | 'cancelled', error }；
   * 被 reset 接管时返回 null。
   */
  async function transcribeOne(path) {
    const currentRun = ++runSeq
    filePath.value = path
    error.value = ''
    stage.value = ''
    progress.value = null
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

    // 数据面只接受本机用户确认过的文件：选择 / 拖入 / 手动输入的路径在发起请求前逐个登记
    if (isDesktop) {
      try {
        await allowTranscribePath(request.path)
      } catch (err) {
        return fail(`无法转写该文件：${err?.message || err}`)
      }
    }

    try {
      const payload = await api.transcribeFile(request)
      if (currentRun !== runSeq) {
        return null
      }

      if (isAsyncJobAccepted(payload)) {
        currentJobId = payload.job_id
        // 提交期间（算哈希、探测时长）已经点了取消：拿到任务号立刻取消
        if (cancelRequested.value) {
          await sendCancel(currentJobId)
        }
        const polled = await pollJobResult(payload.job_id, currentRun, request.path)
        if (polled == null) {
          return null
        }
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
      if (currentRun !== runSeq) {
        return null
      }
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

  function reset() {
    if (currentJobId) {
      void sendCancel(currentJobId)
    }
    runSeq += 1 // 使进行中的轮询失效
    queueGen += 1
    queueRunning = false
    cancelRequested.value = false
    currentJobId = ''
    wakePoll?.()
    queue.value = []
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
    selectEngine,
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
    progressInfo,
    cancelRequested,
    queue,
    error,
    result,
    history,
    chooseFile,
    enqueue,
    ingestDroppedFiles,
    ingestDroppedPaths,
    startTranscribe,
    cancelCurrent,
    removeFromQueue,
    clearFinished,
    copyResult,
    selectHistoryItem: (item) => {
      result.value = item
      filePath.value = item.path
    },
    reset
  }
}
