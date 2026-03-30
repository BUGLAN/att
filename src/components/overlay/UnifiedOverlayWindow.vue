<template>
  <div class="unified-overlay-root" :class="`mode-${mode}`">
    <SelectionPanel
      v-if="mode === 'selection'"
      :activation="activation"
      :pinned="isPinned"
      @close="handleClose"
      @pin="handlePin"
      @action-complete="handleActionComplete"
    />
    <ClipboardPanel
      v-else
      :activation="activation"
      :pinned="isPinned"
      @close="handleClose"
      @pin="handlePin"
    />
  </div>
</template>

<script setup>
import { nextTick, onMounted, onUnmounted, ref } from 'vue'
import SelectionPanel from '@/components/overlay/SelectionPanel.vue'
import ClipboardPanel from '@/components/overlay/ClipboardPanel.vue'
import { useElectron } from '@/composables/useElectron'

const {
  overlay,
  onOverlayActivate,
  onOverlayPinnedChanged,
  removeAllListeners
} = useElectron()

const mode = ref('selection')
const activation = ref({
  mode: 'selection',
  sequence: 0,
  text: ''
})
const isPinned = ref(false)

function notifyReadyAfterPaint() {
  nextTick(() => {
    if (typeof requestAnimationFrame === 'function') {
      requestAnimationFrame(() => {
        overlay.notifyReady()
      })
      return
    }

    overlay.notifyReady()
  })
}

function handleClose() {
  overlay.hide()
}

function handlePin(nextPinned) {
  overlay.setPinned(nextPinned)
}

function handleActionComplete(payload = {}) {
  overlay.notifyActionComplete({
    mode: mode.value,
    ...payload
  })
}

onMounted(() => {
  onOverlayActivate((payload = {}) => {
    mode.value = payload.mode || 'selection'
    activation.value = payload
    notifyReadyAfterPaint()
  })

  onOverlayPinnedChanged((payload = {}) => {
    isPinned.value = payload.pinned === true
  })
})

onUnmounted(() => {
  removeAllListeners('overlay:activate')
  removeAllListeners('overlay:pinned-changed')
})
</script>

<style scoped>
.unified-overlay-root {
  width: 100vw;
  height: 100vh;
}
</style>
