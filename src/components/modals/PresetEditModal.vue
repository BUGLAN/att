<template>
  <Modal title="查看提示词" modal-name="presetEdit">
    <div v-if="preset" class="preset-viewer">
      <div class="form-group">
        <label>名称</label>
        <input :value="preset.name" class="form-input" type="text" readonly>
      </div>

      <div class="form-group">
        <label>描述</label>
        <input :value="preset.description || '无描述'" class="form-input" type="text" readonly>
      </div>

      <div class="form-group">
        <label>提示词内容</label>
        <textarea
          :value="preset.prompt_template || ''"
          class="form-textarea"
          rows="14"
          readonly
        ></textarea>
      </div>
    </div>

    <div v-else class="manager-empty">未找到提示词内容</div>

    <template #footer>
      <button class="btn btn-secondary" type="button" @click="close">关闭</button>
    </template>
  </Modal>
</template>

<script setup>
import { computed } from 'vue'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'
import Modal from '@/components/common/Modal.vue'

const appStore = useAppStore()
const settingsStore = useSettingsStore()
const preset = computed(() => settingsStore.currentPreset || null)

function close() {
  appStore.closeModal('presetEdit')
}
</script>
