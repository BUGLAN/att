'use strict';
// 微信输入法进程内的常驻 agent。编码器地址由 Rust 侧扫描 PE 后注入，
// 协议与录音文件不经过外部桥进程。移植自 kuxiaowo/wechat-asr-bridge（MIT）。

const ENCODERS = __WETYPE_ENCODERS__;
const targetModule = Process.getModuleByName('wetype_update.exe');
const base = targetModule.base;

function native(address, ret, args) {
  return new NativeFunction(base.add(address), ret, args, { abi: 'win64', exceptions: 'steal' });
}

const construct = native(ENCODERS.construct, 'pointer', ['pointer']);
const initialize = native(ENCODERS.initialize, 'int', ['pointer', 'int', 'int', 'int', 'int']);
const encode = native(ENCODERS.encode, 'int', ['pointer', 'pointer', 'int', 'pointer', 'int', 'pointer']);
const release = native(ENCODERS.release, 'void', ['pointer']);
const business = Process.getModuleByName('wetap_business.dll');
const submit = new NativeFunction(
  business.getExportByName('business_net_start_task_with_option'),
  'int',
  ['int', 'int', 'pointer', 'uint', 'pointer', 'pointer', 'pointer'],
  { abi: 'win64', exceptions: 'steal' }
);

const tasks = new Map();
let closing = false;
let callbacks = 0;

// 回调必须活到输入法进程退出：超时请求即使不再被 Promise 使用，也不能释放 user_data。
const callback = new NativeCallback((user, taskId, code, response, length) => {
  callbacks++;
  const key = user.toString();
  const task = tasks.get(key);
  if (!task) {
    callbacks--;
    return;
  }
  let result;
  try {
    if (length > 4 * 1024 * 1024 || (length && response.isNull())) {
      throw Error('Invalid native response length');
    }
    result = {
      taskId,
      code,
      data: length ? Array.from(new Uint8Array(response.readByteArray(length))) : []
    };
  } catch (error) {
    result = { taskId, code: -1, error: String(error), data: [] };
  }
  clearTimeout(task.timeout);
  // 先返回原生调用栈，再唤醒 Rust 的 RPC；避免在回调里重入 Frida。
  setTimeout(() => {
    tasks.delete(key);
    callbacks--;
    task.resolve(result);
  }, 250);
}, 'void', ['pointer', 'int', 'int', 'pointer', 'uint'], 'win64');

rpc.exports = {
  encode(pcm) {
    if (closing || pcm.length === 0 || pcm.length % 640 !== 0) {
      throw Error('Invalid PCM or agent stopping');
    }
    const object = Memory.alloc(0x28);
    const input = Memory.alloc(pcm.length);
    const capacity = (pcm.length / 640) * 1502 + 16;
    const output = Memory.alloc(capacity);
    const length = Memory.alloc(4);
    input.writeByteArray(new Uint8Array(pcm));
    construct(object);
    try {
      let code = initialize(object, 16000, 1, 64000, 1);
      if (code !== 0) {
        throw Error('Opus initialization failed: ' + code);
      }
      code = encode(object, input, pcm.length, output, capacity, length);
      if (code !== 0) {
        throw Error('Opus encoding failed: ' + code);
      }
      const size = length.readU32();
      if (size > capacity) {
        throw Error('Invalid Opus output size');
      }
      return Array.from(new Uint8Array(output.readByteArray(size)));
    } finally {
      release(object);
    }
  },

  request(bytes, owner = '') {
    if (closing || !bytes.length) {
      throw Error('Invalid request or agent stopping');
    }
    return new Promise((resolve, reject) => {
      const data = Memory.alloc(bytes.length);
      const options = Memory.alloc(16);
      const user = Memory.alloc(8);
      data.writeByteArray(new Uint8Array(bytes));
      options.writeByteArray(new Uint8Array(16));
      options.writeU32(3000);
      options.add(4).writeU8(2);
      const key = user.toString();
      const task = { data, options, user, resolve, reject, owner };
      tasks.set(key, task);
      task.timeout = setTimeout(() => {
        // 回调可能已被原生层登记；这里只拒绝 Promise，任务和回调内存继续保留。
        reject(Error('Native task timed out; callback retained'));
      }, 15000);
      try {
        const id = submit(4548, 0, data, bytes.length, user, callback, options);
        if (id < 0) {
          clearTimeout(task.timeout);
          tasks.delete(key);
          resolve({ taskId: id, code: id, data: [] });
        }
      } catch (error) {
        clearTimeout(task.timeout);
        reject(error);
      }
    });
  },

  stats() {
    return { pending: tasks.size, callbacks, closing };
  },

  finalize(owner) {
    return {
      cleaned: owner === undefined
        ? tasks.size === 0 && callbacks === 0
        : !Array.from(tasks.values()).some((task) => task.owner === owner)
    };
  },

  dispose() {
    closing = true;
    return { cleaned: tasks.size === 0 && callbacks === 0 };
  }
};
