<template>
  <Modal title="查看提示词" modal-name="presetEdit">
    <div v-if="preset" class="preset-viewer">
      <div class="form-group">
        <label>名称</label>
        <BaseInput :value="preset.name" type="text" readonly />
      </div>

      <div class="form-group">
        <label>描述</label>
        <BaseInput :value="preset.description || '无描述'" type="text" readonly />
      </div>

      <div class="form-group">
        <label>提示词内容</label>
        <BaseTextarea
          :value="preset.prompt_template || ''"
          :rows="14"
          readonly
        />
      </div>
    </div>

    <EmptyState v-else size="sm" title="未找到提示词内容" />

    <template #footer>
      <BaseButton variant="outline" @click="close">关闭</BaseButton>
    </template>
  </Modal>
</template>

<script setup>
import { computed } from 'vue'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'
import Modal from '@/components/common/Modal.vue'
import BaseButton from '@/components/common/BaseButton.vue'
import BaseInput from '@/components/common/BaseInput.vue'
import BaseTextarea from '@/components/common/BaseTextarea.vue'
import EmptyState from '@/components/common/EmptyState.vue'

const appStore = useAppStore()
const settingsStore = useSettingsStore()
const preset = computed(() => settingsStore.currentPreset || null)

function close() {
  appStore.closeModal('presetEdit')
}
</script>
