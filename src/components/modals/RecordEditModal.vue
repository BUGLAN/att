<template>
  <Modal title="编辑记录" modal-name="recordEdit">
    <div class="form-group">
      <label>原始文本</label>
      <div class="record-edit-display">{{ record?.raw_text || '无' }}</div>
    </div>

    <div class="form-group">
      <label>优化后文本</label>
      <div v-if="record?.optimized_text" class="record-edit-display optimized-text">
        {{ record.optimized_text }}
      </div>
      <div v-else class="record-edit-display">暂无优化文本</div>
    </div>

    <div class="form-group">
      <label>编辑文本</label>
      <textarea
        id="recordEditText"
        class="form-textarea"
        v-model="editText"
        placeholder="输入文本..."
      ></textarea>
    </div>

    <template #footer>
      <button class="btn btn-secondary" @click="close">取消</button>
      <button class="btn btn-primary" @click="save">保存</button>
    </template>
  </Modal>
</template>

<script setup>
import { ref, watch, computed } from 'vue'
import { useAppStore } from '@/stores/app'
import { useRecordsStore } from '@/stores/records'
import Modal from '@/components/common/Modal.vue'

const appStore = useAppStore()
const recordsStore = useRecordsStore()

const record = computed(() => appStore.currentRecord)
const editText = ref('')

// 监听记录变化
watch(record, (newRecord) => {
  if (newRecord) {
    editText.value = newRecord.optimized_text || newRecord.raw_text || ''
  }
}, { immediate: true })

function close() {
  appStore.closeModal('recordEdit')
}

async function save() {
  if (!record.value) return

  try {
    await recordsStore.updateRecord(record.value.id, {
      raw_text: editText.value,
      is_modified: true
    })
    appStore.showToast('已保存')
    close()
  } catch (error) {
    appStore.showToast('保存失败')
  }
}
</script>
