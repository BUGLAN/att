const OVERLAY_MODE = {
  selection: 'selection',
  clipboard: 'clipboard'
}

const OVERLAY_CHANNELS = {
  openSelection: 'overlay:open-selection',
  openClipboard: 'overlay:open-clipboard',
  hide: 'overlay:hide',
  pin: 'overlay:pin',
  ready: 'overlay:ready',
  activate: 'overlay:activate',
  pinnedChanged: 'overlay:pinned-changed',
  actionComplete: 'overlay:action-complete'
}

module.exports = {
  OVERLAY_MODE,
  OVERLAY_CHANNELS
}
