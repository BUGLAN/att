<template>
  <div :id="modalId" class="modal active" @click.self="close">
    <div class="modal-content">
      <div class="modal-header">
        <h3>{{ title }}</h3>
        <button class="modal-close" type="button" @click="close">×</button>
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
import { computed } from 'vue'
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
const modalId = computed(() => `${props.modalName}Modal`)

function close() {
  appStore.closeModal(props.modalName)
}
</script>
