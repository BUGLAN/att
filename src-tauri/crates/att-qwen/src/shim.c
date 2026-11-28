/*
 * att-qwen C 桥：把 sherpa-onnx 的 Qwen3-ASR / SenseVoice 离线识别包装成对外的少数字符串接口。
 *
 * 为什么要这层 C 桥：
 *  1. 直接包含 vendored 的官方头 `vendor/c-api.h`（来自 sherpa-onnx v2.0.1），结构体布局与
 *     动态库完全一致——Rust 侧不用手写几十个嵌套 #[repr(C)] 结构体，避免字段错位这种静默崩溃；
 *  2. 运行期用 LoadLibrary/dlopen 加载 `sherpa-onnx-c-api`，构建期不链接任何原生库：
 *     没装推理库的机器照样能编译与跑其它引擎，缺库时是运行期可读报错。
 *
 * 内存归属：本文件返回的文本用 malloc 分配，由 att_qwen_free_text 释放；
 * sherpa-onnx 的 recognizer/stream/result 都由本文件按官方约定销毁。
 */

#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "c-api.h"

#if defined(_WIN32)
#include <windows.h>
#define ATT_LIB_NAME "sherpa-onnx-c-api.dll"
#elif defined(__APPLE__)
#include <dlfcn.h>
#define ATT_LIB_NAME "libsherpa-onnx-c-api.dylib"
#else
#include <dlfcn.h>
#define ATT_LIB_NAME "libsherpa-onnx-c-api.so"
#endif

#define ATT_ERR_LEN 1024

/* ---- 动态库加载 ---- */

#if defined(_WIN32)

static void *att_dlopen(const char *path) {
  int len = MultiByteToWideChar(CP_UTF8, 0, path, -1, NULL, 0);
  if (len <= 0) {
    return NULL;
  }
  wchar_t *wide = (wchar_t *)malloc(sizeof(wchar_t) * (size_t)len);
  if (!wide) {
    return NULL;
  }
  MultiByteToWideChar(CP_UTF8, 0, path, -1, wide, len);
  /* LOAD_WITH_ALTERED_SEARCH_PATH：让 onnxruntime.dll 从推理库所在目录解析依赖 */
  HMODULE module = LoadLibraryExW(wide, NULL, LOAD_WITH_ALTERED_SEARCH_PATH);
  free(wide);
  return (void *)module;
}

static void *att_dlsym(void *handle, const char *name) {
  return (void *)GetProcAddress((HMODULE)handle, name);
}

static void att_dlclose(void *handle) {
  if (handle) {
    FreeLibrary((HMODULE)handle);
  }
}

#else

static void *att_dlopen(const char *path) { return dlopen(path, RTLD_NOW | RTLD_LOCAL); }

static void *att_dlsym(void *handle, const char *name) { return dlsym(handle, name); }

static void att_dlclose(void *handle) {
  if (handle) {
    dlclose(handle);
  }
}

#endif

/* ---- sherpa-onnx 函数表 ---- */

typedef const SherpaOnnxOfflineRecognizer *(*att_fn_create_recognizer)(
    const SherpaOnnxOfflineRecognizerConfig *);
typedef void (*att_fn_destroy_recognizer)(const SherpaOnnxOfflineRecognizer *);
typedef const SherpaOnnxOfflineStream *(*att_fn_create_stream)(
    const SherpaOnnxOfflineRecognizer *);
typedef void (*att_fn_destroy_stream)(const SherpaOnnxOfflineStream *);
typedef void (*att_fn_accept_waveform)(const SherpaOnnxOfflineStream *, int32_t,
                                       const float *, int32_t);
typedef void (*att_fn_decode)(const SherpaOnnxOfflineRecognizer *,
                              const SherpaOnnxOfflineStream *);
typedef const SherpaOnnxOfflineRecognizerResult *(*att_fn_get_result)(
    const SherpaOnnxOfflineStream *);
typedef void (*att_fn_destroy_result)(const SherpaOnnxOfflineRecognizerResult *);

