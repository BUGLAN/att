import assert from 'node:assert/strict'
import test from 'node:test'

import { modelsDirectoryView } from '../src/components/settings/modelsDirectory.mjs'

test('模型目录：环境变量优先时显示实际目录并禁用更改', () => {
  const view = modelsDirectoryView({
    savedDir: 'D:/saved',
    draftDir: 'D:/draft',
    effectiveDir: 'E:/env-models',
    nextDir: 'E:/env-models',
    envOverride: true,
    desktop: true
  })

  assert.equal(view.path, 'E:/env-models')
  assert.equal(view.message, '已由环境变量 ATT_MODELS_DIR 指定')
  assert.equal(view.canChange, false)
  assert.equal(view.canReset, false)
  assert.equal(view.restartPending, false)
})

test('模型目录：选择后先提示保存，服务端保存后提示重启', () => {
  const saving = modelsDirectoryView({
    savedDir: 'C:/old',
    draftDir: 'D:/new',
    effectiveDir: 'C:/old',
    nextDir: 'C:/old',
    desktop: true
  })
  assert.equal(saving.path, 'D:/new')
  assert.equal(saving.message, '正在保存模型目录…')
  assert.equal(saving.pendingSave, true)
  assert.equal(saving.canRestart, false)

  const saved = modelsDirectoryView({
    savedDir: 'D:/new',
    draftDir: 'D:/new',
    effectiveDir: 'C:/old',
    nextDir: 'D:/new',
    desktop: true
  })
  assert.equal(saved.path, 'D:/new')
  assert.equal(saved.message, '已保存，重启应用后生效。旧模型不会自动搬移，可手动移动或在新目录重新下载')
  assert.equal(saved.restartPending, true)
  assert.equal(saved.canRestart, true)
})

test('模型目录：恢复默认也按下次启动目标展示，浏览器模式不可更改', () => {
  const reset = modelsDirectoryView({
    savedDir: '',
    draftDir: '',
    effectiveDir: 'D:/custom',
    nextDir: 'C:/app-data/models',
    desktop: true
  })
  assert.equal(reset.path, 'C:/app-data/models')
  assert.equal(reset.restartPending, true)

  const web = modelsDirectoryView({
    savedDir: '',
    draftDir: '',
    effectiveDir: 'C:/app-data/models',
    nextDir: 'C:/app-data/models',
    desktop: false
  })
  assert.equal(web.canChange, false)
  assert.equal(web.message, '可在桌面版中选择其他目录')
})
