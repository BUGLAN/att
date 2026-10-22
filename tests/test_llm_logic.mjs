/**
 * LLM 设置页纯逻辑回归测试（src/components/settings/llm/llmLogic.mjs）。
 *
 * 锁死的点：
 * 1. 模型串组装与后端 compose_model_string 一致（新模型零发版的关键约定）；
 * 2. 手输模型名解析：分隔、去重、非法项单独报告；
 * 3. 错误分级 → 中文文案（P7：建议可执行）；
 * 4. 展示映射（状态徽章 / 延迟 / 参数摘要 / 密钥提示）。
 */
import test from 'node:test'
import assert from 'node:assert/strict'

import {
  composeCustomModelString,
  composeModelString,
  describeParams,
  errorCopy,
  formatLatency,
  isValidModelName,
  keyDisplay,
  parseManualModelInput,
  statusTone,
  STATUS_LABELS
} from '../src/components/settings/llm/llmLogic.mjs'

// ==================== 模型串组装 ====================

test('composeModelString：裸模型名按服务商前缀组装', () => {
  assert.equal(composeModelString('deepseek', 'deepseek-flash'), 'deepseek/deepseek-flash')
  assert.equal(composeModelString('ollama_chat', 'qwen3:14b'), 'ollama_chat/qwen3:14b')
})

test('composeModelString：已带 / 的名字原样透传（高级用户手输完整串）', () => {
  assert.equal(composeModelString('deepseek', 'openrouter/qwen/max'), 'openrouter/qwen/max')
})

test('composeModelString：空名返回空串、前缀缺失回退 openai', () => {
  assert.equal(composeModelString('deepseek', '   '), '')
  assert.equal(composeModelString('', 'm'), 'openai/m')
})

test('composeCustomModelString：含斜杠的本地网关模型 ID 整串包 openai/（2026-10-22 实测踩坑）', () => {
  assert.equal(
    composeCustomModelString('group/deepseek-v4.1-flash'),
    'openai/group/deepseek-v4.1-flash',
    '绝不能把首段 group 透传给 LiteLLM 当服务商前缀'
  )
  assert.equal(composeCustomModelString('deepseek-flash'), 'openai/deepseek-flash')
  assert.equal(composeCustomModelString('openai/mistral'), 'openai/mistral', '对已是 openai/ 开头的 ID 幂等')
  assert.equal(composeCustomModelString('  '), '')
})

// ==================== 手输解析 ====================

test('parseManualModelInput：中英文逗号/换行分隔、去空去重', () => {
  const { valid, invalid } = parseManualModelInput(' deepseek-v4.2-preview ，deepseek-flash\ninvalid name，deepseek-flash;')
  assert.deepEqual(valid, ['deepseek-v4.2-preview', 'deepseek-flash'])
  assert.deepEqual(invalid, ['invalid name'])
})

test('parseManualModelInput：完整模型串也算合法输入', () => {
  const { valid, invalid } = parseManualModelInput('custom/mymodel')
  assert.deepEqual(valid, ['custom/mymodel'])
  assert.deepEqual(invalid, [])
})

test('isValidModelName：与后端 is_valid_model_string 同一字符集', () => {
  assert.equal(isValidModelName('deepseek/deepseek-flash'), true)
  assert.equal(isValidModelName('qwen3:14b'), true)
  assert.equal(isValidModelName(''), false)
  assert.equal(isValidModelName('has space'), false)
})

// ==================== 错误文案 ====================

test('errorCopy：按分级给标题，message 优先作为详情', () => {
  const copy = errorCopy('auth_failed', '鉴权失败：***。建议：检查 API Key')
  assert.equal(copy.title, '鉴权失败')
  assert.equal(copy.detail, '鉴权失败：***。建议：检查 API Key')
})

test('errorCopy：message 缺失时回退内置建议', () => {
  const copy = errorCopy('timeout', '')
  assert.equal(copy.title, '请求超时')
  assert.match(copy.detail, /检查网络|代理/)
})

test('errorCopy：未知分级不崩溃并落到 unknown', () => {
  const copy = errorCopy('totally_new_code', undefined)
  assert.equal(copy.title, '验证失败')
  assert.match(copy.detail, /日志/)
})

// ==================== 展示映射 ====================

test('statusTone 与状态标签', () => {
  assert.equal(statusTone('ok'), 'ok')
  assert.equal(statusTone('failed'), 'err')
  assert.equal(statusTone('decrypt_failed'), 'err')
  assert.equal(statusTone('unknown'), 'neutral')
  assert.equal(STATUS_LABELS.decrypt_failed, '密钥无法解密')
})

test('formatLatency：亚秒用毫秒，超秒用秒', () => {
  assert.equal(formatLatency(812), '812ms')
  assert.equal(formatLatency(1900), '1.9s')
  assert.equal(formatLatency(null), '')
  assert.equal(formatLatency(-5), '')
})

test('describeParams：参数摘要行', () => {
  assert.equal(
    describeParams({ temperature: 0.3, max_tokens: 4096, timeout_s: 60 }),
    'temp 0.3 · max 4,096 · 超时 60s'
  )
  assert.equal(describeParams(null), '')
})

test('keyDisplay：掩码展示与无密钥提示', () => {
  assert.equal(keyDisplay('sk-****7f3a', true), '密钥已保存（sk-****7f3a）')
  assert.equal(keyDisplay('', true), '未设置密钥')
  assert.equal(keyDisplay('', false), '无需密钥')
})

test('进程显式退出（run-node-tests 约定）', () => {
  process.exit(0)
})