typedef struct AttQwen {
  void *lib;
  att_fn_create_recognizer create_recognizer;
  att_fn_destroy_recognizer destroy_recognizer;
  att_fn_create_stream create_stream;
  att_fn_destroy_stream destroy_stream;
  att_fn_accept_waveform accept_waveform;
  att_fn_decode decode;
  att_fn_get_result get_result;
  att_fn_destroy_result destroy_result;
  const SherpaOnnxOfflineRecognizer *recognizer;
  /* 配置里的字符串在 recognizer 生命周期内保持有效（strdup 保活） */
  char *conv_frontend;
  char *encoder;
  char *decoder;
  char *tokenizer;
} AttQwen;

static void att_set_error(char *buffer, int buffer_len, const char *format, ...) {
  if (!buffer || buffer_len <= 0) {
    return;
  }
  va_list args;
  va_start(args, format);
  vsnprintf(buffer, (size_t)buffer_len, format, args);
  va_end(args);
  buffer[buffer_len - 1] = '\0';
}

static void att_qwen_release(AttQwen *handle) {
  if (!handle) {
    return;
  }
  if (handle->recognizer && handle->destroy_recognizer) {
    handle->destroy_recognizer(handle->recognizer);
  }
  free(handle->conv_frontend);
  free(handle->encoder);
  free(handle->decoder);
  free(handle->tokenizer);
  att_dlclose(handle->lib);
  free(handle);
}

/*
 * Windows 上 C:\Windows\System32\onnxruntime.dll（别的软件装的旧版）会抢先被解析，
 * 导致 sherpa-onnx 报「requested API version [28] is not available」并崩溃。
 * 这里先把推理库同目录的 onnxruntime 按**绝对路径**预加载进进程，
 * 之后 sherpa-onnx 对 "onnxruntime.dll" 的按名导入就会命中已加载的这份。
 */
static void att_preload_dependencies(const char *lib_dir) {
#if defined(_WIN32)
  static const char *names[] = {"onnxruntime.dll", "onnxruntime_providers_shared.dll",
                                "DirectML.dll"};
  for (size_t i = 0; i < sizeof(names) / sizeof(names[0]); ++i) {
    size_t len = strlen(lib_dir) + strlen(names[i]) + 2;
    char *path = (char *)malloc(len);
    if (!path) {
      return;
    }
    snprintf(path, len, "%s/%s", lib_dir, names[i]);
    att_dlopen(path); /* 失败无所谓：依赖可能存在、也可能不需要（如 DirectML） */
    free(path);
  }
#else
  (void)lib_dir;
#endif
}

static char *att_strdup(const char *value) {
  if (!value) {
    value = "";
  }
  size_t len = strlen(value);
  char *copy = (char *)malloc(len + 1);
  if (copy) {
    memcpy(copy, value, len + 1);
  }
  return copy;
}

/*
 * 创建 Qwen3-ASR 识别器。
 * lib_dir：存放 sherpa-onnx 动态库的目录；模型四个路径：conv_frontend / encoder / decoder / tokenizer。
 * 成功返回句柄，失败返回 NULL 并在 err 写入中文原因。
 */
