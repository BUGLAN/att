import assert from 'node:assert/strict'

import { cudaRequested, cudnnRequested, withCudaFeatures } from '../scripts/run-tauri.mjs'

// 默认构建：不追加任何 feature，保持 CPU（int8）
assert.deepEqual(withCudaFeatures(['dev'], {}), ['dev'])
assert.deepEqual(withCudaFeatures(['build', '--no-bundle'], { ATT_CUDA: '0' }), ['build', '--no-bundle'])
assert.deepEqual(withCudaFeatures(['dev'], { ATT_CUDA: 'false' }), ['dev'])
assert.equal(cudaRequested({}), false)
assert.equal(cudnnRequested({}), false)

// 便携 CUDA：只开 cuda（cuda-dynamic-loading，运行期按需加载 cuBLAS）
for (const value of ['1', 'true', 'on', 'yes', 'TRUE']) {
  assert.deepEqual(withCudaFeatures(['dev'], { ATT_CUDA: value }), ['dev', '--features', 'cuda'], value)
}

// cuDNN 必须叠加在 CUDA 上；ATT_CUDA=cudnn 是两者的简写
assert.deepEqual(withCudaFeatures(['build'], { ATT_CUDA: '1', ATT_CUDNN: '1' }), [
  'build',
  '--features',
  'cuda,cuda-cudnn'
])
assert.deepEqual(withCudaFeatures(['build'], { ATT_CUDA: 'cudnn' }), ['build', '--features', 'cuda,cuda-cudnn'])
assert.deepEqual(withCudaFeatures(['build'], { ATT_CUDNN: '1' }), ['build', '--features', 'cuda-cudnn'])

// 已有参数原样保留，feature 追加在最后（tauri CLI 两种顺序都认，放在最后不干扰用户参数）
assert.deepEqual(withCudaFeatures(['dev', '--no-watch'], { ATT_CUDA: '1' }), [
  'dev',
  '--no-watch',
  '--features',
  'cuda'
])

console.log('[tauri-cuda] 全部断言通过')
