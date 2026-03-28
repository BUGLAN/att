class PcmCaptureProcessor extends AudioWorkletProcessor {
  process(inputs) {
    const input = inputs[0]
    if (!input || !input[0] || input[0].length === 0) {
      return true
    }

    const channelData = input[0]
    const copy = new Float32Array(channelData.length)
    copy.set(channelData)
    this.port.postMessage(copy, [copy.buffer])
    return true
  }
}

registerProcessor('pcm-capture-processor', PcmCaptureProcessor)
