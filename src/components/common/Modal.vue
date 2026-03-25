<template>
  <div class="modal" :class="{ active: true }" @click.self="close">
    <div class="modal-content">
      <div class="modal-header">
        <h3>{{ title }}</h3>
        <button class="modal-close" @click="close">×</button>
      </div>
      <div class="modal-body">
        <slot></slot>
      </div>
      <div v-if="$slots.footer" class="modal-footer">
        <slot name="footer"></slot>
      </div>
    </div>
  </div>
</template>

<script setup>
import { useAppStore } from '@/stores/app'

const props = defineProps({
  title: {
    type: String,
    required: true
  },
  modalName: {
    type: String,
    required: true
  }
})

const appStore = useAppStore()

function close() {
  appStore.closeModal(props.modalName)
}
</script>
