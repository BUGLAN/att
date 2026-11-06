// 在线识别引擎凭据验证的共享状态：设置页在保存 / 启动时触发，识别引擎页展示。
//
// 后端结论按凭据指纹失效（换 Token / Resource ID / 接入点即作废，见 att_core::stt::verify）；
// 这里只缓存「最近一次拿到的结论」，并把进行中的验证标成 checking。模块级状态，各页面共用一份。
import { reactive } from 'vue'
import { useElectron } from '@/composables/useElectron'
import { useSettingsStore } from '@/stores/settings'
import { isOnlineProvider } from '@/components/settings/onlineEngineStatus.mjs'

// provider → { status: 'checking'|'ok'|'invalid'|'unknown', message }；没验过不存在
const checks = reactive({})
const inflight = new Map()

export function useAsrCredentialCheck() {
  const { api } = useElectron()
  const settingsStore = useSettingsStore()

  /** 读取后端记下的最近一次结论（不发起验证）。 */
  async function load(provider) {
    if (!isOnlineProvider(provider) || inflight.has(provider)) {
      return checks[provider] || null
    }
    try {
      const result = await api.getAsrCredentialCheck(provider)
      setCheck(provider, result?.check)
    } catch (error) {
      console.warn('[ASR] 读取凭据验证结果失败:', error)
    }
    return checks[provider] || null
  }

  /** 立即验证（同一引擎同时只跑一次）；结束后刷新运行时状态，首页的就绪判定随之更新。 */
  function verify(provider) {
    if (!isOnlineProvider(provider)) {
      return Promise.resolve(null)
    }
    if (inflight.has(provider)) {
      return inflight.get(provider)
    }
    checks[provider] = { status: 'checking', message: '' }
    const task = (async () => {
      try {
        const result = await api.checkAsrCredential(provider)
        setCheck(provider, result?.check)
      } catch (error) {
        // 409 = 凭据没配齐（例如刚清除 Token），不算验证失败；其余是本地数据面问题，同样不下结论
        setCheck(provider, null)
      } finally {
        inflight.delete(provider)
        settingsStore.loadRuntimeStatus(false).catch(() => {})
      }
      return checks[provider] || null
    })()
    inflight.set(provider, task)
    return task
  }

  return { checks, load, verify }
}

function setCheck(provider, check) {
  if (check && check.status) {
    checks[provider] = { status: check.status, message: check.message || '' }
  } else {
    delete checks[provider]
  }
}
