// 提示词预设的前端纯逻辑（无 Vue 依赖，tests/test_preset_logic.mjs 覆盖）。
//
// 预设分两类：
// - 内置：prompts/*.txt，只读（打包后是只读资源，升级会覆盖），id = `preset-<文件名>`
// - 自定义：data/custom_prompts.json，可增删改，id = `custom-<uuid>`（改名不变）
// 校验规则与后端 att_core::prompts 保持一致；后端仍是最终裁决，这里只为了即时提示。

export const DEFAULT_PRESET_NAME = '通用优化'
export const MAX_NAME_CHARS = 40
export const MAX_TEMPLATE_CHARS = 20000

/** 按字符（而非 UTF-16 码元）计长度，和 Rust 的 chars().count() 对齐 */
function charCount(text) {
  return Array.from(text).length
}

/** 拆成内置 / 自定义两组，保持服务端给的顺序 */
export function splitPresets(presets = []) {
  const builtin = []
  const custom = []
  for (const preset of presets) {
    if (preset?.is_builtin === false) {
      custom.push(preset)
    } else {
      builtin.push(preset)
    }
  }
  return { builtin, custom }
}

/**
 * 校验编辑中的草稿，返回第一条错误文案；通过返回空串。
 * @param {{ name?: string, prompt_template?: string }} draft
 * @param {Array} presets 当前全部预设（用于重名检查）
 * @param {string} [editingId] 正在编辑的预设 id（和自己同名不算重名）
 */
export function validatePresetDraft(draft, presets = [], editingId = '') {
  const name = String(draft?.name ?? '').trim()
  const template = String(draft?.prompt_template ?? '').trim()
  if (!name) {
    return '预设名称不能为空'
  }
  if (charCount(name) > MAX_NAME_CHARS) {
    return `预设名称不能超过 ${MAX_NAME_CHARS} 个字`
  }
  if (presets.some((preset) => preset.name === name && preset.id !== editingId)) {
    return `已有名为「${name}」的预设，换个名字吧`
  }
  if (!template) {
    return '提示词内容不能为空'
  }
  if (charCount(template) > MAX_TEMPLATE_CHARS) {
    return `提示词内容不能超过 ${MAX_TEMPLATE_CHARS} 个字`
  }
  return ''
}

/** 新增预设的默认名：`新预设`、`新预设 2`… */
export function suggestNewName(presets = []) {
  const taken = new Set(presets.map((preset) => preset.name))
  for (let index = 1; ; index += 1) {
    const candidate = index === 1 ? '新预设' : `新预设 ${index}`
    if (!taken.has(candidate)) {
      return candidate
    }
  }
}

/** 录音默认预设：id 仍存在就用它，否则回退「通用优化」，再否则第一个；一个都没有返回空串 */
export function resolveDefaultPresetId(id, presets = []) {
  if (id && presets.some((preset) => preset.id === id)) {
    return id
  }
  return presets.find((preset) => preset.name === DEFAULT_PRESET_NAME && preset.is_builtin !== false)?.id ||
    presets[0]?.id ||
    ''
}

/** 「优化选中文本」预设：id 不存在（被删）时回退空串 = 跟随默认预设 */
export function resolveRewritePresetId(id, presets = []) {
  return id && presets.some((preset) => preset.id === id) ? id : ''
}

/** 删除确认文案：被选用时写明删除后会回退到哪里 */
export function describeDeleteImpact(preset, { defaultPresetId = '', rewritePresetId = '' } = {}, presets = []) {
  if (!preset) {
    return ''
  }
  const uses = []
  if (preset.id === defaultPresetId) {
    const remaining = presets.filter((item) => item.id !== preset.id)
    const fallback = remaining.find((item) => item.id === resolveDefaultPresetId('', remaining))
    uses.push(`录音默认预设将改为「${fallback?.name || DEFAULT_PRESET_NAME}」`)
  }
  if (preset.id === rewritePresetId) {
    uses.push('「优化选中文本」将改为跟随默认预设')
  }
  const tail = uses.length ? `它正被使用：${uses.join('；')}。` : ''
  return `删除后无法恢复。${tail}`
}
