// 极简悬浮窗展示模型（src/components/floating/floatingView.mjs）单元测试。
// 覆盖：各相位对应的印章、「轉」沿用上一句（保持正文色）、结果态收起时机、尾部截取、印章字竖直校正、声纹门限、未确认文字、
// 出字动效（对齐已上屏的字、吐字节拍、整行左移逼近）、新会话判定。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  FAILURE_HOLD_MS,
  PACE_CPS,
  PACE_MAX_LAG_S,
  SUCCESS_HOLD_MS,
  TAIL_CHARS,
  WAVE_HOLD_MS,
  WAVE_SHOW_LEVEL,
  deriveFloatingView,
  glideToward,
  nextVoiceState,
  paceStep,
  reconcileLine,
  tailWithPending,
  sealGlyphNudge,
  startsNewSession,
  tailText
} from '../src/components/floating/floatingView.mjs'

test('录音中：聽；没说话时没有文字，单段模式始终没有文字', () => {
  const quiet = deriveFloatingView({ phase: 'recording', mode: 'streaming', text: '' })
  assert.equal(quiet.glyph, '聽')
  assert.equal(quiet.text, '')
  assert.equal(quiet.cancellable, true)

  const speaking = deriveFloatingView({ phase: 'recording', mode: 'multi', text: '明天下午三点开会' })
  assert.equal(speaking.text, '明天下午三点开会')

  assert.equal(deriveFloatingView({ phase: 'recording', mode: 'single', text: '不该显示' }).text, '')
})

test('识别与优化：轉；沿用上一句撑住胶囊（不变淡），优化稿不回填', () => {
  const session = { previousText: '嗯明天下午三点开会' }
  const transcribing = deriveFloatingView({ phase: 'transcribing' }, session)
  assert.equal(transcribing.glyph, '轉')
  assert.equal(transcribing.busy, true)
  assert.equal(transcribing.text, '嗯明天下午三点开会')
  assert.ok(!('dim' in transcribing), '「轉」沿用的识别稿保持正文色，不再变淡')

  const waiting = deriveFloatingView({ phase: 'optimizing', text: '' }, session)
  assert.equal(waiting.text, '嗯明天下午三点开会', '优化中仍沿用上一句')

  const optimizing = deriveFloatingView({ phase: 'optimizing', text: '明天下午三点开会。' }, session)
  assert.equal(optimizing.text, '嗯明天下午三点开会', '优化稿即使带过来也不显示')

  const single = deriveFloatingView({ phase: 'transcribing' })
  assert.equal(single.text, '', '单段模式没有上一句：只剩印章与处理点')
})

test('结果态：成 / 敗 只剩印章，按各自时长收起', () => {
  const success = deriveFloatingView({ phase: 'complete', text: '优化文', ok: true })
  assert.equal(success.glyph, '成')
  assert.equal(success.active, false)
  assert.equal(success.text, '')
  assert.equal(success.hideAfterMs, SUCCESS_HOLD_MS)

  assert.equal(deriveFloatingView({ phase: 'complete', text: '旧载荷' }).glyph, '成', '缺 ok 字段按成功处理')

  const empty = deriveFloatingView({ phase: 'complete', text: '没有识别到语音内容', ok: false })
  assert.equal(empty.glyph, '敗')
  assert.equal(empty.detail, '没有识别到语音内容')
  assert.equal(empty.hideAfterMs, FAILURE_HOLD_MS)

  const error = deriveFloatingView({ phase: 'error', text: '鉴权失败' })
  assert.equal(error.glyph, '敗')
  assert.equal(error.cancellable, false)
})

test('未知相位回到待命：錄、不自动隐藏', () => {
  const idle = deriveFloatingView({})
  assert.equal(idle.glyph, '錄')
  assert.equal(idle.active, false)
  assert.equal(idle.hideAfterMs, null)
})

test('尾部截取折叠空白、只留末尾', () => {
  assert.equal(tailText('  第一行\n\n第二行  '), '第一行 第二行')
  assert.equal(tailText('一二三四五', 3), '三四五')
  assert.equal(Array.from(tailText('字'.repeat(200))).length, TAIL_CHARS)
})

test('印章字校正：量出偏下多少就上移多少，并以暗边之上的受光面为中心', () => {
  // Songti TC 22px 实测：字体上伸 23 / 下伸 7，「聽」墨迹上 16.85 / 下 3.96 → 墨迹中心比印章中心低约 1.55px
  assert.equal(sealGlyphNudge({ fontAscent: 23, fontDescent: 7, inkAscent: 16.85, inkDescent: 3.96 }), -2.5)
  // 字体本身居中（PingFang 类）：只扣暗边的 1px
  assert.equal(sealGlyphNudge({ fontAscent: 11, fontDescent: 11, inkAscent: 10, inkDescent: 10 }), -1)
  // 量不到度量：同样只扣暗边
  assert.equal(sealGlyphNudge({}), -1)
})

test('声纹门限：底噪不触发，过门限立即出现，停顿片刻内保持，安静够久才收起', () => {
  let voice = nextVoiceState()
  assert.equal(voice.loud, false)

  // -40 dBFS 左右的底噪（level ≈ 0.06）不算说话
  voice = nextVoiceState(voice, 0.06, 0)
  assert.equal(voice.loud, false)

  voice = nextVoiceState(voice, WAVE_SHOW_LEVEL, 100)
  assert.equal(voice.loud, true)

  // 字与字之间的停顿：门限下但没超过保持时长，声纹不收
  voice = nextVoiceState(voice, 0.02, 100 + WAVE_HOLD_MS - 1)
  assert.equal(voice.loud, true)

  voice = nextVoiceState(voice, 0.02, 100 + WAVE_HOLD_MS)
  assert.equal(voice.loud, false)

  // 安静后再次说话立即恢复
  assert.equal(nextVoiceState(voice, 0.5, 5000).loud, true)
})