AttQwen *att_qwen_create(const char *lib_dir, const char *conv_frontend, const char *encoder,
                         const char *decoder, const char *tokenizer, int32_t num_threads,
                         const char *provider, int32_t max_total_len, int32_t max_new_tokens,
                         char *err, int32_t err_len) {
  if (!lib_dir || !conv_frontend || !encoder || !decoder || !tokenizer) {
    att_set_error(err, err_len, "参数缺失：推理库目录与模型文件路径都是必需的");
    return NULL;
  }

  size_t path_len = strlen(lib_dir) + strlen(ATT_LIB_NAME) + 2;
  char *lib_path = (char *)malloc(path_len);
  if (!lib_path) {
    att_set_error(err, err_len, "内存分配失败");
    return NULL;
  }
  snprintf(lib_path, path_len, "%s/%s", lib_dir, ATT_LIB_NAME);
  AttQwen *handle = (AttQwen *)calloc(1, sizeof(AttQwen));
  if (!handle) {
    free(lib_path);
    att_set_error(err, err_len, "内存分配失败");
    return NULL;
  }
  att_preload_dependencies(lib_dir);
  handle->lib = att_dlopen(lib_path);
  if (!handle->lib) {
    att_set_error(err, err_len,
                  "无法加载 sherpa-onnx 推理库：%s（请确认该目录下有 %s 与 onnxruntime 依赖）",
                  lib_path, ATT_LIB_NAME);
    free(lib_path);
    att_qwen_release(handle);
    return NULL;
  }
  free(lib_path);

  handle->create_recognizer =
      (att_fn_create_recognizer)att_dlsym(handle->lib, "SherpaOnnxCreateOfflineRecognizer");
  handle->destroy_recognizer = (att_fn_destroy_recognizer)att_dlsym(
      handle->lib, "SherpaOnnxDestroyOfflineRecognizer");
  handle->create_stream =
      (att_fn_create_stream)att_dlsym(handle->lib, "SherpaOnnxCreateOfflineStream");
  handle->destroy_stream =
      (att_fn_destroy_stream)att_dlsym(handle->lib, "SherpaOnnxDestroyOfflineStream");
  handle->accept_waveform =
      (att_fn_accept_waveform)att_dlsym(handle->lib, "SherpaOnnxAcceptWaveformOffline");
  handle->decode = (att_fn_decode)att_dlsym(handle->lib, "SherpaOnnxDecodeOfflineStream");
  handle->get_result =
      (att_fn_get_result)att_dlsym(handle->lib, "SherpaOnnxGetOfflineStreamResult");
  handle->destroy_result = (att_fn_destroy_result)att_dlsym(
      handle->lib, "SherpaOnnxDestroyOfflineRecognizerResult");
  if (!handle->create_recognizer || !handle->destroy_recognizer || !handle->create_stream ||
      !handle->destroy_stream || !handle->accept_waveform || !handle->decode ||
      !handle->get_result || !handle->destroy_result) {
    att_set_error(err, err_len, "sherpa-onnx 推理库缺少离线识别符号（版本可能过旧，需 >= 2.0.1）");
    att_qwen_release(handle);
    return NULL;
  }

  handle->conv_frontend = att_strdup(conv_frontend);
  handle->encoder = att_strdup(encoder);
  handle->decoder = att_strdup(decoder);
  handle->tokenizer = att_strdup(tokenizer);
  if (!handle->conv_frontend || !handle->encoder || !handle->decoder || !handle->tokenizer) {
    att_set_error(err, err_len, "内存分配失败");
    att_qwen_release(handle);
    return NULL;
  }

  SherpaOnnxOfflineRecognizerConfig config;
  memset(&config, 0, sizeof(config));
  config.feat_config.sample_rate = 16000;
  config.feat_config.feature_dim = 80;
  config.model_config.tokens = "";
  config.model_config.num_threads = num_threads > 0 ? num_threads : 2;
  config.model_config.debug = 0;
  config.model_config.provider = (provider && provider[0]) ? provider : "cpu";
  config.model_config.model_type = "";
  config.model_config.modeling_unit = "";
  config.model_config.bpe_vocab = "";
  config.model_config.qwen3_asr.conv_frontend = handle->conv_frontend;
  config.model_config.qwen3_asr.encoder = handle->encoder;
  config.model_config.qwen3_asr.decoder = handle->decoder;
  config.model_config.qwen3_asr.tokenizer = handle->tokenizer;
  config.model_config.qwen3_asr.max_total_len = max_total_len > 0 ? max_total_len : 512;
  config.model_config.qwen3_asr.max_new_tokens = max_new_tokens > 0 ? max_new_tokens : 512;
  config.model_config.qwen3_asr.temperature = 1e-6f;
  config.model_config.qwen3_asr.top_p = 0.8f;
  config.model_config.qwen3_asr.seed = 42;
  config.model_config.qwen3_asr.hotwords = "";
  config.decoding_method = "greedy_search";

  handle->recognizer = handle->create_recognizer(&config);
  if (!handle->recognizer) {
    att_set_error(err, err_len,
                  "创建 Qwen3-ASR 识别器失败（模型文件不完整或与推理库版本不匹配）：%s",
                  conv_frontend);
    att_qwen_release(handle);
    return NULL;
  }
  return handle;
}

