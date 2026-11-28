// 本地模型的「运行设备」选项与提示（纯逻辑，供设置页与单测使用）。
//
// 平台加速能力：
// - faster-whisper 走 CTranslate2：macOS 无 CUDA / Metal 后端，只有 CPU（Apple Accelerate）；
//   其它平台在 CUDA 版构建下可用 GPU。
// - Qwen3-ASR / SenseVoice 走 sherpa-onnx（ONNX Runtime）：auto 固定走 CPU，macOS 可显式选择 CoreML，
//   Windows / Linux 可显式选择 CUDA。
// 见 att-qwen/vendor/c-api.h 中 provider 取值（cpu / cuda / coreml）。

const DEVICE_LABELS = {
  auto: '自动',
  cpu: 'CPU',
  cuda: 'GPU（CUDA）',
  coreml: 'CoreML（Apple GPU）'
}

export function isMacPlatform(platform) {
  return String(platform || '').toLowerCase() === 'darwin'
}

// 某个本地引擎在指定平台可选的设备项（顺序即展示顺序，auto 在前）。
export function deviceOptionsFor(kind, platform) {
  const options = [
    { value: 'auto', label: DEVICE_LABELS.auto },
    { value: 'cpu', label: DEVICE_LABELS.cpu }
  ]
  const mac = isMacPlatform(platform)
  if (kind === 'whisper') {
    // macOS 无 CUDA，只到 CPU 为止
    if (!mac) {
      options.push({ value: 'cuda', label: DEVICE_LABELS.cuda })
    }
    return options
  }
  // Qwen3-ASR / SenseVoice（sherpa-onnx）
  options.push(
    mac
      ? { value: 'coreml', label: DEVICE_LABELS.coreml }
      : { value: 'cuda', label: DEVICE_LABELS.cuda }
  )
  return options
}

// 设备选择项的说明文案。
export function deviceHintFor(kind, platform) {
  const mac = isMacPlatform(platform)
  if (kind === 'whisper') {
    return mac
      ? '自动 = CPU（macOS 无 CUDA，走 Apple Accelerate）。'
      : '自动：有可用 CUDA 时用 GPU，否则 CPU；GPU 需 CUDA 版构建（ATT_CUDA=1）与 NVIDIA 显卡，缺省回退 CPU。'
  }
  return mac
    ? '自动 = CPU；需要加速请显式选择 CoreML（Apple GPU / 神经引擎）。'
    : '自动 = CPU；需要 GPU 加速请显式选择 GPU（CUDA）。'
}

// 归一设备值：别名映射 + 平台不支持的值回落到 auto。
export function normalizeDeviceValue(kind, platform, value) {
  const allowed = deviceOptionsFor(kind, platform).map((option) => option.value)
  let normalized = String(value == null ? '' : value).trim().toLowerCase()
  if (normalized === 'gpu') normalized = 'cuda'
  if (normalized === 'metal' || normalized === 'apple') normalized = 'coreml'
  return allowed.includes(normalized) ? normalized : 'auto'
}
