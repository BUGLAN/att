// NDJSON 流式解析：后端每行一个 JSON 对象，这里按 \n 切分并回调。
// 不依赖 fetch / DOM，可被 Node 直接单测。

export function createNdjsonParser(onObject) {
  if (typeof onObject !== 'function') {
    throw new TypeError('createNdjsonParser 需要 onObject 回调')
  }

  let buffer = ''

  function consume(line) {
    const trimmed = line.replace(/\r$/, '').trim()
    if (!trimmed) {
      return
    }

    let parsed
    try {
      parsed = JSON.parse(trimmed)
    } catch (error) {
      const wrapped = new SyntaxError(`NDJSON 行解析失败：${error.message}`)
      wrapped.cause = error
      wrapped.line = trimmed
      throw wrapped
    }

    onObject(parsed)
  }

  return {
    push(chunk) {
      buffer += chunk
      let index = buffer.indexOf('\n')
      while (index >= 0) {
        consume(buffer.slice(0, index))
        buffer = buffer.slice(index + 1)
        index = buffer.indexOf('\n')
      }
    },

    flush() {
      if (buffer.trim()) {
        consume(buffer)
      }
      buffer = ''
    }
  }
}
