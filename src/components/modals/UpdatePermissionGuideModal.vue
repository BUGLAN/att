<template>
  <!--
    macOS 更新后首次启动的权限引导（只弹一次）：输入监控 / 辅助功能的授权跟着代码签名走，
    应用内更新后旧授权失效、系统设置里的旧条目却仍显示开启，引导用户删掉旧条目再重新添加。
    切回窗口时自动重新检测，都恢复了就自己关掉。
  -->
  <Modal title="更新后需要重新授权" modal-name="updatePermissionGuide">
    <div class="permission-guide">
      <p class="permission-guide__lead">{{ guide.versionText }}{{ UPDATE_PERMISSION_REASON }}</p>

      <div v-for="item in guide.items" :key="item.id" class="permission-guide__item">
        <div class="permission-guide__info">
          <span class="permission-guide__label">{{ item.label }}</span>
          <span class="permission-guide__impact">{{ item.impact }}</span>
        </div>
        <BaseButton variant="outline" size="sm" @click="openSettings(item.id)">打开系统设置</BaseButton>
      </div>

      <ol class="permission-guide__steps">
        <li v-for="step in UPDATE_PERMISSION_STEPS" :key="step">{{ step }}</li>
      </ol>
    </div>

    <template #footer>
      <BaseButton variant="outline" @click="dismiss">稍后再说</BaseButton>
      <BaseButton variant="solid" :loading="checking" @click="recheck">重新检测</BaseButton>
    </template>
  </Modal>
</template>

<script setup>
import { onBeforeUnmount, onMounted, ref } from 'vue'
import Modal from '@/components/common/Modal.vue'
import BaseButton from '@/components/common/BaseButton.vue'
import { useAppStore } from '@/stores/app'
import { useElectron } from '@/composables/useElectron'
import { useUpdatePermissionGuide } from '@/composables/useUpdatePermissionGuide'
import { UPDATE_PERMISSION_REASON, UPDATE_PERMISSION_STEPS } from '@/components/settings/permissionGuide.mjs'

const appStore = useAppStore()
const { openPermissionSettings } = useElectron()
const { guide, recheck: recheckGuide, dismiss } = useUpdatePermissionGuide()
const checking = ref(false)

async function openSettings(id) {
  try {
    await openPermissionSettings(id)
  } catch (error) {
    appStore.showToast(String(error?.message || error), { tone: 'error' })
  }
}

async function recheck({ quiet = false } = {}) {
  checking.value = true
  try {
    const restored = await recheckGuide()
    if (!restored && !quiet) {
      appStore.showToast('还没生效：按上面的步骤删掉旧条目、重新添加后再检测', { tone: 'info' })
    }
  } finally {
    checking.value = false
  }
}

// 用户去系统设置处理完切回窗口：自动检测一次
function handleFocus() {
  void recheck({ quiet: true })
}

onMounted(() => {
  window.addEventListener('focus', handleFocus)
})

onBeforeUnmount(() => {
  window.removeEventListener('focus', handleFocus)
  // × / Esc / 遮罩关闭也算看过：本次启动内不再提示
  void dismiss({ close: false })
})
</script>

<style scoped>
.permission-guide {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
}

.permission-guide__lead {
  margin: 0;
  font-size: var(--fs-sm);
  line-height: 1.7;
  color: var(--text-2);
}

.permission-guide__item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--s-3);
  padding: var(--s-3);
  border: 1px solid var(--line);
  border-radius: var(--r-sm);
}

.permission-guide__info {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.permission-guide__label {
  font-size: var(--fs-base);
  color: var(--text-1);
}

.permission-guide__impact {
  font-size: var(--fs-cap);
  color: var(--warn, var(--text-3));
}

.permission-guide__steps {
  margin: 0;
  padding-left: 1.4em;
  font-size: var(--fs-sm);
  line-height: 1.8;
  color: var(--text-2);
}
</style>