/*
 * 识别一段 16k 单声道 float32 PCM（[-1,1]）。
 * 成功返回 malloc 的 UTF-8 文本（可能为空串），调用方用 att_qwen_free_text 释放；失败返回 NULL。
 */
char *att_qwen_transcribe(AttQwen *handle, const float *samples, int32_t n, int32_t sample_rate,
                          char *err, int32_t err_len) {
  if (!handle || !handle->recognizer) {
    att_set_error(err, err_len, "识别器未初始化");
    return NULL;
  }
  if (!samples || n <= 0) {
    att_set_error(err, err_len, "没有音频数据");
    return NULL;
  }
  if (sample_rate <= 0) {
    sample_rate = 16000;
  }

  const SherpaOnnxOfflineStream *stream = handle->create_stream(handle->recognizer);
  if (!stream) {
    att_set_error(err, err_len, "创建识别流失败");
    return NULL;
  }
  handle->accept_waveform(stream, sample_rate, samples, n);
  handle->decode(handle->recognizer, stream);
  const SherpaOnnxOfflineRecognizerResult *result = handle->get_result(stream);
  char *text = NULL;
  if (result) {
    text = att_strdup(result->text);
    handle->destroy_result(result);
  }
  handle->destroy_stream(stream);
  if (!text) {
    att_set_error(err, err_len, "识别结果为空指针");
    return NULL;
  }
  return text;
}

void att_qwen_free_text(char *text) { free(text); }

void att_qwen_destroy(AttQwen *handle) { att_qwen_release(handle); }

/* ==================== SenseVoice（复用同一份 sherpa-onnx 动态库） ==================== */

typedef struct AttSenseVoice {
  void *lib;
  att_fn_create_recognizer create_recognizer;
  att_fn_destroy_recognizer destroy_recognizer;
  att_fn_create_stream create_stream;
  att_fn_destroy_stream destroy_stream;
  att_fn_accept_waveform accept_waveform;
  att_fn_decode decode;
  att_fn_get_result get_result;
  att_fn_destroy_result destroy_result;
  const SherpaOnnxOfflineRecognizer *recognizer;
  char *model;
  char *tokens;
  char *language;
} AttSenseVoice;

static void att_sensevoice_release(AttSenseVoice *handle) {
  if (!handle) {
    return;
  }
  if (handle->recognizer && handle->destroy_recognizer) {
    handle->destroy_recognizer(handle->recognizer);
  }
  free(handle->model);
  free(handle->tokens);
  free(handle->language);
  att_dlclose(handle->lib);
  free(handle);
}

/*
 * 创建 SenseVoice 识别器。
 * lib_dir：存放 sherpa-onnx 动态库的目录；model / tokens 指向模型目录里的两个必需文件。
 */
