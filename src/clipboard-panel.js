// 设计令牌必须先于任何组件/窗口样式引入（与 main.js / floating.js 顺序一致）
import './styles/tokens.css'
import { createApp } from 'vue'
import ClipboardPanel from './components/clipboardPanel/ClipboardPanel.vue'
import './styles/page-atoms.css'
import './styles/clipboard-panel.css'

createApp(ClipboardPanel).mount('#app')