test('进入优化：转写原文作为草稿显示到结束', () => {
  const session = { previousText: '明天下午三点开' }
  const begin = deriveFloatingView({ phase: 'optimizing', text: '', draft: '明天下午三点开会' }, session)
  assert.equal(begin.glyph, '轉')
  assert.equal(begin.text, '明天下午三点开会', '草稿比上一句中间结果更新，优先显示')
})

test('流式未确认文字：末尾那截单独标出，截取后仍是全文的后缀', () => {
  assert.deepEqual(tailWithPending('你好世界', 2), { text: '你好世界', pending: '世界' })
  assert.deepEqual(tailWithPending('你好世界', 0), { text: '你好世界', pending: '' })
  assert.deepEqual(tailWithPending('你好世界', 99), { text: '你好世界', pending: '你好世界' }, '字数越界按全部未确认')

  // 截取末尾：未确认部分落在截取范围内保持原样，比截取范围还长则整行都算未确认
  assert.deepEqual(tailWithPending('一二三四五六七八九十', 3, 5), { text: '六七八九十', pending: '八九十' })
  assert.deepEqual(tailWithPending('一二三四五六七八九十', 8, 5), { text: '六七八九十', pending: '六七八九十' })

  const streaming = deriveFloatingView({ phase: 'recording', mode: 'streaming', text: '明天下午三点开会', pending: 2 })
  assert.equal(streaming.text, '明天下午三点开会')
  assert.equal(streaming.pending, '开会')

  assert.equal(deriveFloatingView({ phase: 'recording', mode: 'multi', text: '分段定稿' }).pending, '', '多段结果都是定稿')
  assert.equal(deriveFloatingView({ phase: 'transcribing' }, { previousText: '明天下午三点开会' }).pending, '', '「轉」不再区分未确认的那截')
})

test('出字对齐：追加保留全部、回改换掉末尾、裁头只去开头', () => {
  const chars = (t) => Array.from(t)
  assert.deepEqual(reconcileLine(chars('今天下午'), chars('今天下午三点')), { drop: 0, keep: 4 }, '末尾追加')
  assert.deepEqual(reconcileLine(chars('我们在山楼'), chars('我们在三楼会议室')), { drop: 0, keep: 3 }, '回改「山楼」')
  assert.deepEqual(reconcileLine(chars('一二三四五'), chars('三四五六七')), { drop: 2, keep: 3 }, 'tailText 裁掉开头')
  assert.deepEqual(reconcileLine(chars('天天天'), chars('天天天天')), { drop: 0, keep: 3 }, '重复字取最小裁头')
  assert.deepEqual(reconcileLine(chars('甲乙'), chars('丙丁')), { drop: 0, keep: 0 }, '完全对不上整行回改')
  assert.deepEqual(reconcileLine([], chars('新')), { drop: 0, keep: 0 })
})

test('吐字节拍：平时匀速，积压多时加速，落后不超过上限', () => {
  // 平时 16 字/秒：一帧 16ms 攒不够一个字，攒满后放一个
  let step = paceStep(0, 16, 3)
  assert.equal(step.emit, 0)
  let budget = 0
  let emitted = 0
  for (let t = 0; t < 1000; t += 16) {
    step = paceStep(budget, 16, 100)
    budget = step.budget
    emitted += step.emit
  }
  assert.ok(emitted >= 100 * 0.9, `积压 100 字时一秒内应基本放完，实际 ${emitted}`)

  // 一次到 3 个字：按 PACE_CPS 放，约 3/16 秒放完而不是一下全出
  step = paceStep(0, 1000 / PACE_CPS, 3)
  assert.equal(step.emit, 1)
  assert.equal(paceStep(0.9, 1000 / PACE_CPS, 1).emit, 1, '不超过积压数')
  assert.deepEqual(paceStep(0.7, 16, 0), { emit: 0, budget: 0 }, '没有积压时清零')
  assert.ok(PACE_MAX_LAG_S <= 0.5)
})

test('整行左移：向目标逼近、足够近时落定', () => {
  const half = glideToward(0, 100, 110 * Math.LN2)
  assert.ok(Math.abs(half - 50) < 0.01, '一个半衰期走一半')
  assert.equal(glideToward(99.9, 100, 16), 100, '差距很小直接落定')
  assert.equal(glideToward(40, 40, 16), 40)
})

test('新会话判定：从收起进入工作态，或流式在「轉」时直接开始下一段', () => {
  const listening = deriveFloatingView({ phase: 'recording', mode: 'streaming', text: '' })
  const converting = deriveFloatingView({ phase: 'optimizing', mode: 'streaming', draft: '上一段' })
  const success = deriveFloatingView({ phase: 'complete', ok: true })

  assert.equal(startsNewSession({ wasActive: false, prevSeal: 'idle', next: listening }), true, '从待命开始')
  assert.equal(startsNewSession({ wasActive: false, prevSeal: 'success', next: listening }), true, '结果态停留中又按下')
  assert.equal(startsNewSession({ wasActive: true, prevSeal: 'converting', next: listening }), true, '上一段优化转入后台')
  assert.equal(startsNewSession({ wasActive: true, prevSeal: 'listening', next: listening }), false, '录音中的中间结果')
  assert.equal(startsNewSession({ wasActive: true, prevSeal: 'listening', next: converting }), false, '松开进入轉')
  assert.equal(startsNewSession({ wasActive: true, prevSeal: 'converting', next: success }), false, '结果态不是会话开头')
})