AttSenseVoice *att_sensevoice_create(const char *lib_dir, const char *model, const char *tokens,
                                     const char *language, int32_t use_itn, int32_t num_threads,
                                     const char *provider, char *err, int32_t err_len) {
  if (!lib_dir || !model || !tokens || !language) {
    att_set_error(err, err_len, "参数缺失：推理库目录、模型、词表和语言都是必需的");
    return NULL;
  }

  size_t path_len = strlen(lib_dir) + strlen(ATT_LIB_NAME) + 2;
  char *lib_path = (char *)malloc(path_len);
  if (!lib_path) {
    att_set_error(err, err_len, "内存分配失败");
    return NULL;
  }
  snprintf(lib_path, path_len, "%s/%s", lib_dir, ATT_LIB_NAME);
  AttSenseVoice *handle = (AttSenseVoice *)calloc(1, sizeof(AttSenseVoice));
  if (!handle) {
    free(lib_path);
    att_set_error(err, err_len, "内存分配失败");
    return NULL;
  }
  att_preload_dependencies(lib_dir);
  handle->lib = att_dlopen(lib_path);
  if (!handle->lib) {
    att_set_error(err, err_len,
                  "无法加载 sherpa-onnx 推理库：%s（请确认该目录下有 %s 与 onnxruntime 依赖）",
                  lib_path, ATT_LIB_NAME);
    free(lib_path);
    att_sensevoice_release(handle);
    return NULL;
  }
  free(lib_path);

  handle->create_recognizer =
      (att_fn_create_recognizer)att_dlsym(handle->lib, "SherpaOnnxCreateOfflineRecognizer");
  handle->destroy_recognizer = (att_fn_destroy_recognizer)att_dlsym(
      handle->lib, "SherpaOnnxDestroyOfflineRecognizer");
  handle->create_stream =
      (att_fn_create_stream)att_dlsym(handle->lib, "SherpaOnnxCreateOfflineStream");
  handle->destroy_stream =
      (att_fn_destroy_stream)att_dlsym(handle->lib, "SherpaOnnxDestroyOfflineStream");
  handle->accept_waveform =
      (att_fn_accept_waveform)att_dlsym(handle->lib, "SherpaOnnxAcceptWaveformOffline");
  handle->decode = (att_fn_decode)att_dlsym(handle->lib, "SherpaOnnxDecodeOfflineStream");
  handle->get_result =
      (att_fn_get_result)att_dlsym(handle->lib, "SherpaOnnxGetOfflineStreamResult");
  handle->destroy_result = (att_fn_destroy_result)att_dlsym(
      handle->lib, "SherpaOnnxDestroyOfflineRecognizerResult");
  if (!handle->create_recognizer || !handle->destroy_recognizer || !handle->create_stream ||
      !handle->destroy_stream || !handle->accept_waveform || !handle->decode ||
      !handle->get_result || !handle->destroy_result) {
    att_set_error(err, err_len, "sherpa-onnx 推理库缺少离线识别符号（版本可能过旧，需 >= 2.0.1）");
    att_sensevoice_release(handle);
    return NULL;
  }

  handle->model = att_strdup(model);
  handle->tokens = att_strdup(tokens);
  handle->language = att_strdup(language);
  if (!handle->model || !handle->tokens || !handle->language) {
    att_set_error(err, err_len, "内存分配失败");
    att_sensevoice_release(handle);
    return NULL;
  }

  SherpaOnnxOfflineRecognizerConfig config;
  memset(&config, 0, sizeof(config));
  config.feat_config.sample_rate = 16000;
  config.feat_config.feature_dim = 80;
  config.model_config.tokens = handle->tokens;
  config.model_config.num_threads = num_threads > 0 ? num_threads : 2;
  config.model_config.debug = 0;
  config.model_config.provider = (provider && provider[0]) ? provider : "cpu";
  config.model_config.sense_voice.model = handle->model;
  config.model_config.sense_voice.language = handle->language;
  config.model_config.sense_voice.use_itn = use_itn ? 1 : 0;
  config.decoding_method = "greedy_search";

  handle->recognizer = handle->create_recognizer(&config);
  if (!handle->recognizer) {
    att_set_error(err, err_len,
                  "创建 SenseVoice 识别器失败（模型文件不完整或与推理库版本不匹配）：%s",
                  model);
    att_sensevoice_release(handle);
    return NULL;
  }
  return handle;
}

