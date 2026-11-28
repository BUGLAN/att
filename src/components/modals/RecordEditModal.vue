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
import { ref, watch } from 'vue'
import { useAppStore } from '@/stores/app'
import { useRecordsStore } from '@/stores/records'
import { useElectron } from '@/composables/useElectron'
import Modal from '@/components/common/Modal.vue'
import BaseButton from '@/components/common/BaseButton.vue'
import BaseTextarea from '@/components/common/BaseTextarea.vue'
import { recordEditText, resolveRecordEditSync } from './recordEditModel.mjs'

const appStore = useAppStore()
const recordsStore = useRecordsStore()
const { clipboard } = useElectron()

// 弹窗打开后固定编辑这条记录。currentRecord 可能因新录音、优化或重新识别而切换。
const recordId = ref(appStore.currentRecord?.id || null)
const record = ref(appStore.currentRecord)
const editText = ref(recordEditText(record.value))
const baselineText = ref(editText.value)

watch(() => appStore.currentRecord, (newRecord) => {
  const synced = resolveRecordEditSync({
    recordId: recordId.value,
    editedText: editText.value,
    baselineText: baselineText.value,
    nextRecord: newRecord
  })
  if (!synced) return

  record.value = synced.record
  editText.value = synced.text
  baselineText.value = synced.baselineText
})

function close() {
  appStore.closeModal('recordEdit')
}

async function copyField(text) {
  if (!text) return
  await clipboard.writeText(text)
  appStore.showToast('已复制到剪贴板', { tone: 'success' })
}

async function save() {
  if (!record.value || !recordId.value) {
    return
  }

  const payload = record.value.optimized_text
    ? { optimized_text: editText.value, is_modified: true }
    : { raw_text: editText.value, is_modified: true }

  try {
    const updated = await recordsStore.updateRecord(recordId.value, payload)
    if (appStore.currentRecord?.id === recordId.value) {
      appStore.setCurrentRecord({ ...appStore.currentRecord, ...updated })
    }
    appStore.showToast('记录已保存', { tone: 'success' })
    close()
  } catch (error) {
    appStore.showToast('保存记录失败', { tone: 'error' })
  }
}
</script>
