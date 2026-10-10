<template>
  <Modal title="编辑记录" modal-name="recordEdit">
    <div class="form-group">
      <label>原始文本</label>
      <div class="record-edit-display copyable" title="点击复制" @click="copyField(record?.raw_text)">
        {{ record?.raw_text || '暂无内容' }}
      </div>
    </div>

    <div class="form-group">
      <label>优化文本</label>
      <div class="record-edit-display optimized-text copyable" title="点击复制" @click="copyField(record?.optimized_text)">
        {{ record?.optimized_text || '暂无优化文本' }}
      </div>
    </div>

    <div class="form-group">
      <label>编辑内容</label>
      <BaseTextarea id="recordEditText" v-model="editText" placeholder="输入文本..." />
    </div>

    <template #footer>
      <BaseButton variant="outline" @click="close">取消</BaseButton>
      <BaseButton variant="solid" @click="save">保存</BaseButton>
    </template>
  </Modal>
</template>

<script setup>
import { computed, ref, watch } from 'vue'
import { useAppStore } from '@/stores/app'
import { useRecordsStore } from '@/stores/records'
import { useElectron } from '@/composables/useElectron'
import Modal from '@/components/common/Modal.vue'
import BaseButton from '@/components/common/BaseButton.vue'
import BaseTextarea from '@/components/common/BaseTextarea.vue'

const appStore = useAppStore()
const recordsStore = useRecordsStore()
const { clipboard } = useElectron()

const record = computed(() => appStore.currentRecord)
const editText = ref('')

watch(record, (newRecord) => {
  if (!newRecord) {
    editText.value = ''
    return
  }

  editText.value = newRecord.optimized_text || newRecord.raw_text || ''
}, { immediate: true })

function close() {
  appStore.closeModal('recordEdit')
}

async function copyField(text) {
  if (!text) return
  await clipboard.writeText(text)
  appStore.showToast('已复制到剪贴板', { tone: 'success' })
}

async function save() {
  if (!record.value) {
    return
  }

  const payload = record.value.optimized_text
    ? { optimized_text: editText.value, is_modified: true }
    : { raw_text: editText.value, is_modified: true }

  try {
    const updated = await recordsStore.updateRecord(record.value.id, payload)
    appStore.setCurrentRecord({ ...record.value, ...updated })
    appStore.showToast('记录已保存', { tone: 'success' })
    close()
  } catch (error) {
    appStore.showToast('保存记录失败', { tone: 'error' })
  }
}
</script>
