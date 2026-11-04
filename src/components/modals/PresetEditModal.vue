<template>
  <!--
    提示词预设弹窗，三种模式（settingsStore.presetEditor.mode）：
    - view：查看内置预设（只读，prompts/*.txt）
    - edit：编辑自定义预设
    - create：新增自定义预设（含复制而来的）
  -->
  <Modal :title="title" modal-name="presetEdit" :close-on-backdrop="isView">
    <div v-if="editor" class="preset-editor">
      <p v-if="isView" class="preset-editor__note">
        内置预设随安装包发布，升级时会更新，只能查看，不能修改。
      </p>

      <div class="form-group">
        <label for="presetEditName">名称</label>
        <BaseInput
          id="presetEditName"
          v-model="draft.name"
          type="text"
          :readonly="isView"
          :maxlength="MAX_NAME_CHARS"
          placeholder="例如：周报整理"
        />
      </div>

      <div class="form-group">
        <label for="presetEditTemplate">提示词内容</label>
        <BaseTextarea
          id="presetEditTemplate"
          v-model="draft.prompt_template"
          :rows="14"
          :readonly="isView"
          placeholder="写给模型的整理规则，例如：把口述内容整理成条理清晰的周报，保留所有事实和数字，不要编造。"
        />
        <p v-if="!isView" class="preset-editor__hint">
          识别出的原文会自动作为待处理文本附在后面，不需要写占位符；也兼容旧式的 {text} 写法。
        </p>
      </div>

      <p v-if="errorText" class="preset-editor__error" role="alert">{{ errorText }}</p>
    </div>

    <EmptyState v-else size="sm" title="未找到提示词内容" />

    <template #footer>
      <template v-if="isView">
        <BaseButton variant="outline" @click="close">关闭</BaseButton>
      </template>
      <template v-else>
        <BaseButton variant="outline" :disabled="saving" @click="close">取消</BaseButton>
        <BaseButton variant="solid" :loading="saving" @click="save">保存</BaseButton>
      </template>
    </template>
  </Modal>
</template>

<script setup>
import { computed, reactive, ref, watch } from 'vue'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'
import Modal from '@/components/common/Modal.vue'
import BaseButton from '@/components/common/BaseButton.vue'
import BaseInput from '@/components/common/BaseInput.vue'
import BaseTextarea from '@/components/common/BaseTextarea.vue'
import EmptyState from '@/components/common/EmptyState.vue'
import { MAX_NAME_CHARS, validatePresetDraft } from '@/components/settings/presetLogic.mjs'

const appStore = useAppStore()
const settingsStore = useSettingsStore()
const editor = computed(() => settingsStore.presetEditor || null)
const isView = computed(() => editor.value?.mode === 'view')
const title = computed(() => ({
  view: '查看内置预设',
  edit: '编辑预设',
  create: '新增预设'
})[editor.value?.mode] || '提示词预设')

const draft = reactive({ name: '', prompt_template: '' })
const errorText = ref('')
const saving = ref(false)

// 打开 / 切换模式时用 editor 里的初始值重置草稿
watch(editor, (value) => {
  draft.name = value?.name || ''
  draft.prompt_template = value?.prompt_template || ''
  errorText.value = ''
}, { immediate: true })

// 用户改动后清掉旧的错误提示
watch(draft, () => {
  errorText.value = ''
})

function close() {
  appStore.closeModal('presetEdit')
}

async function save() {
  const editingId = editor.value?.mode === 'edit' ? editor.value.preset?.id : ''
  const message = validatePresetDraft(draft, settingsStore.presets, editingId)
  if (message) {
    errorText.value = message
    return
  }
  const payload = { name: draft.name.trim(), prompt_template: draft.prompt_template.trim() }
  saving.value = true
  try {
    if (editingId) {
      await settingsStore.updatePreset(editingId, payload)
      appStore.showToast(`已保存预设「${payload.name}」`, { tone: 'success' })
    } else {
      await settingsStore.createPreset(payload)
      appStore.showToast(`已新增预设「${payload.name}」`, { tone: 'success' })
    }
    close()
  } catch (error) {
    // 后端校验（重名 / 文件损坏等）原样展示在弹窗里，不关窗，草稿不丢
    errorText.value = String(error?.message || error)
  } finally {
    saving.value = false
  }
}
</script>

<style scoped>
.preset-editor__note {
  margin: 0 0 var(--s-4);
  color: var(--text-2);
  font-size: var(--fs-sm);
  line-height: 1.6;
}

.preset-editor__hint {
  margin: var(--s-2) 0 0;
  color: var(--text-3);
  font-size: var(--fs-cap);
}

.preset-editor__error {
  margin: var(--s-3) 0 0;
  color: var(--err);
  font-size: var(--fs-sm);
}
</style>