/* 识别一段 16k 单声道 float32 PCM；成功返回 malloc 的 UTF-8 文本。 */
char *att_sensevoice_transcribe(AttSenseVoice *handle, const float *samples, int32_t n,
                                int32_t sample_rate, char *err, int32_t err_len) {
  if (!handle || !handle->recognizer) {
    att_set_error(err, err_len, "识别器未初始化");
    return NULL;
  }
  if (!samples || n <= 0) {
    att_set_error(err, err_len, "没有音频数据");
    return NULL;
  }
  if (sample_rate <= 0) {
    sample_rate = 16000;
  }

  const SherpaOnnxOfflineStream *stream = handle->create_stream(handle->recognizer);
  if (!stream) {
    att_set_error(err, err_len, "创建识别流失败");
    return NULL;
  }
  handle->accept_waveform(stream, sample_rate, samples, n);
  handle->decode(handle->recognizer, stream);
  const SherpaOnnxOfflineRecognizerResult *result = handle->get_result(stream);
  char *text = NULL;
  if (result) {
    text = att_strdup(result->text);
    handle->destroy_result(result);
  }
  handle->destroy_stream(stream);
  if (!text) {
    att_set_error(err, err_len, "识别结果为空指针");
    return NULL;
  }
  return text;
}

void att_sensevoice_free_text(char *text) { free(text); }

void att_sensevoice_destroy(AttSenseVoice *handle) { att_sensevoice_release(handle); }

/* ==================== Silero VAD（与 Qwen 共用同一份 sherpa-onnx 动态库） ====================
 *
 * faster-whisper 的 `vad_filter=True` 在 Python 侧是 Silero VAD；Rust 侧没有现成实现，
 * 但 sherpa-onnx 自带 Silero VAD 的 C API，这里复用同一份动态库把「语音区间」算出来，
 * 由上层按区间送 Whisper 推理（音乐/噪声素材上能显著压掉 Whisper 的幻觉）。
 */

typedef const SherpaOnnxVoiceActivityDetector *(*att_fn_create_vad)(
    const SherpaOnnxVadModelConfig *config, float buffer_size_in_seconds);
typedef void (*att_fn_destroy_vad)(const SherpaOnnxVoiceActivityDetector *);
typedef void (*att_fn_accept_vad)(const SherpaOnnxVoiceActivityDetector *, const float *,
                                  int32_t);
typedef int32_t (*att_fn_vad_empty)(const SherpaOnnxVoiceActivityDetector *);
typedef const SherpaOnnxSpeechSegment *(*att_fn_vad_front)(
    const SherpaOnnxVoiceActivityDetector *);
typedef void (*att_fn_vad_pop)(const SherpaOnnxVoiceActivityDetector *);
typedef void (*att_fn_vad_flush)(const SherpaOnnxVoiceActivityDetector *);
typedef void (*att_fn_vad_reset)(const SherpaOnnxVoiceActivityDetector *);
typedef void (*att_fn_destroy_segment)(const SherpaOnnxSpeechSegment *);

typedef struct AttVad {
  void *lib;
  att_fn_create_vad create;
  att_fn_destroy_vad destroy;
  att_fn_accept_vad accept;
  att_fn_vad_empty is_empty;
  att_fn_vad_front front;
  att_fn_vad_pop pop;
  att_fn_vad_flush flush;
  att_fn_vad_reset reset;
  att_fn_destroy_segment destroy_segment;
  const SherpaOnnxVoiceActivityDetector *vad;
  char *model;
  int32_t window_size;
  int64_t *ranges;
  size_t range_count;
  size_t range_capacity;
} AttVad;

static void att_vad_release(AttVad *handle) {
  if (!handle) {
    return;
  }
  if (handle->vad && handle->destroy) {
    handle->destroy(handle->vad);
  }
  free(handle->model);
  free(handle->ranges);
  att_dlclose(handle->lib);
  free(handle);
}

