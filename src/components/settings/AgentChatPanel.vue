<template>
  <!-- Agent 对话面板：消息列表 + 输入框，全部复用 src/components/common 原语 -->
  <div class="agent-chat">
    <div ref="messageListRef" class="agent-chat__messages">
      <EmptyState
        v-if="!messages.length && !isSending"
        size="sm"
        title="还没有对话"
        description="在下方输入内容即可与 Agent 对话，回复使用「默认 LLM 配置」生成。"
      />

      <div
        v-for="(message, index) in messages"
        :key="`${index}-${message.role}`"
        class="agent-chat__row"
        :class="`agent-chat__row--${message.role}`"
      >
        <span class="agent-chat__role">{{ message.role === 'user' ? '我' : 'Agent' }}</span>
        <p class="agent-chat__bubble">{{ message.content }}</p>
      </div>

      <!-- 发送中：用骨架屏占位，避免"正在思考…"纯文字 -->
      <div v-if="isSending" class="agent-chat__row agent-chat__row--assistant">
        <span class="agent-chat__role">Agent</span>
        <div class="agent-chat__bubble agent-chat__bubble--pending">
          <Skeleton :lines="2" inline />
        </div>
      </div>
    </div>

    <div class="agent-chat__composer">
      <BaseTextarea
        v-model="draft"
        :rows="3"
        :disabled="isSending"
        placeholder="输入消息，Enter 发送，Shift+Enter 换行"
        @keydown="handleKeydown"
      />

      <div class="agent-chat__actions">
        <span class="agent-chat__hint">Enter 发送 · Shift+Enter 换行</span>
        <div class="agent-chat__buttons">
          <BaseButton
            variant="ghost"
            :disabled="isSending || !messages.length"
            @click="handleClear"
          >
            清空会话
          </BaseButton>
          <BaseButton
            variant="solid"
            :loading="isSending"
            :disabled="!canSend"
            @click="handleSend"
          >
            发送
          </BaseButton>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup>
import { computed, nextTick, ref } from 'vue'
import api from '@/api/client'
import { useAppStore } from '@/stores/app'
import BaseButton from '@/components/common/BaseButton.vue'
import BaseTextarea from '@/components/common/BaseTextarea.vue'
import EmptyState from '@/components/common/EmptyState.vue'
import Skeleton from '@/components/common/Skeleton.vue'

defineOptions({ name: 'AgentChatPanel' })

const appStore = useAppStore()

// 会话只保存在本地内存：设置页刷新即重新开始，不做持久化（后端无会话存储）
const messages = ref([])
const draft = ref('')
const isSending = ref(false)
const messageListRef = ref(null)

const canSend = computed(() => !isSending.value && draft.value.trim().length > 0)

/**
 * Enter 发送、Shift+Enter 换行。
 * 中文输入法组合期间的 Enter（isComposing / keyCode 229）必须放行给输入法，否则会误发送。
 */
function handleKeydown(event) {
  if (event.key !== 'Enter' || event.shiftKey) {
    return
  }
  if (event.isComposing || event.keyCode === 229) {
    return
  }

  event.preventDefault()
  handleSend()
}

async function scrollToBottom() {
  await nextTick()
  const list = messageListRef.value
  if (list) {
    list.scrollTop = list.scrollHeight
  }
}

async function handleSend() {
  const content = draft.value.trim()

  if (!content || isSending.value) {
    return
  }

  // 先把用户消息与上下文一并发给后端，再由后端拼接成多轮 prompt
  const nextMessages = [...messages.value, { role: 'user', content }]
  messages.value = nextMessages
  draft.value = ''
  isSending.value = true
  await scrollToBottom()

  try {
    const result = await api.runAIChat(nextMessages)
    const answer = typeof result?.answer === 'string' ? result.answer.trim() : ''

    if (!answer) {
      throw new Error('Agent 没有返回内容，请稍后重试')
    }

    messages.value = [...messages.value, { role: 'assistant', content: answer }]
  } catch (error) {
    // 错误统一走 Toast（client.js 已把 502/400 的 detail 解析成可读中文）
    appStore.showToast(error?.message || 'Agent 对话失败', { tone: 'error' })
  } finally {
    isSending.value = false
    await scrollToBottom()
  }
}

function handleClear() {
  if (isSending.value) {
    return
  }

  messages.value = []
  draft.value = ''
}
</script>

<style scoped>
.agent-chat {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
  padding: var(--s-3);
  background: var(--bg-card);
  border: 1px solid var(--border);
  border-radius: var(--radius-md);
}

.agent-chat__messages {
  display: flex;
  flex-direction: column;
  gap: var(--s-3);
  max-height: 320px;
  min-height: 96px;
  padding: var(--s-2);
  overflow-y: auto;
  border: 1px solid var(--line);
  border-radius: var(--r-sm);
  background: var(--bg-raise);
}

.agent-chat__row {
  display: flex;
  flex-direction: column;
  gap: var(--s-1);
  max-width: 82%;
}

.agent-chat__row--user {
  align-self: flex-end;
  align-items: flex-end;
}

.agent-chat__row--assistant {
  align-self: flex-start;
  align-items: flex-start;
}

.agent-chat__role {
  color: var(--text-3);
  font-size: var(--fs-cap);
  letter-spacing: 0.05em;
}

.agent-chat__bubble {
  margin: 0;
  padding: var(--s-2) var(--s-3);
  border: 1px solid var(--line);
  border-radius: var(--r-md);
  background: var(--bg-surface);
  color: var(--text-1);
  font-size: var(--fs-base);
  line-height: 1.7;
  white-space: pre-wrap;
  word-break: break-word;
}

.agent-chat__row--user .agent-chat__bubble {
  border-color: var(--accent-line);
  background: var(--accent-dim);
}

.agent-chat__bubble--pending {
  width: 180px;
}

.agent-chat__composer {
  display: flex;
  flex-direction: column;
  gap: var(--s-2);
}

.agent-chat__actions {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--s-2);
  flex-wrap: wrap;
}

.agent-chat__hint {
  color: var(--text-3);
  font-size: var(--fs-cap);
}

.agent-chat__buttons {
  display: flex;
  align-items: center;
  gap: var(--s-2);
}
</style>
