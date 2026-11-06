// 提示词预设前端纯逻辑（src/components/settings/presetLogic.mjs）单元测试。
// 覆盖：分组、草稿校验（与后端规则一致）、新建默认名、删除后选用处的回退、删除确认文案。
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  presetExcerpt,
  MAX_NAME_CHARS,
  describeDeleteImpact,
  resolveDefaultPresetId,
  resolveRewritePresetId,
  splitPresets,
  suggestNewName,
  validatePresetDraft
} from '../src/components/settings/presetLogic.mjs'

const PRESETS = [
  { id: 'preset-程序员模式', name: '程序员模式', is_builtin: true },
  { id: 'preset-通用优化', name: '通用优化', is_builtin: true },
  { id: 'custom-a', name: '周报', is_builtin: false },
  { id: 'custom-b', name: '会议纪要', is_builtin: false }
]

test('分组：内置与自定义各自保持服务端顺序', () => {
  const { builtin, custom } = splitPresets(PRESETS)
  assert.deepEqual(builtin.map((p) => p.id), ['preset-程序员模式', 'preset-通用优化'])
  assert.deepEqual(custom.map((p) => p.id), ['custom-a', 'custom-b'])
  assert.deepEqual(splitPresets(), { builtin: [], custom: [] })
})

test('校验：空名 / 超长 / 重名 / 空正文；编辑自己时同名不算重名', () => {
  assert.equal(validatePresetDraft({ name: '  ', prompt_template: 'x' }, PRESETS), '预设名称不能为空')
  assert.match(validatePresetDraft({ name: '长'.repeat(MAX_NAME_CHARS + 1), prompt_template: 'x' }, PRESETS), /不能超过/)
  // 按字符计：40 个 emoji 不超限（UTF-16 长度是 80）
  assert.equal(validatePresetDraft({ name: '😀'.repeat(MAX_NAME_CHARS), prompt_template: 'x' }, PRESETS), '')
  assert.match(validatePresetDraft({ name: '通用优化', prompt_template: 'x' }, PRESETS), /已有名为「通用优化」/)
  assert.match(validatePresetDraft({ name: ' 周报 ', prompt_template: 'x' }, PRESETS), /已有名为「周报」/)
  assert.equal(validatePresetDraft({ name: '周报', prompt_template: 'x' }, PRESETS, 'custom-a'), '')
  assert.equal(validatePresetDraft({ name: '新的', prompt_template: '  \n ' }, PRESETS), '提示词内容不能为空')
  assert.equal(validatePresetDraft({ name: '新的', prompt_template: '正文' }, PRESETS), '')
})

test('默认名：新建递增', () => {
  assert.equal(suggestNewName(PRESETS), '新预设')
  assert.equal(suggestNewName([...PRESETS, { id: 'custom-c', name: '新预设' }]), '新预设 2')
})

test('回退：默认预设失效 → 通用优化 → 第一个；优化选中文本失效 → 跟随默认', () => {
  assert.equal(resolveDefaultPresetId('custom-a', PRESETS), 'custom-a')
  assert.equal(resolveDefaultPresetId('custom-已删', PRESETS), 'preset-通用优化')
  assert.equal(resolveDefaultPresetId('', [{ id: 'custom-x', name: '通用优化', is_builtin: false }, { id: 'preset-a', name: 'a' }]), 'custom-x',
    '没有内置「通用优化」时取第一个')
  assert.equal(resolveDefaultPresetId('x', []), '')
  assert.equal(resolveRewritePresetId('custom-a', PRESETS), 'custom-a')
  assert.equal(resolveRewritePresetId('custom-已删', PRESETS), '')
  assert.equal(resolveRewritePresetId('', PRESETS), '')
})

test('删除确认：未被使用 / 被默认与优化选中文本同时使用', () => {
  const target = PRESETS[2]
  assert.equal(describeDeleteImpact(target, {}, PRESETS), '删除后无法恢复。')
  const text = describeDeleteImpact(target, { defaultPresetId: 'custom-a', rewritePresetId: 'custom-a' }, PRESETS)
  assert.match(text, /录音默认预设将改为「通用优化」/)
  assert.match(text, /「优化选中文本」将改为跟随默认预设/)
  assert.equal(describeDeleteImpact(null), '')
})

test('presetExcerpt：跳过 Markdown 标题取第一行正文（内置预设都以「# 角色」开头）', () => {
  assert.equal(presetExcerpt('# 角色\n你是语音转写文本的整理器，专门处理技术内容。'), '你是语音转写文本的整理器，专门处理技术内容。')
  assert.equal(presetExcerpt('\n\n## 任务\n---\n- 去掉口头禅'), '去掉口头禅')
  assert.equal(presetExcerpt('直接就是正文'), '直接就是正文')
  assert.equal(presetExcerpt('# 只有标题'), '')
  assert.equal(presetExcerpt(''), '')
  assert.equal(presetExcerpt(null), '')
  assert.equal(presetExcerpt('甲'.repeat(70)), `${'甲'.repeat(60)}…`)
})
