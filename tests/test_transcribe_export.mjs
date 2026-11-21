import test from 'node:test'
import assert from 'node:assert/strict'

import {
  SUBTITLE_NEEDS_SEGMENTS,
  TRANSCRIBE_EXPORT_FORMATS,
  buildSrt,
  buildTranscribeExport,
  buildVtt,
  canExportSubtitles,
  exportFileName,
  formatSubtitleTime,
  subtitleCues
} from '../src/composables/transcribe/subtitleExport.mjs'
import {
  TRANSCRIBE_HISTORY_LIMIT,
  historyFromRecords,
  normalizeTranscribeResult,
  recordToTranscribeResult
} from '../src/composables/transcribe/fileTranscribeModel.mjs'

const segments = [
  { start: 1, end: 3.5, text: '第一句' },
  { start: 3.5, end: 3725.0425, text: '跨小时的一句' },
  { start: 4000, end: 4002, text: '   ' }
]

test('formatSubtitleTime：SRT 用逗号、VTT 用点，补齐到毫秒', () => {
  assert.equal(formatSubtitleTime(1), '00:00:01,000')
  assert.equal(formatSubtitleTime(1, '.'), '00:00:01.000')
  assert.equal(formatSubtitleTime(3725.0425), '01:02:05,043')
  assert.equal(formatSubtitleTime(59.9996), '00:01:00,000', '四舍五入进位到下一秒')
  assert.equal(formatSubtitleTime(-2), '00:00:00,000')
  assert.equal(formatSubtitleTime('x'), '00:00:00,000')
})

test('buildSrt：序号、时间行、文本，块之间空一行；空文本段跳过', () => {
  assert.equal(
    buildSrt(segments),
    '1\n00:00:01,000 --> 00:00:03,500\n第一句\n\n2\n00:00:03,500 --> 01:02:05,043\n跨小时的一句\n'
  )
})

test('buildVtt：WEBVTT 头 + 点分毫秒', () => {
  assert.equal(
    buildVtt(segments),
    'WEBVTT\n\n00:00:01.000 --> 00:00:03.500\n第一句\n\n00:00:03.500 --> 01:02:05.043\n跨小时的一句\n'
  )
})

test('subtitleCues：结束不晚于开始时补 1 秒；文本里的空行合并成单个换行', () => {
  assert.deepEqual(subtitleCues([{ start: 5, end: 5, text: 'a\n\n\nb' }]), [{ start: 5, end: 6, text: 'a\nb' }])
  assert.deepEqual(subtitleCues(null), [])
})

test('buildTranscribeExport：TXT 用结果文本（有校对时即校对稿）；字幕缺分段时给出原因', () => {
  const result = { fileName: '周会 录音.mp4', text: '校对稿', rawText: '原文', segments }
  assert.deepEqual(buildTranscribeExport(result, 'txt'), {
    content: '校对稿\n',
    filename: '周会 录音.txt',
    mime: 'text/plain;charset=utf-8'
  })
  const srt = buildTranscribeExport(result, 'srt')
  assert.equal(srt.filename, '周会 录音.srt')
  assert.ok(srt.content.startsWith('1\n00:00:01,000'))
  assert.equal(buildTranscribeExport(result, 'vtt').mime, 'text/vtt;charset=utf-8')

  const bare = { fileName: 'a.wav', text: '有字', segments: [] }
  assert.ok(!canExportSubtitles(bare))
  assert.deepEqual(buildTranscribeExport(bare, 'srt'), { error: SUBTITLE_NEEDS_SEGMENTS })
  assert.ok(buildTranscribeExport({ fileName: 'a.wav', text: '  ' }, 'txt').error)
  assert.ok(buildTranscribeExport(null, 'txt').error)
  assert.deepEqual(TRANSCRIBE_EXPORT_FORMATS.map((item) => item.value), ['txt', 'srt', 'vtt'])
})

test('exportFileName：去掉原扩展名；没有文件名时用 transcript', () => {
  assert.equal(exportFileName('a.b.mp4', 'srt'), 'a.b.srt')
  assert.equal(exportFileName('无后缀', 'vtt'), '无后缀.vtt')
  assert.equal(exportFileName('', 'txt'), 'transcript.txt')
})

test('recordToTranscribeResult：文件转写记录 → 结果视图；有校对稿时原文进 rawText', () => {
  const record = {
    id: 'rec-1',
    created_at: '2026-11-21T10:00:00Z',
    duration: 61.5,
    raw_text: '原文',
    optimized_text: '校对稿',
    engine: 'whisper',
    source_file: '周会.mp4'
  }
  const view = recordToTranscribeResult(record)
  assert.equal(view.fileName, '周会.mp4')
  assert.equal(view.recordId, 'rec-1')
  assert.equal(view.text, '校对稿')
  assert.equal(view.rawText, '原文')
  assert.equal(view.durationSeconds, 61.5)
  assert.deepEqual(view.segments, [])
  assert.equal(view.proofread.applied, true)
  assert.equal(view.fromRecord, true)

  const plain = recordToTranscribeResult({ ...record, optimized_text: null })
  assert.equal(plain.text, '原文')
  assert.equal(plain.rawText, '')
  assert.equal(plain.proofread, null)
  // 录音记录（没有来源文件）不进转文字页历史
  assert.equal(recordToTranscribeResult({ ...record, source_file: undefined }), null)
})

test('historyFromRecords：只留文件转写的记录，最多 20 条', () => {
  const items = Array.from({ length: 25 }, (_, index) => ({
    id: `r${index}`,
    raw_text: `第 ${index} 条`,
    source_file: index === 3 ? undefined : `f${index}.mp3`
  }))
  const history = historyFromRecords(items)
  assert.equal(history.length, TRANSCRIBE_HISTORY_LIMIT)
  assert.ok(!history.some((item) => item.recordId === 'r3'))
  assert.deepEqual(historyFromRecords(null), [])
})

test('normalizeTranscribeResult：带上后端返回的 record_id', () => {
  assert.equal(normalizeTranscribeResult({ text: 'x', record_id: 'rec-9' }).recordId, 'rec-9')
  assert.equal(normalizeTranscribeResult({ text: 'x' }).recordId, '')
})