/* 创建 VAD；成功返回句柄，失败返回 NULL 并写 err。 */
AttVad *att_vad_create(const char *lib_dir, const char *model, float threshold,
                       float min_silence, float min_speech, float max_speech, char *err,
                       int32_t err_len) {
  if (!lib_dir || !model) {
    att_set_error(err, err_len, "参数缺失：推理库目录与 VAD 模型路径都是必需的");
    return NULL;
  }
  size_t path_len = strlen(lib_dir) + strlen(ATT_LIB_NAME) + 2;
  char *lib_path = (char *)malloc(path_len);
  AttVad *handle = (AttVad *)calloc(1, sizeof(AttVad));
  if (!lib_path || !handle) {
    free(lib_path);
    free(handle);
    att_set_error(err, err_len, "内存分配失败");
    return NULL;
  }
  snprintf(lib_path, path_len, "%s/%s", lib_dir, ATT_LIB_NAME);
  att_preload_dependencies(lib_dir);
  handle->lib = att_dlopen(lib_path);
  if (!handle->lib) {
    att_set_error(err, err_len, "无法加载 sherpa-onnx 推理库：%s", lib_path);
    free(lib_path);
    att_vad_release(handle);
    return NULL;
  }
  free(lib_path);

  handle->create = (att_fn_create_vad)att_dlsym(handle->lib, "SherpaOnnxCreateVoiceActivityDetector");
  handle->destroy = (att_fn_destroy_vad)att_dlsym(handle->lib, "SherpaOnnxDestroyVoiceActivityDetector");
  handle->accept = (att_fn_accept_vad)att_dlsym(handle->lib, "SherpaOnnxVoiceActivityDetectorAcceptWaveform");
  handle->is_empty = (att_fn_vad_empty)att_dlsym(handle->lib, "SherpaOnnxVoiceActivityDetectorEmpty");
  handle->front = (att_fn_vad_front)att_dlsym(handle->lib, "SherpaOnnxVoiceActivityDetectorFront");
  handle->pop = (att_fn_vad_pop)att_dlsym(handle->lib, "SherpaOnnxVoiceActivityDetectorPop");
  handle->flush = (att_fn_vad_flush)att_dlsym(handle->lib, "SherpaOnnxVoiceActivityDetectorFlush");
  handle->reset = (att_fn_vad_reset)att_dlsym(handle->lib, "SherpaOnnxVoiceActivityDetectorReset");
  handle->destroy_segment = (att_fn_destroy_segment)att_dlsym(handle->lib, "SherpaOnnxDestroySpeechSegment");
  if (!handle->create || !handle->destroy || !handle->accept || !handle->is_empty ||
      !handle->front || !handle->pop || !handle->flush || !handle->reset ||
      !handle->destroy_segment) {
    att_set_error(err, err_len, "sherpa-onnx 推理库缺少 VAD 符号（版本可能过旧，需 >= 2.0.1）");
    att_vad_release(handle);
    return NULL;
  }

  handle->model = att_strdup(model);
  handle->window_size = 512;

  SherpaOnnxVadModelConfig config;
  memset(&config, 0, sizeof(config));
  config.silero_vad.model = handle->model;
  config.silero_vad.threshold = threshold > 0.0f ? threshold : 0.5f;
  config.silero_vad.min_silence_duration = min_silence > 0.0f ? min_silence : 0.5f;
  config.silero_vad.min_speech_duration = min_speech > 0.0f ? min_speech : 0.25f;
  config.silero_vad.max_speech_duration = max_speech > 0.0f ? max_speech : 30.0f;
  config.silero_vad.window_size = handle->window_size;
  config.sample_rate = 16000;
  config.num_threads = 1;
  config.provider = "cpu";
  config.debug = 0;

  handle->vad = handle->create(&config, 60.0f);
  if (!handle->vad) {
    att_set_error(err, err_len, "创建 Silero VAD 失败：%s", model);
    att_vad_release(handle);
    return NULL;
  }
  return handle;
}

/* 取走 VAD 当前已闭合的段，只保存左右边界，立即释放段内的样本拷贝。 */
static int32_t att_vad_drain_segments(AttVad *handle, char *err, int32_t err_len) {
  while (!handle->is_empty(handle->vad)) {
    const SherpaOnnxSpeechSegment *segment = handle->front(handle->vad);
    if (segment) {
      if (handle->range_count == handle->range_capacity) {
        size_t capacity = handle->range_capacity ? handle->range_capacity * 2 : 32;
        int64_t *grown =
            (int64_t *)realloc(handle->ranges, capacity * 2 * sizeof(int64_t));
        if (!grown) {
          handle->destroy_segment(segment);
          handle->pop(handle->vad);
          att_set_error(err, err_len, "内存分配失败");
          return 0;
        }
        handle->ranges = grown;
        handle->range_capacity = capacity;
      }
      handle->ranges[handle->range_count * 2] = segment->start;
      handle->ranges[handle->range_count * 2 + 1] = segment->start + segment->n;
      handle->range_count++;
      handle->destroy_segment(segment);
    }
    handle->pop(handle->vad);
  }
  return 1;
}

