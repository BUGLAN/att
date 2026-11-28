<template>
  <Teleport to="body">
    <div v-if="modelValue" class="backup-dialog" @click.self="cancel" @keydown="handleKeydown">
      <div ref="dialogRef" class="backup-dialog__panel" role="dialog" aria-modal="true" aria-labelledby="backupExportTitle">
        <h3 id="backupExportTitle" class="backup-dialog__title">导出数据备份</h3>
        <p class="backup-dialog__desc">
          打包设置、记录、录音、剪贴板、提示词预设和 LLM 配置。运行日志与临时文件不会进入备份。
        </p>

        <fieldset class="backup-fieldset">
          <legend class="backup-legend">加密密钥</legend>
          <label class="backup-option">
            <input v-model="keyMode" type="radio" value="omit">
            <span>
              <strong>不含密钥（推荐）</strong>
              <small>恢复后需要重新填写识别凭据和 LLM 密钥。</small>
            </span>
          </label>
          <label class="backup-option">
            <input v-model="keyMode" type="radio" value="include">
            <span>
              <strong>包含密钥</strong>
              <small>恢复后凭据可直接使用。备份文件须妥善保管，任何人拿到都能解密其中的密钥。</small>
            </span>
          </label>
        </fieldset>

        <label class="backup-check">
          <input v-model="includeAudio" type="checkbox">
          <span>包含录音音频（体积较大；取消后恢复的记录将无法回放音频）</span>
        </label>

        <label v-if="keyMode === 'include'" class="backup-check backup-check--warning">
          <input v-model="includeKeyConfirmed" type="checkbox">
          <span>我理解：这个 zip 中的密钥未额外加密，拿到备份文件的人可以解开全部已保存凭据。</span>
        </label>

        <div class="backup-dialog__actions">
          <BaseButton variant="outline" @click="cancel">取消</BaseButton>
          <BaseButton
            variant="solid"
            :disabled="keyMode === 'include' && !includeKeyConfirmed"
            @click="confirm"
          >
            选择保存位置
          </BaseButton>
        </div>
      </div>
    </div>
  </Teleport>
</template>

<script setup>
import { nextTick, onBeforeUnmount, ref, watch } from 'vue'
import BaseButton from '@/components/common/BaseButton.vue'

const props = defineProps({
  modelValue: {
    type: Boolean,
    default: false
  }
})

const emit = defineEmits(['update:modelValue', 'confirm'])
const dialogRef = ref(null)
const keyMode = ref('omit')
const includeAudio = ref(true)
const includeKeyConfirmed = ref(false)
let lastActiveElement = null

function close() {
  emit('update:modelValue', false)
}

function cancel() {
  close()
}

function confirm() {
  if (keyMode.value === 'include' && !includeKeyConfirmed.value) {
    return
  }
  emit('confirm', {
    include_key: keyMode.value === 'include',
    include_audio: includeAudio.value
  })
  close()
}

function handleKeydown(event) {
  if (event.key === 'Escape') {
    event.preventDefault()
    cancel()
    return
  }
  if (event.key !== 'Tab' || !dialogRef.value) {
    return
  }
  const focusables = Array.from(
    dialogRef.value.querySelectorAll(
      'button:not(:disabled), input:not(:disabled), [tabindex]:not([tabindex="-1"])'
    )
  )
  if (!focusables.length) {
    return
  }
  const first = focusables[0]
  const last = focusables[focusables.length - 1]
  const active = document.activeElement
  if (event.shiftKey && (active === first || !dialogRef.value.contains(active))) {
    event.preventDefault()
    last.focus()
  } else if (!event.shiftKey && active === last) {
    event.preventDefault()
    first.focus()
  }
}

watch(() => props.modelValue, (visible) => {
  if (visible) {
    keyMode.value = 'omit'
    includeAudio.value = true
    includeKeyConfirmed.value = false
    lastActiveElement = document.activeElement
    nextTick(() => dialogRef.value?.querySelector('button')?.focus())
    return
  }
  if (lastActiveElement?.focus) {
    lastActiveElement.focus()
  }
  lastActiveElement = null
})

onBeforeUnmount(() => {
  lastActiveElement = null
})
</script>

<style scoped>
.backup-dialog {
  position: fixed;
  inset: 0;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: var(--s-6);
  background: var(--scrim-soft);
  backdrop-filter: blur(6px);
  z-index: var(--z-modal);
}

.backup-dialog__panel {
  width: min(520px, 100%);
  max-height: calc(100dvh - 2 * var(--s-6));
  overflow-y: auto;
  overscroll-behavior: contain;
  padding: var(--s-6);
  border: 1px solid var(--line);
  border-radius: var(--r-md);
  background: var(--bg-surface);
  box-shadow: var(--e-3), var(--edge-hi);
}

.backup-dialog__title {
  margin: 0 0 var(--s-2);
  color: var(--text-1);
  font-size: var(--fs-md);
  font-weight: 600;
}

.backup-dialog__desc {
  margin: 0 0 var(--s-4);
  color: var(--text-2);
  font-size: var(--fs-sm);
  line-height: 1.7;
}

.backup-fieldset {
  display: grid;
  gap: var(--s-3);
  margin: 0 0 var(--s-4);
  padding: 0;
  border: 0;
}

.backup-legend {
  margin-bottom: var(--s-2);
  padding: 0;
  color: var(--text-2);
  font-size: var(--fs-sm);
  font-weight: 600;
}

.backup-option,
.backup-check {
  display: flex;
  align-items: flex-start;
  gap: var(--s-3);
  color: var(--text-2);
  font-size: var(--fs-sm);
  line-height: 1.55;
  cursor: pointer;
}

.backup-option input,
.backup-check input {
  flex: none;
  margin-top: 3px;
  accent-color: var(--accent);
}

.backup-option span {
  display: grid;
  gap: 2px;
}

.backup-option strong {
  color: var(--text-1);
  font-weight: 600;
}

.backup-option small {
  color: var(--text-3);
  font-size: var(--fs-cap);
}

.backup-check {
  margin-bottom: var(--s-6);
}

.backup-check--warning {
  margin-top: calc(-1 * var(--s-3));
  color: var(--warn);
}

.backup-dialog__actions {
  display: flex;
  justify-content: flex-end;
  gap: var(--s-2);
}
</style>
