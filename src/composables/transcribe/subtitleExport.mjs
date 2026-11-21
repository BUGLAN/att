/**
 * 「转文字」结果导出：TXT 全文、SRT / VTT 字幕（纯逻辑，不依赖 Vue / 浏览器全局，可在 Node 里测试）。
 *
 * - TXT：有校对稿时用校对稿（结果里的 `text` 已是校对后的版本）；
 * - SRT：`序号\n00:00:01,000 --> 00:00:03,500\n文本\n`，块之间空一行；
 * - VTT：首行 `WEBVTT`，时间用 `.` 分隔毫秒（`00:00:01.000`）；
 * - 字幕按分段时间戳生成：没有分段（转写时没开「分段时间戳」，或从记录集回看的历史）时不能导出。
 */

/** 导出菜单项（转文字页「导出」）。 */
export const TRANSCRIBE_EXPORT_FORMATS = Object.freeze([
  { value: 'txt', label: '纯文本（.txt）' },
  { value: 'srt', label: 'SRT 字幕（.srt）' },
  { value: 'vtt', label: 'WebVTT 字幕（.vtt）' }
])

export const SUBTITLE_NEEDS_SEGMENTS =
  '字幕要用分段时间戳：打开「分段时间戳」后重新转写这个文件（同一文件会直接命中缓存）'

const MIME_TYPES = {
  txt: 'text/plain;charset=utf-8',
  srt: 'application/x-subrip;charset=utf-8',
  vtt: 'text/vtt;charset=utf-8'
}

/** 导出文件的 MIME 类型。 */
export function exportMimeType(format) {
  return MIME_TYPES[format] || MIME_TYPES.txt
}

/**
 * 秒 → 字幕时间戳：SRT `HH:MM:SS,mmm`，VTT `HH:MM:SS.mmm`（负数 / 非法按 0）。
 */
export function formatSubtitleTime(seconds, separator = ',') {
  const safe = Number.isFinite(Number(seconds)) && Number(seconds) > 0 ? Number(seconds) : 0
  const totalMs = Math.round(safe * 1000)
  const ms = totalMs % 1000
  const totalSeconds = Math.floor(totalMs / 1000)
  const pad = (value, width = 2) => String(value).padStart(width, '0')
  return `${pad(Math.floor(totalSeconds / 3600))}:${pad(Math.floor((totalSeconds % 3600) / 60))}:${pad(totalSeconds % 60)}${separator}${pad(ms, 3)}`
}

/**
 * 分段 → 字幕块：去掉空文本；结束不晚于开始时补成开始 + 1 秒（避免播放器丢弃这一条）；
 * 文本里的空行会被播放器当成块结束，合并成单个换行。
 */
export function subtitleCues(segments) {
  return (Array.isArray(segments) ? segments : [])
    .map((segment) => {
      const text = String(segment?.text ?? '').replace(/\r\n?/g, '\n').replace(/\n{2,}/g, '\n').trim()
      const start = Number.isFinite(Number(segment?.start)) ? Math.max(0, Number(segment.start)) : 0
      let end = Number.isFinite(Number(segment?.end)) ? Number(segment.end) : start
      if (end <= start) {
        end = start + 1
      }
      return { start, end, text }
    })
    .filter((cue) => cue.text)
}

/** SRT 字幕全文（没有可用分段时返回空串）。 */
export function buildSrt(segments) {
  const cues = subtitleCues(segments)
  if (!cues.length) {
    return ''
  }
  return cues
    .map((cue, index) => `${index + 1}\n${formatSubtitleTime(cue.start, ',')} --> ${formatSubtitleTime(cue.end, ',')}\n${cue.text}\n`)
    .join('\n')
}

/** WebVTT 字幕全文（没有可用分段时返回空串）。 */
export function buildVtt(segments) {
  const cues = subtitleCues(segments)
  if (!cues.length) {
    return ''
  }
  const body = cues
    .map((cue) => `${formatSubtitleTime(cue.start, '.')} --> ${formatSubtitleTime(cue.end, '.')}\n${cue.text}\n`)
    .join('\n')
  return `WEBVTT\n\n${body}`
}

/** 能不能导出字幕（有至少一段带文本的分段）。 */
export function canExportSubtitles(result) {
  return subtitleCues(result?.segments).length > 0
}

/** 导出文件名：原文件名去掉扩展名 + 新扩展名；拿不到文件名时用 `transcript`。 */
export function exportFileName(fileName, format) {
  const base = String(fileName ?? '').replace(/\.[^./\\]+$/, '').trim() || 'transcript'
  return `${base}.${format}`
}

/**
 * 按格式生成导出内容：{ content, filename, mime } 或 { error }（字幕没有分段 / 没有文字）。
 */
export function buildTranscribeExport(result, format) {
  if (!result) {
    return { error: '还没有转写结果' }
  }
  const filename = exportFileName(result.fileName, format)
  const mime = exportMimeType(format)
  if (format === 'srt' || format === 'vtt') {
    if (!canExportSubtitles(result)) {
      return { error: SUBTITLE_NEEDS_SEGMENTS }
    }
    const content = format === 'srt' ? buildSrt(result.segments) : buildVtt(result.segments)
    return { content, filename, mime }
  }
  const text = String(result.text ?? '').trim()
  if (!text) {
    return { error: '没有识别到文字，无法导出' }
  }
  return { content: `${text}\n`, filename, mime }
}