/* 开始一次流式 VAD。每次独立音频都必须调用，避免绝对采样位置跨调用累加。 */
int32_t att_vad_start(AttVad *handle, char *err, int32_t err_len) {
  if (!handle || !handle->vad) {
    att_set_error(err, err_len, "VAD 未初始化");
    return 0;
  }
  handle->reset(handle->vad);
  handle->range_count = 0;
  return 1;
}

/* 接受一段连续音频；每喂一个窗口就弹走已闭合段，避免段内 f32 样本积压。 */
int32_t att_vad_accept(AttVad *handle, const float *samples, int32_t n, char *err,
                       int32_t err_len) {
  if (!handle || !handle->vad) {
    att_set_error(err, err_len, "VAD 未初始化");
    return 0;
  }
  if (!samples || n <= 0) {
    att_set_error(err, err_len, "没有音频数据");
    return 0;
  }
  for (int32_t offset = 0; offset < n; offset += handle->window_size) {
    int32_t count = handle->window_size;
    if (offset + count > n) {
      count = n - offset;
    }
    handle->accept(handle->vad, samples + offset, count);
    if (!att_vad_drain_segments(handle, err, err_len)) {
      return 0;
    }
  }
  return 1;
}

/* 收到最后一段未闭合语音并返回区间 JSON（左闭右开，单位=采样点）。 */
char *att_vad_finish(AttVad *handle, char *err, int32_t err_len) {
  if (!handle || !handle->vad) {
    att_set_error(err, err_len, "VAD 未初始化");
    return NULL;
  }
  handle->flush(handle->vad);
  if (!att_vad_drain_segments(handle, err, err_len)) {
    return NULL;
  }

  size_t capacity = handle->range_count * 48 + 3;
  char *json = (char *)malloc(capacity);
  if (!json) {
    att_set_error(err, err_len, "内存分配失败");
    return NULL;
  }
  size_t used = 0;
  json[used++] = '[';
  for (size_t i = 0; i < handle->range_count; i++) {
    char item[64];
    int written = snprintf(item, sizeof(item), "%s[%lld,%lld]", i ? "," : "",
                           (long long)handle->ranges[i * 2],
                           (long long)handle->ranges[i * 2 + 1]);
    if (written <= 0 || used + (size_t)written + 2 > capacity) {
      free(json);
      att_set_error(err, err_len, "VAD 区间结果异常");
      return NULL;
    }
    memcpy(json + used, item, (size_t)written);
    used += (size_t)written;
  }
  json[used++] = ']';
  json[used] = '\0';
  return json;
}

/* 出错时释放 C 侧尚未导出的段，下一轮 start 会重新复位探测器。 */
void att_vad_abort(AttVad *handle) {
  if (!handle || !handle->vad) {
    return;
  }
  while (!handle->is_empty(handle->vad)) {
    const SherpaOnnxSpeechSegment *segment = handle->front(handle->vad);
    if (segment) {
      handle->destroy_segment(segment);
    }
    handle->pop(handle->vad);
  }
  handle->reset(handle->vad);
  handle->range_count = 0;
}

/* 兼容一次性调用；Rust 侧现在使用 start/accept/finish 分块喂入。 */
char *att_vad_speech_ranges(AttVad *handle, const float *samples, int32_t n, char *err,
                            int32_t err_len) {
  if (!att_vad_start(handle, err, err_len) ||
      !att_vad_accept(handle, samples, n, err, err_len)) {
    att_vad_abort(handle);
    return NULL;
  }
  char *json = att_vad_finish(handle, err, err_len);
  if (!json) {
    att_vad_abort(handle);
  }
  return json;
}

void att_vad_destroy(AttVad *handle) { att_vad_release(handle); }
