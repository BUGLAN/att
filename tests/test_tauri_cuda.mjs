import assert from 'node:assert/strict'

import { cudaRequested, cudaToolkitAvailable, cudnnRequested, withCudaFeatures } from '../scripts/run-tauri.mjs'

// cudaToolkitAvailable：只看 CUDA_PATH / CUDA_HOME
assert.equal(cudaToolkitAvailable({}), false)
assert.equal(cudaToolkitAvailable({ CUDA_PATH: 'C:\\Program Files\\NVIDIA GPU Computing Toolkit\\CUDA\\v13.0' }), true)
assert.equal(cudaToolkitAvailable({ CUDA_HOME: '/usr/local/cuda' }), true)

// 默认（无 CUDA Toolkit、非 macOS）：不追加 feature，保持 CPU（int8）
assert.deepEqual(withCudaFeatures(['dev'], {}, 'win32'), ['dev'])
assert.equal(cudaRequested({}, 'win32'), false)
assert.equal(cudnnRequested({}), false)

// 默认（检测到 CUDA Toolkit、非 macOS）：自动启用 CUDA（设置里运行时再切 CPU/GPU，无 GPU 回退 CPU）
const withCuda = { CUDA_PATH: 'C:\\Program Files\\NVIDIA GPU Computing Toolkit\\CUDA\\v13.0' }
assert.equal(cudaRequested(withCuda, 'win32'), true)
assert.deepEqual(withCudaFeatures(['dev'], withCuda, 'win32'), ['dev', '--features', 'cuda'])
assert.deepEqual(withCudaFeatures(['build'], withCuda, 'linux'), ['build', '--features', 'cuda'])

// macOS：无论是否装 CUDA 都不启用（走 Accelerate）
assert.equal(cudaRequested(withCuda, 'darwin'), false)
assert.deepEqual(withCudaFeatures(['dev'], withCuda, 'darwin'), ['dev'])

// 显式关闭：ATT_CUDA=0 / off / ATT_CPU=1，即使装了 CUDA Toolkit 也保持 CPU
for (const env of [{ ...withCuda, ATT_CUDA: '0' }, { ...withCuda, ATT_CUDA: 'off' }, { ...withCuda, ATT_CPU: '1' }]) {
  assert.equal(cudaRequested(env, 'win32'), false)
  assert.deepEqual(withCudaFeatures(['dev'], env, 'win32'), ['dev'])
}

// 显式开启（无 CUDA Toolkit 也可，交给编译器报错或 CI 提供）：ATT_CUDA 真值
for (const value of ['1', 'true', 'on', 'yes', 'TRUE']) {
  assert.deepEqual(withCudaFeatures(['dev'], { ATT_CUDA: value }, 'win32'), ['dev', '--features', 'cuda'], value)
}

// cuDNN 必须叠加在 CUDA 上；ATT_CUDA=cudnn 是两者的简写
assert.deepEqual(withCudaFeatures(['build'], { ATT_CUDA: '1', ATT_CUDNN: '1' }, 'win32'), [
  'build', '--features', 'cuda,cuda-cudnn'
])
assert.deepEqual(withCudaFeatures(['build'], { ATT_CUDA: 'cudnn' }, 'win32'), ['build', '--features', 'cuda,cuda-cudnn'])
assert.deepEqual(withCudaFeatures(['build'], { ATT_CUDNN: '1' }, 'win32'), ['build', '--features', 'cuda-cudnn'])

// 已有参数原样保留，feature 追加在最后
assert.deepEqual(withCudaFeatures(['dev', '--no-watch'], { ATT_CUDA: '1' }, 'win32'), [
  'dev', '--no-watch', '--features', 'cuda'
])

console.log('[tauri-cuda] 全部断言通过')
