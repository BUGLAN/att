<template>
  <Modal :title="isEdit ? '编辑预设' : '新建预设'" modal-name="presetEdit">
    <div class="form-group">
      <label>名称</label>
      <input
        type="text"
        class="form-input"
        v-model="form.name"
        placeholder="预设名称"
      >
    </div>

    <div class="form-group">
      <label>分类</label>
      <select class="form-select" v-model="form.category">
        <option value="general">通用</option>
        <option value="formal">正式</option>
        <option value="casual">口语</option>
        <option value="technical">技术</option>
        <option value="custom">自定义</option>
      </select>
    </div>

    <div class="form-group">
      <label>提示词</label>
      <textarea
        class="form-textarea"
        v-model="form.prompt"
        placeholder="输入优化提示词..."
        rows="6"
      ></textarea>
    </div>

    <template #footer>
      <button class="btn btn-secondary" @click="close">取消</button>
      <button class="btn btn-primary" @click="save">保存</button>
    </template>
  </Modal>
</template>

<script setup>
import { ref, computed, watch } from 'vue'
import { useAppStore } from '@/stores/app'
import { useSettingsStore } from '@/stores/settings'
import Modal from '@/components/common/Modal.vue'
import { usePresets } from '@/composables/useSettings'

const appStore = useAppStore()
const settingsStore = useSettingsStore()
const { savePreset } = usePresets()

const isEdit = computed(() => !!settingsStore.currentPreset?.id)

const form = ref({
  name: '',
  category: 'general',
  prompt: ''
})

// 监听当前预设
watch(() => settingsStore.currentPreset, (preset) => {
  if (preset) {
    form.value = {
      name: preset.name || '',
      category: preset.category || 'general',
      prompt: preset.prompt || ''
    }
  } else {
    form.value = {
      name: '',
      category: 'general',
      prompt: ''
    }
  }
}, { immediate: true })

function close() {
  appStore.closeModal('presetEdit')
  appStore.openModal('preset')
}

async function save() {
  if (!form.value.name.trim()) {
    appStore.showToast('请输入预设名称')
    return
  }

  try {
    await savePreset({
      id: settingsStore.currentPreset?.id,
      ...form.value
    })
    close()
  } catch (error) {
    appStore.showToast('保存失败')
  }
}
</script>
