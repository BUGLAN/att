<template>
  <header class="title-bar">
    <div class="title-bar-drag" data-tauri-drag-region @dblclick="windowControl.maximize()">
      <div class="title-bar-brand" data-tauri-drag-region>
        <span class="ui-seal title-bar-seal" aria-hidden="true"><span class="ui-seal-ink">{{ BRAND_GLYPH }}</span></span>
        <span class="logo-text">Audio To Text</span>
      </div>
    </div>

    <div class="title-bar-buttons">
      <button class="title-bar-btn" type="button" title="最小化" @click="windowControl.minimize()">
        <span></span>
      </button>
      <button class="title-bar-btn" type="button" title="最大化 / 还原" @click="windowControl.maximize()">
        <svg viewBox="0 0 12 12" fill="none" aria-hidden="true">
          <rect x="2.5" y="2.5" width="7" height="7" rx="1"></rect>
        </svg>
      </button>
      <button class="title-bar-btn close" type="button" title="关闭" @click="windowControl.close()">
        <svg viewBox="0 0 12 12" fill="none" aria-hidden="true">
          <path d="M3 3l6 6M9 3 3 9"></path>
        </svg>
      </button>
    </div>
  </header>
</template>

<script setup>
import { useElectron } from '@/composables/useElectron'
import { BRAND_GLYPH } from '@/components/common/glyphs.mjs'

const { windowControl } = useElectron()
</script>
