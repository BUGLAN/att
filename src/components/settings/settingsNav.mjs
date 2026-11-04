// 设置页二级导航的纯模型：分组、页面元数据与搜索匹配（不依赖 Vue，供 tests/test_settings_nav.mjs 锁定）。
//
// 设置项按「用户想做什么」分页，而不是按实现模块：一页只放一类事情，
// 引擎专属配置只在选中该引擎时出现（见 SettingsAsrPage.vue）。

export const SETTINGS_GROUPS = Object.freeze(['通用', '语音识别', 'AI', '系统'])

export const SETTINGS_SECTIONS = Object.freeze([
  {
    id: 'recording',
    group: '通用',
    label: '录音',
    desc: '输入设备与录音期间的系统行为。',
    keywords: ['麦克风', '输入设备', '暂停其他音频', '静音']
  },
  {
    id: 'hotkey',
    group: '通用',
    label: '快捷键',
    desc: '全局录音与优化选中文本的快捷键，窗口在后台时同样生效。',
    keywords: ['快捷键', '热键', '按住', '组合键', 'hotkey', '选中文本', '改写']
  },
  {
    id: 'clipboard',
    group: '通用',
    label: '剪贴板',
    desc: '剪贴板悬浮框：唤起快捷键、选中条目后的动作、尺寸与位置。',
    keywords: ['剪贴板', '剪切板', '悬浮框', '剪贴板快捷键', '粘贴', '复制', '尺寸', '高度', '宽度', '位置']
  },
  {
    id: 'output',
    group: '通用',
    label: '输出与优化',
    desc: '转写完成后自动优化、复制与粘贴；优化选中文本用的预设。',
    keywords: ['自动优化', 'ai 优化', '预设', '自动复制', '粘贴', '剪贴板', '选中文本', '提示词']
  },
  {
    id: 'asr',
    group: '语音识别',
    label: '识别引擎',
    desc: '选择识别引擎与录音模式；下方只显示当前引擎需要的配置。',
    keywords: ['引擎', 'asr', '豆包', 'token', 'resource id', '接入点', 'chatgpt', '令牌', 'whisper', 'qwen', '识别语言', '语言', '录音模式', '单段', '多段', '流式', '静音分段', '阈值', '本地模型', '加载']
  },
  {
    id: 'llm',
    group: 'AI',
    label: 'LLM 服务',
    desc: '供应商档案与模型，决定「AI 优化」与文件校对用哪个模型。',
    keywords: ['llm', '供应商', '模型', 'deepseek', 'openai', 'api key', '默认模型', '备用模型', '思考强度']
  },
  {
    id: 'presets',
    group: 'AI',
    label: '提示词预设',
    desc: '优化时使用的提示词，来自 prompts/ 目录。',
    keywords: ['提示词', '预设', 'prompt', '默认预设']
  },
  {
    id: 'permissions',
    group: '系统',
    label: '权限',
    desc: '录音、快捷键与自动粘贴依赖的系统授权。',
    keywords: ['权限', '授权', '麦克风权限', '输入监控', '辅助功能']
  },
  {
    id: 'diagnostics',
    group: '系统',
    label: '诊断与关于',
    desc: '服务状态、数据目录、日志与版本。',
    keywords: ['诊断', '后端', '连接', '数据目录', '日志', '版本', '关于']
  }
])

export const DEFAULT_SECTION_ID = 'recording'

export function findSection(id) {
  return SETTINGS_SECTIONS.find((section) => section.id === id) || null
}

/** 合法的页面 id（未知值回落默认页，用于恢复上次停留页或外部跳转）。 */
export function resolveSectionId(id) {
  return findSection(id) ? id : DEFAULT_SECTION_ID
}

/** 搜索匹配：标题 / 描述 / 关键词，大小写不敏感、忽略首尾空白；空查询匹配全部。 */
export function sectionMatches(section, query) {
  const needle = String(query || '').trim().toLowerCase()
  if (!needle) {
    return true
  }
  return [section.label, section.desc, ...section.keywords].some((text) => text.toLowerCase().includes(needle))
}

/**
 * 按分组组织导航；带查询时只保留命中的页面并去掉空分组。
 * 返回 [{ label, items: [section] }]，顺序与 SETTINGS_GROUPS / SETTINGS_SECTIONS 一致。
 */
export function buildNavGroups(query = '') {
  return SETTINGS_GROUPS
    .map((label) => ({
      label,
      items: SETTINGS_SECTIONS.filter((section) => section.group === label && sectionMatches(section, query))
    }))
    .filter((group) => group.items.length > 0)
}
