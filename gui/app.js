/* llmgui Web GUI — vanilla JS, zero build, offline.
 *
 * Security notes:
 *  - All dynamic text is written with textContent; no innerHTML anywhere.
 *  - The API key is stored in localStorage on this machine only and sent as a
 *    Bearer header to the configured API base only.
 */
'use strict';

(function () {
  /* ------------------------------------------------------------------ *
   * Static catalog (mirrors models/v0.toml, no network needed)
   * ------------------------------------------------------------------ */

  const DEFAULT_MODELS = [
    {
      id: 'qwen2.5-7b-instruct-q4km',
      name: 'Qwen2.5-7B-Instruct-GGUF',
      repo: 'Qwen/Qwen2.5-7B-Instruct-GGUF',
      file: 'qwen2.5-7b-instruct-q4_k_m.gguf',
      quant: 'Q4_K_M',
      size: '4.68 GB',
      license: 'Apache-2.0',
      role: 'default',
      purpose: '默认推理模型：日常聊天、问答与常规生成，7B 能力与显存占用较均衡。',
    },
    {
      id: 'qwen2.5-0.5b-instruct-q4km',
      name: 'Qwen2.5-0.5B-Instruct-GGUF',
      repo: 'Qwen/Qwen2.5-0.5B-Instruct-GGUF',
      file: 'qwen2.5-0.5b-instruct-q4_k_m.gguf',
      quant: 'Q4_K_M',
      size: '约 0.40 GB',
      license: 'Apache-2.0',
      role: 'smoke',
      purpose: '冒烟测试与低配机器验证：启动快、占用小，用于打通链路。',
    },
    {
      id: 'qwen2.5-7b-instruct',
      name: 'Qwen2.5-7B-Instruct',
      repo: 'Qwen/Qwen2.5-7B-Instruct',
      file: '',
      quant: 'BF16',
      size: '约 15.2 GB',
      license: 'Apache-2.0',
      role: 'train-base',
      purpose: 'Python 微调训练底座（SFT / QLoRA），不作为 GGUF 推理模型使用。',
    },
  ];

  const ROLE_LABELS = {
    default: '默认推理',
    smoke: '冒烟验证',
    'train-base': '训练底座',
  };

  const ROLE_TONES = {
    default: 'chip--ok',
    smoke: '',
    'train-base': 'chip--warn',
  };

  const VIEWS = ['models', 'chat', 'train', 'settings'];

  const STORAGE_KEYS = {
    apiBase: 'llmgui.apiBase',
    apiKey: 'llmgui.apiKey',
    theme: 'llmgui.theme',
  };

  const TRAIN_DEFAULTS = {
    baseModel: 'Qwen/Qwen2.5-7B-Instruct',
    datasetPath: 'data/sft.jsonl',
    method: 'qlora',
    seqLen: 2048,
    microBatch: 1,
    gradAccum: 16,
    loraR: 16,
    loraAlpha: 32,
  };

  /* ------------------------------------------------------------------ *
   * State + DOM cache
   * ------------------------------------------------------------------ */

  const state = {
    view: 'models',
    apiBase: '',
    apiKey: '',
    theme: 'dark',
    status: { online: false, health: null, error: null, checked: false },
    serverModels: [],
    trainYaml: '',
    chat: {
      history: [],
      controller: null,
      streaming: false,
      pinned: true,
    },
  };

  let dom = {};
  let healthTimer = null;

  /* ------------------------------------------------------------------ *
   * Small helpers
   * ------------------------------------------------------------------ */

  function createEl(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text !== undefined && text !== null) node.textContent = String(text);
    return node;
  }

  function readStorage(key) {
    try {
      return window.localStorage.getItem(key);
    } catch (error) {
      return null;
    }
  }

  function writeStorage(key, value) {
    try {
      if (value === null || value === undefined || value === '') {
        window.localStorage.removeItem(key);
      } else {
        window.localStorage.setItem(key, value);
      }
      return true;
    } catch (error) {
      toast('无法写入本机 localStorage，设置可能不会被记住。', 'warn');
      return false;
    }
  }

  function describeFetchError(error) {
    if (error && error.name === 'AbortError') return '请求超时（5 秒）';
    if (error instanceof TypeError) {
      return '无法连接服务，请确认 llmgui serve 已启动，或检查设置中的 API base';
    }
    if (error && typeof error.message === 'string' && error.message) return error.message;
    return String(error);
  }

  function formatUptime(seconds) {
    const total = Math.max(0, Math.floor(Number(seconds) || 0));
    if (total < 60) return total + 's';
    const minutes = Math.floor(total / 60);
    if (minutes < 60) return minutes + 'm ' + (total % 60) + 's';
    const hours = Math.floor(minutes / 60);
    return hours + 'h ' + (minutes % 60) + 'm';
  }

  function clampNumber(value, min, max, fallback) {
    const number = typeof value === 'number' ? value : Number.parseFloat(value);
    if (!Number.isFinite(number)) return fallback;
    return Math.min(max, Math.max(min, number));
  }

  function clampInt(value, min, max, fallback) {
    const number = Number.parseInt(value, 10);
    if (!Number.isFinite(number)) return fallback;
    return Math.min(max, Math.max(min, number));
  }

  /* ------------------------------------------------------------------ *
   * Toasts
   * ------------------------------------------------------------------ */

  function toast(message, tone) {
    if (!dom.toastStack) return;
    const item = createEl('div', 'toast' + (tone ? ' toast--' + tone : ''), message);
    dom.toastStack.appendChild(item);
    while (dom.toastStack.children.length > 4) {
      const first = dom.toastStack.firstElementChild;
      if (first) first.remove();
    }
    window.setTimeout(function () {
      item.remove();
    }, tone === 'error' ? 6000 : 3500);
  }

  /* ------------------------------------------------------------------ *
   * API helpers
   * ------------------------------------------------------------------ */

  function normalizeApiBase(value) {
    const trimmed = String(value || '').trim();
    if (!trimmed) return '';
    const withScheme = /^[a-z][a-z0-9+.-]*:\/\//i.test(trimmed) ? trimmed : 'http://' + trimmed;
    let parsed;
    try {
      parsed = new URL(withScheme);
    } catch (error) {
      throw new Error('API base 不是合法的 URL');
    }
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
      throw new Error('API base 只支持 http / https');
    }
    return parsed.href.replace(/\/+$/, '');
  }

  function apiUrl(path) {
    const base = state.apiBase;
    return base ? base + path : path;
  }

  function authHeaders(extra) {
    const headers = Object.assign({}, extra || {});
    const key = state.apiKey;
    if (key) headers.Authorization = 'Bearer ' + key;
    return headers;
  }

  async function readErrorDetail(response) {
    let message = '';
    const contentType = (response.headers.get('content-type') || '').toLowerCase();
    try {
      if (contentType.indexOf('application/json') !== -1) {
        const body = await response.json();
        if (body && body.error && typeof body.error.message === 'string') {
          message = body.error.message;
        } else if (body && typeof body.message === 'string') {
          message = body.message;
        }
      } else {
        const text = await response.text();
        message = text.trim().slice(0, 400);
      }
    } catch (error) {
      message = '';
    }
    if (!message) {
      message = 'HTTP ' + response.status + (response.statusText ? ' ' + response.statusText : '');
    }
    if (response.status === 401 || response.status === 403 || /invalid api key/i.test(message)) {
      message += '（服务端可能启用了 API key，请在设置页填写正确的 key）';
    }
    return message;
  }

  /* ------------------------------------------------------------------ *
   * Health / status bar
   * ------------------------------------------------------------------ */

  function setStatusChecking() {
    dom.status.classList.remove('is-online', 'is-offline');
    dom.status.classList.add('is-checking');
    dom.statusLabel.textContent = '检测中…';
    dom.statusMeta.textContent = '';
  }

  function renderStatus() {
    const online = state.status.online;
    const health = state.status.health || {};
    dom.status.classList.toggle('is-online', online);
    dom.status.classList.toggle('is-offline', !online);
    dom.status.classList.remove('is-checking');

    if (online) {
      const parts = [];
      if (health.backend) parts.push('backend ' + health.backend);
      if (health.model) parts.push('model ' + health.model);
      if (health.version) parts.push('v' + health.version);
      if (Number.isFinite(Number(health.uptime_secs))) {
        parts.push('uptime ' + formatUptime(health.uptime_secs));
      }
      if (Number.isFinite(Number(health.slots_available))) {
        parts.push('slots ' + health.slots_available + ' 空闲');
      }
      dom.statusLabel.textContent = '在线';
      dom.statusMeta.textContent = parts.join(' · ');
      dom.statusMeta.title = parts.join(' · ');
    } else {
      dom.statusLabel.textContent = '离线';
      dom.statusMeta.textContent = state.status.error || '无法连接服务';
      dom.statusMeta.title = state.status.error || '';
    }
    updateChatModel();
  }

  async function refreshHealth(options) {
    const silent = Boolean(options && options.silent);
    const notify = Boolean(options && options.notify);
    if (!silent) setStatusChecking();

    const controller = new AbortController();
    const timer = window.setTimeout(function () {
      controller.abort();
    }, 5000);

    try {
      const response = await fetch(apiUrl('/healthz'), {
        method: 'GET',
        headers: authHeaders(),
        cache: 'no-store',
        signal: controller.signal,
      });
      if (!response.ok) throw new Error(await readErrorDetail(response));
      const health = await response.json();
      state.status = { online: true, health: health, error: null, checked: true };
    } catch (error) {
      state.status = {
        online: false,
        health: null,
        error: describeFetchError(error),
        checked: true,
      };
      if (notify) toast('状态刷新失败：' + state.status.error, 'error');
    } finally {
      window.clearTimeout(timer);
    }

    renderStatus();
  }

  /* ------------------------------------------------------------------ *
   * Models view
   * ------------------------------------------------------------------ */

  function copyCommandFor(model) {
    if (model.file) {
      return 'llmgui pull ' + model.repo + ' --file ' + model.file;
    }
    return 'llmgui pull ' + model.repo;
  }

  function renderModelCards() {
    dom.modelGrid.textContent = '';
    DEFAULT_MODELS.forEach(function (model) {
      const card = createEl('article', 'model-card');

      const head = createEl('div', 'model-card__head');
      const titleWrap = createEl('div');
      const title = createEl('h3', 'model-card__title', model.name);
      const repo = createEl('p', 'model-card__repo', model.repo);
      titleWrap.appendChild(title);
      titleWrap.appendChild(repo);
      const roleChip = createEl(
        'span',
        'chip ' + (ROLE_TONES[model.role] || ''),
        ROLE_LABELS[model.role] || model.role
      );
      head.appendChild(titleWrap);
      head.appendChild(roleChip);

      const purpose = createEl('p', 'model-card__purpose', model.purpose);

      const facts = createEl('dl', 'model-card__facts');
      const factRows = [
        ['量化', model.quant],
        ['大小', model.size],
        ['许可', model.license],
        ['文件', model.file || '整仓库（训练底座）'],
      ];
      factRows.forEach(function (pair) {
        const row = createEl('div');
        row.appendChild(createEl('dt', '', pair[0]));
        row.appendChild(createEl('dd', '', pair[1]));
        facts.appendChild(row);
      });

      const command = copyCommandFor(model);
      const commandBox = createEl('div', 'model-card__command');
      const commandCode = createEl('code', '', command);
      const copyButton = createEl('button', 'btn btn--ghost btn--sm', '复制命令');
      copyButton.type = 'button';
      copyButton.dataset.copy = command;
      copyButton.setAttribute('aria-label', '复制 ' + model.name + ' 的拉取命令');
      commandBox.appendChild(commandCode);
      commandBox.appendChild(copyButton);

      card.appendChild(head);
      card.appendChild(purpose);
      card.appendChild(facts);
      card.appendChild(commandBox);
      dom.modelGrid.appendChild(card);
    });
  }

  async function refreshServerModels(options) {
    const notify = Boolean(options && options.notify);
    if (!dom.modelsServerStatus) return;
    dom.modelsServerStatus.textContent = '正在读取 GET /v1/models…';
    dom.modelsServerList.textContent = '';

    try {
      const response = await fetch(apiUrl('/v1/models'), {
        method: 'GET',
        headers: authHeaders(),
        cache: 'no-store',
      });
      if (!response.ok) throw new Error(await readErrorDetail(response));
      const body = await response.json();
      const models = body && Array.isArray(body.data) ? body.data : [];
      state.serverModels = models;

      if (!models.length) {
        dom.modelsServerStatus.textContent = '服务已连接，但当前没有已注册模型。';
      } else {
        dom.modelsServerStatus.textContent = '服务已连接，共 ' + models.length + ' 个可用模型：';
        models.forEach(function (item) {
          const id =
            item && typeof item.id === 'string' && item.id ? item.id : '未知模型';
          const chip = createEl('li', 'chip', id);
          const owner = item && typeof item.owned_by === 'string' ? item.owned_by : '';
          if (owner) chip.title = 'owned_by: ' + owner;
          dom.modelsServerList.appendChild(chip);
        });
      }
    } catch (error) {
      state.serverModels = [];
      dom.modelsServerStatus.textContent =
        '无法读取 /v1/models：' + describeFetchError(error);
      if (notify) toast('无法读取模型列表', 'error');
    }

    updateChatModel();
  }

  /* ------------------------------------------------------------------ *
   * Chat view
   * ------------------------------------------------------------------ */

  function currentModelId() {
    const health = state.status.health;
    if (health && typeof health.model === 'string' && health.model.trim()) {
      return health.model.trim();
    }
    if (state.serverModels.length) {
      const first = state.serverModels[0];
      if (first && typeof first.id === 'string' && first.id.trim()) {
        return first.id.trim();
      }
    }
    return 'local-model';
  }

  function updateChatModel() {
    if (!dom.chatModel) return;
    dom.chatModel.textContent = '模型：' + currentModelId();
    dom.chatModel.title = state.status.online
      ? ''
      : '服务离线，当前使用兜底模型 id；连接恢复后会自动更新';
  }

  function setChatStreaming(streaming) {
    state.chat.streaming = streaming;
    dom.chatSend.disabled = streaming;
    dom.chatStop.disabled = !streaming;
    dom.chatClear.disabled = streaming;
  }

  function setChatStatus(text, tone) {
    dom.chatStatus.textContent = text;
    dom.chatStatus.className = 'chip';
    if (tone === 'warn') dom.chatStatus.classList.add('chip--warn');
    if (tone === 'ok') dom.chatStatus.classList.add('chip--ok');
  }

  function scrollChat(force) {
    if (!dom.chatMessages) return;
    if (force || state.chat.pinned) {
      dom.chatMessages.scrollTop = dom.chatMessages.scrollHeight;
    }
  }

  function appendChatMessage(role, text) {
    const article = createEl('article', 'msg msg--' + role);
    const meta = createEl('div', 'msg__meta');
    const roleLabel = role === 'user' ? '你' : role === 'assistant' ? '助手' : '错误';
    const note =
      role === 'assistant'
        ? createEl('span', 'msg__note', '生成中…')
        : createEl('span', 'msg__note');
    meta.appendChild(createEl('span', 'msg__role', roleLabel));
    meta.appendChild(note);

    const body = createEl('div', 'msg__body', text);
    article.appendChild(meta);
    article.appendChild(body);
    if (role === 'assistant') article.classList.add('is-streaming');

    dom.chatMessages.appendChild(article);
    dom.chatEmpty.hidden = true;
    scrollChat(true);
    return { el: article, body: body, note: note };
  }

  function appendErrorMessage(message) {
    const article = createEl('article', 'msg msg--error');
    const meta = createEl('div', 'msg__meta');
    meta.appendChild(createEl('span', 'msg__role', '错误'));
    const body = createEl('div', 'msg__body', message);
    article.appendChild(meta);
    article.appendChild(body);
    dom.chatMessages.appendChild(article);
    dom.chatEmpty.hidden = true;
    scrollChat(true);
  }

  function buildStatsText(meta) {
    const parts = [];
    if (meta.ttftMs !== null && Number.isFinite(meta.ttftMs)) {
      parts.push('TTFT ' + Math.round(meta.ttftMs) + ' ms');
    }

    const usage = meta.usage && typeof meta.usage === 'object' ? meta.usage : null;
    const usageTokens =
      usage && Number.isFinite(Number(usage.completion_tokens))
        ? Number(usage.completion_tokens)
        : null;
    const tokens =
      usageTokens !== null ? usageTokens : Math.max(1, Math.ceil(meta.chars / 4));
    const decodeMs = Math.max(1, (meta.elapsedMs || 0) - (meta.ttftMs || 0));
    const tokensPerSecond = tokens / (decodeMs / 1000);
    const approx = usageTokens === null ? '≈' : '';
    parts.push(approx + tokensPerSecond.toFixed(1) + ' tok/s');
    parts.push(
      usageTokens === null
        ? '≈' + tokens + ' tok（按字符估算）'
        : tokens + ' completion tok'
    );
    if (usage && Number.isFinite(Number(usage.prompt_tokens))) {
      parts.push(Number(usage.prompt_tokens) + ' prompt tok');
    }
    if (meta.finishReason) parts.push('finish: ' + meta.finishReason);
    return parts.join(' · ');
  }

  function applyCompletionJson(json) {
    const result = { text: '', usage: null, finishReason: '' };
    if (!json || typeof json !== 'object') return result;
    if (json.error) {
      const message =
        json.error && typeof json.error.message === 'string'
          ? json.error.message
          : '服务返回错误';
      throw new Error(message);
    }
    if (json.usage && typeof json.usage === 'object') result.usage = json.usage;
    const choices = Array.isArray(json.choices) ? json.choices : [];
    const choice = choices[0];
    if (choice) {
      if (choice.message) {
        const content = choice.message.content;
        if (typeof content === 'string') {
          result.text = content;
        } else if (Array.isArray(content)) {
          result.text = content
            .map(function (part) {
              return part && typeof part.text === 'string' ? part.text : '';
            })
            .join('');
        }
      } else if (typeof choice.text === 'string') {
        result.text = choice.text;
      }
      if (typeof choice.finish_reason === 'string') result.finishReason = choice.finish_reason;
    }
    return result;
  }

  /**
   * Consume an SSE byte stream. Calls onPayload(payloadString) for every
   * `data:` line; `[DONE]` terminates. Comments/keep-alives are ignored.
   */
  async function consumeSse(reader, onPayload) {
    const decoder = new TextDecoder('utf-8');
    let buffer = '';
    let finished = false;

    while (!finished) {
      const chunk = await reader.read();
      if (chunk.done) break;
      buffer += decoder.decode(chunk.value, { stream: true });

      let newlineIndex = buffer.indexOf('\n');
      while (newlineIndex !== -1) {
        const rawLine = buffer.slice(0, newlineIndex);
        buffer = buffer.slice(newlineIndex + 1);
        newlineIndex = buffer.indexOf('\n');

        const line = rawLine.replace(/\r$/, '');
        if (!line || line.charAt(0) === ':') continue;
        if (line.indexOf('data:') !== 0) continue;

        let payload = line.slice(5);
        if (payload.charAt(0) === ' ') payload = payload.slice(1);
        payload = payload.trim();
        if (!payload) continue;

        if (payload === '[DONE]') {
          finished = true;
          break;
        }
        onPayload(payload);
      }
    }
  }

  async function sendChat() {
    if (state.chat.streaming) return;

    const text = dom.chatInput.value.trim();
    if (!text) {
      toast('请输入消息内容', 'warn');
      dom.chatInput.focus();
      return;
    }

    const temperature = clampNumber(dom.chatTemperature.value, 0, 2, 0.7);
    const maxTokens = clampInt(dom.chatMaxTokens.value, 1, 32768, 512);
    dom.chatTemperature.value = String(temperature);
    dom.chatMaxTokens.value = String(maxTokens);

    state.chat.history.push({ role: 'user', content: text });
    appendChatMessage('user', text);
    dom.chatInput.value = '';
    autoGrowComposer();
    dom.chatInput.focus();
    state.chat.pinned = true;

    const assistant = appendChatMessage('assistant', '');
    const controller = new AbortController();
    state.chat.controller = controller;
    setChatStreaming(true);
    setChatStatus('生成中…');

    const startedAt = window.performance.now();
    let firstTokenAt = null;
    let completionText = '';
    let usage = null;
    let finishReason = '';
    let finalized = false;

    const finalizePartial = function (label) {
      if (finalized) return;
      finalized = true;
      assistant.el.classList.remove('is-streaming');
      if (completionText) {
        state.chat.history.push({ role: 'assistant', content: completionText });
        assistant.body.textContent = completionText;
        assistant.note.textContent =
          label +
          ' · ' +
          buildStatsText({
            ttftMs: firstTokenAt === null ? null : firstTokenAt - startedAt,
            usage: usage,
            chars: completionText.length,
            elapsedMs: window.performance.now() - startedAt,
            finishReason: finishReason,
          });
      } else {
        assistant.el.remove();
      }
    };

    try {
      const response = await fetch(apiUrl('/v1/chat/completions'), {
        method: 'POST',
        headers: authHeaders({ 'Content-Type': 'application/json' }),
        cache: 'no-store',
        signal: controller.signal,
        body: JSON.stringify({
          model: currentModelId(),
          messages: state.chat.history.slice(),
          stream: true,
          temperature: temperature,
          max_tokens: maxTokens,
        }),
      });

      if (!response.ok) throw new Error(await readErrorDetail(response));

      const contentType = (response.headers.get('content-type') || '').toLowerCase();
      const isJson = contentType.indexOf('application/json') !== -1;

      if (!response.body || isJson) {
        const json = await response.json();
        const applied = applyCompletionJson(json);
        if (!applied.text) throw new Error('服务返回了空响应');
        completionText = applied.text;
        usage = applied.usage;
        finishReason = applied.finishReason;
        assistant.body.textContent = completionText;
        scrollChat(false);
      } else {
        const reader = response.body.getReader();
        await consumeSse(reader, function (payload) {
          let chunk;
          try {
            chunk = JSON.parse(payload);
          } catch (error) {
            return;
          }
          if (!chunk || typeof chunk !== 'object') return;
          if (chunk.error) {
            const message =
              chunk.error && typeof chunk.error.message === 'string'
                ? chunk.error.message
                : '服务返回错误';
            throw new Error(message);
          }
          if (chunk.usage && typeof chunk.usage === 'object') usage = chunk.usage;

          const choices = Array.isArray(chunk.choices) ? chunk.choices : [];
          choices.forEach(function (choice) {
            if (!choice) return;
            if (choice.delta && typeof choice.delta.content === 'string') {
              const delta = choice.delta.content;
              if (delta) {
                if (firstTokenAt === null) firstTokenAt = window.performance.now();
                completionText += delta;
                assistant.body.textContent = completionText;
                scrollChat(false);
              }
            }
            if (typeof choice.finish_reason === 'string' && choice.finish_reason) {
              finishReason = choice.finish_reason;
            }
          });
        });
      }

      if (!completionText) throw new Error('服务未返回任何内容');

      finalized = true;
      state.chat.history.push({ role: 'assistant', content: completionText });
      assistant.el.classList.remove('is-streaming');
      assistant.el.classList.add('is-done');
      assistant.note.textContent = buildStatsText({
        ttftMs: firstTokenAt === null ? null : firstTokenAt - startedAt,
        usage: usage,
        chars: completionText.length,
        elapsedMs: window.performance.now() - startedAt,
        finishReason: finishReason,
      });
      setChatStatus('就绪', 'ok');
      scrollChat(false);
    } catch (error) {
      if (error && error.name === 'AbortError') {
        finalizePartial('已停止');
        setChatStatus('已停止', 'warn');
        toast('已停止生成', 'warn');
      } else {
        finalizePartial('已中断');
        const message = describeFetchError(error);
        appendErrorMessage(message);
        setChatStatus('错误', 'warn');
        toast('请求失败：' + message, 'error');
        void refreshHealth({ silent: true });
      }
    } finally {
      state.chat.controller = null;
      setChatStreaming(false);
      state.chat.pinned = true;
      scrollChat(true);
    }
  }

  function stopChat() {
    if (state.chat.controller) {
      setChatStatus('正在停止…', 'warn');
      state.chat.controller.abort();
    }
  }

  function clearChat() {
    if (state.chat.streaming) {
      toast('请先停止生成，再清空对话', 'warn');
      return;
    }
    const nodes = dom.chatMessages.querySelectorAll('.msg');
    if (!state.chat.history.length && !nodes.length) return;
    if (!window.confirm('清空当前对话？此操作不可撤销。')) return;
    state.chat.history = [];
    nodes.forEach(function (node) {
      node.remove();
    });
    dom.chatEmpty.hidden = false;
    setChatStatus('就绪');
  }

  function autoGrowComposer() {
    if (!dom.chatInput) return;
    dom.chatInput.style.height = 'auto';
    dom.chatInput.style.height = Math.min(dom.chatInput.scrollHeight, 180) + 'px';
  }

  /* ------------------------------------------------------------------ *
   * Train view
   * ------------------------------------------------------------------ */

  function yamlScalar(value) {
    const text = String(value === null || value === undefined ? '' : value).trim();
    if (!text) return '""';
    const safe = /^[A-Za-z0-9_./@:+-]+$/.test(text);
    const reserved = /^(true|false|null|yes|no|on|off|-?\d+(\.\d+)?)$/i.test(text);
    if (safe && !reserved) return text;
    return JSON.stringify(text);
  }

  function readTrainConfig() {
    return {
      baseModel: dom.trainBaseModel.value.trim() || TRAIN_DEFAULTS.baseModel,
      datasetPath: dom.trainDatasetPath.value.trim() || TRAIN_DEFAULTS.datasetPath,
      method: dom.trainMethod.value === 'lora' ? 'lora' : 'qlora',
      seqLen: clampInt(dom.trainSeqLen.value, 128, 32768, TRAIN_DEFAULTS.seqLen),
      microBatch: clampInt(dom.trainMicroBatch.value, 1, 64, TRAIN_DEFAULTS.microBatch),
      gradAccum: clampInt(dom.trainGradAccum.value, 1, 1024, TRAIN_DEFAULTS.gradAccum),
      loraR: clampInt(dom.trainLoraR.value, 1, 256, TRAIN_DEFAULTS.loraR),
      loraAlpha: clampInt(dom.trainLoraAlpha.value, 1, 512, TRAIN_DEFAULTS.loraAlpha),
    };
  }

  function buildTrainYaml(config) {
    const lines = [
      'base_model: ' + yamlScalar(config.baseModel),
      'method: ' + config.method,
    ];
    if (config.method === 'qlora') lines.push('load_in_4bit: true');
    lines.push(
      'lora:',
      '  r: ' + config.loraR,
      '  alpha: ' + config.loraAlpha,
      '  dropout: 0.05',
      '  target_modules: all-linear',
      'seq_len: ' + config.seqLen,
      'micro_batch: ' + config.microBatch,
      'grad_accum: ' + config.gradAccum,
      'packing: true',
      'gradient_checkpointing: true',
      'optim: adamw_8bit',
      'dataset:',
      '  path: ' + yamlScalar(config.datasetPath),
      '  format: sharegpt',
      'output_dir: jobs/my-sft'
    );
    return lines.join('\n') + '\n';
  }

  function updateTrainPreview() {
    if (!dom.trainYaml) return;
    const yaml = buildTrainYaml(readTrainConfig());
    state.trainYaml = yaml;
    dom.trainYaml.textContent = yaml;
    dom.trainCopyYaml.dataset.copy = yaml;
    dom.trainCopyCommand.dataset.copy = 'llmgui train --config train.yaml';
  }

  function downloadTrainYaml() {
    const blob = new Blob([state.trainYaml || ''], { type: 'text/yaml;charset=utf-8' });
    const url = URL.createObjectURL(blob);
    const link = document.createElement('a');
    link.href = url;
    link.download = 'train.yaml';
    document.body.appendChild(link);
    link.click();
    link.remove();
    window.setTimeout(function () {
      URL.revokeObjectURL(url);
    }, 1000);
    toast('已生成 train.yaml 下载', 'ok');
  }

  /* ------------------------------------------------------------------ *
   * Settings view
   * ------------------------------------------------------------------ */

  function applyTheme(theme, options) {
    state.theme = theme === 'light' ? 'light' : 'dark';
    document.documentElement.setAttribute('data-theme', state.theme);
    if (dom.themeToggle) dom.themeToggle.checked = state.theme === 'light';
    if (dom.settingsTheme) dom.settingsTheme.value = state.theme;
    if (!options || options.persist !== false) {
      writeStorage(STORAGE_KEYS.theme, state.theme);
    }
  }

  function loadSettings() {
    state.apiBase = readStorage(STORAGE_KEYS.apiBase) || '';
    state.apiKey = readStorage(STORAGE_KEYS.apiKey) || '';
    state.theme = readStorage(STORAGE_KEYS.theme) === 'light' ? 'light' : 'dark';

    dom.settingsApiBase.value = state.apiBase;
    dom.settingsApiKey.value = state.apiKey;
    applyTheme(state.theme, { persist: false });
  }

  function setTestResult(message, tone) {
    dom.settingsTestResult.textContent = message;
    dom.settingsTestResult.className = 'test-result';
    if (tone === 'ok') dom.settingsTestResult.classList.add('is-ok');
    if (tone === 'error') dom.settingsTestResult.classList.add('is-error');
  }

  function saveSettings(event) {
    event.preventDefault();
    let base = '';
    try {
      base = normalizeApiBase(dom.settingsApiBase.value);
    } catch (error) {
      setTestResult(error.message, 'error');
      toast(error.message, 'error');
      dom.settingsApiBase.focus();
      return;
    }

    state.apiBase = base;
    state.apiKey = dom.settingsApiKey.value.trim();
    dom.settingsApiBase.value = state.apiBase;
    dom.settingsApiKey.value = state.apiKey;

    writeStorage(STORAGE_KEYS.apiBase, state.apiBase);
    writeStorage(STORAGE_KEYS.apiKey, state.apiKey);

    setTestResult('设置已保存在本机浏览器。', 'ok');
    toast('设置已保存', 'ok');
    applyTheme(dom.settingsTheme.value);
    void refreshHealth({ notify: true });
    void refreshServerModels();
  }

  function clearSettings() {
    if (!window.confirm('清除本机保存的 API base、API key 与主题偏好？')) return;
    writeStorage(STORAGE_KEYS.apiBase, null);
    writeStorage(STORAGE_KEYS.apiKey, null);
    writeStorage(STORAGE_KEYS.theme, null);
    state.apiBase = '';
    state.apiKey = '';
    dom.settingsApiBase.value = '';
    dom.settingsApiKey.value = '';
    setTestResult('已清除本机保存的设置。', 'ok');
    applyTheme('dark');
    toast('已清除设置', 'ok');
    void refreshHealth({ notify: true });
    void refreshServerModels();
  }

  async function testConnection() {
    let base = '';
    try {
      base = normalizeApiBase(dom.settingsApiBase.value);
    } catch (error) {
      setTestResult(error.message, 'error');
      return;
    }

    const key = dom.settingsApiKey.value.trim();
    const button = dom.settingsTest;
    button.disabled = true;
    setTestResult('正在连接 ' + (base || '当前同源服务') + ' …');

    const controller = new AbortController();
    const timer = window.setTimeout(function () {
      controller.abort();
    }, 5000);

    try {
      const response = await fetch((base ? base : '') + '/v1/models', {
        method: 'GET',
        headers: key ? { Authorization: 'Bearer ' + key } : {},
        cache: 'no-store',
        signal: controller.signal,
      });
      if (!response.ok) throw new Error(await readErrorDetail(response));
      const body = await response.json();
      const models = body && Array.isArray(body.data) ? body.data : [];
      const ids = models
        .map(function (item) {
          return item && typeof item.id === 'string' ? item.id : '';
        })
        .filter(Boolean);
      setTestResult(
        '连接成功：/v1/models 返回 ' + models.length + ' 个模型' +
          (ids.length ? '（' + ids.join(', ') + '）' : ''),
        'ok'
      );
    } catch (error) {
      setTestResult('连接失败：' + describeFetchError(error), 'error');
    } finally {
      window.clearTimeout(timer);
      button.disabled = false;
    }
  }

  /* ------------------------------------------------------------------ *
   * Clipboard (with fallback for non-secure contexts)
   * ------------------------------------------------------------------ */

  async function copyText(text) {
    if (navigator.clipboard && window.isSecureContext) {
      try {
        await navigator.clipboard.writeText(text);
        return true;
      } catch (error) {
        /* fall through to execCommand */
      }
    }
    try {
      const area = document.createElement('textarea');
      area.value = text;
      area.setAttribute('readonly', '');
      area.style.position = 'fixed';
      area.style.top = '-1000px';
      area.style.opacity = '0';
      document.body.appendChild(area);
      area.select();
      const ok = document.execCommand('copy');
      area.remove();
      return ok;
    } catch (error) {
      return false;
    }
  }

  /* ------------------------------------------------------------------ *
   * Navigation / view switching
   * ------------------------------------------------------------------ */

  function showView(view, options) {
    const updateHash = !options || options.updateHash !== false;
    const target = VIEWS.indexOf(view) !== -1 ? view : 'models';
    state.view = target;

    Object.keys(dom.views).forEach(function (name) {
      dom.views[name].hidden = name !== target;
    });

    dom.navItems.forEach(function (item) {
      const active = item.dataset.view === target;
      item.classList.toggle('is-active', active);
      if (active) item.setAttribute('aria-current', 'page');
      else item.removeAttribute('aria-current');
    });

    if (updateHash) {
      const hash = '#' + target;
      if (window.location.hash !== hash) {
        window.history.replaceState(null, '', hash);
      }
    }

    if (target === 'models') void refreshServerModels();
    if (target === 'chat') {
      updateChatModel();
      dom.chatInput.focus();
    }
  }

  /* ------------------------------------------------------------------ *
   * Bindings + init
   * ------------------------------------------------------------------ */

  function cacheDom() {
    dom = {
      nav: document.getElementById('nav'),
      navItems: Array.prototype.slice.call(
        document.querySelectorAll('.nav__item[data-view]')
      ),
      status: document.getElementById('status'),
      statusLabel: document.getElementById('status-label'),
      statusMeta: document.getElementById('status-meta'),
      themeToggle: document.getElementById('theme-toggle'),
      refreshHealth: document.getElementById('refresh-health'),
      views: {
        models: document.getElementById('view-models'),
        chat: document.getElementById('view-chat'),
        train: document.getElementById('view-train'),
        settings: document.getElementById('view-settings'),
      },
      modelGrid: document.getElementById('model-grid'),
      modelsServerStatus: document.getElementById('models-server-status'),
      modelsServerList: document.getElementById('models-server-list'),
      modelsRefresh: document.getElementById('models-refresh'),
      chatModel: document.getElementById('chat-model'),
      chatStatus: document.getElementById('chat-status'),
      chatClear: document.getElementById('chat-clear'),
      chatMessages: document.getElementById('chat-messages'),
      chatEmpty: document.getElementById('chat-empty'),
      chatForm: document.getElementById('chat-form'),
      chatInput: document.getElementById('chat-input'),
      chatSend: document.getElementById('chat-send'),
      chatStop: document.getElementById('chat-stop'),
      chatTemperature: document.getElementById('chat-temperature'),
      chatMaxTokens: document.getElementById('chat-max-tokens'),
      trainForm: document.getElementById('train-form'),
      trainBaseModel: document.getElementById('train-base-model'),
      trainDatasetPath: document.getElementById('train-dataset-path'),
      trainMethod: document.getElementById('train-method'),
      trainSeqLen: document.getElementById('train-seq-len'),
      trainMicroBatch: document.getElementById('train-micro-batch'),
      trainGradAccum: document.getElementById('train-grad-accum'),
      trainLoraR: document.getElementById('train-lora-r'),
      trainLoraAlpha: document.getElementById('train-lora-alpha'),
      trainYaml: document.getElementById('train-yaml'),
      trainCopyYaml: document.getElementById('train-copy-yaml'),
      trainCopyCommand: document.getElementById('train-copy-command'),
      trainDownloadYaml: document.getElementById('train-download-yaml'),
      settingsForm: document.getElementById('settings-form'),
      settingsApiBase: document.getElementById('settings-api-base'),
      settingsApiKey: document.getElementById('settings-api-key'),
      settingsToggleKey: document.getElementById('settings-toggle-key'),
      settingsTheme: document.getElementById('settings-theme'),
      settingsTest: document.getElementById('settings-test'),
      settingsClear: document.getElementById('settings-clear'),
      settingsTestResult: document.getElementById('settings-test-result'),
      toastStack: document.getElementById('toast-stack'),
    };
  }

  function bindEvents() {
    dom.nav.addEventListener('click', function (event) {
      const target = event.target instanceof Element
        ? event.target.closest('[data-view]')
        : null;
      if (target && target.dataset.view) showView(target.dataset.view);
    });

    window.addEventListener('hashchange', function () {
      showView(window.location.hash.replace(/^#/, ''), { updateHash: false });
    });

    dom.refreshHealth.addEventListener('click', function () {
      void refreshHealth({ notify: true });
    });

    dom.modelsRefresh.addEventListener('click', function () {
      void refreshServerModels({ notify: true });
    });

    dom.themeToggle.addEventListener('change', function () {
      applyTheme(dom.themeToggle.checked ? 'light' : 'dark');
    });

    dom.settingsTheme.addEventListener('change', function () {
      applyTheme(dom.settingsTheme.value);
    });

    /* Chat */
    dom.chatForm.addEventListener('submit', function (event) {
      event.preventDefault();
      void sendChat();
    });
    dom.chatInput.addEventListener('input', autoGrowComposer);
    dom.chatInput.addEventListener('keydown', function (event) {
      if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
        event.preventDefault();
        void sendChat();
      }
    });
    dom.chatStop.addEventListener('click', stopChat);
    dom.chatClear.addEventListener('click', clearChat);
    dom.chatMessages.addEventListener('scroll', function () {
      const box = dom.chatMessages;
      state.chat.pinned = box.scrollHeight - box.scrollTop - box.clientHeight < 100;
    });

    /* Train */
    dom.trainForm.addEventListener('submit', function (event) {
      event.preventDefault();
    });
    dom.trainForm.addEventListener('input', updateTrainPreview);
    dom.trainForm.addEventListener('change', updateTrainPreview);
    dom.trainDownloadYaml.addEventListener('click', downloadTrainYaml);

    /* Settings */
    dom.settingsForm.addEventListener('submit', saveSettings);
    dom.settingsClear.addEventListener('click', clearSettings);
    dom.settingsTest.addEventListener('click', function () {
      void testConnection();
    });
    dom.settingsToggleKey.addEventListener('click', function () {
      const visible = dom.settingsApiKey.type === 'text';
      dom.settingsApiKey.type = visible ? 'password' : 'text';
      dom.settingsToggleKey.textContent = visible ? '显示' : '隐藏';
      dom.settingsToggleKey.setAttribute('aria-pressed', visible ? 'false' : 'true');
    });

    /* Copy buttons (model commands, train YAML/command) */
    document.addEventListener('click', function (event) {
      const button = event.target instanceof Element
        ? event.target.closest('[data-copy]')
        : null;
      if (!button) return;
      const text = button.dataset.copy || '';
      if (!text) return;
      void copyText(text).then(function (ok) {
        toast(ok ? '已复制到剪贴板' : '复制失败，请手动选择文本', ok ? 'ok' : 'error');
      });
    });

    document.addEventListener('visibilitychange', function () {
      if (!document.hidden) void refreshHealth({ silent: true });
    });
    window.addEventListener('online', function () {
      void refreshHealth({ silent: true });
    });
    window.addEventListener('offline', function () {
      state.status = {
        online: false,
        health: null,
        error: '浏览器处于离线状态',
        checked: true,
      };
      renderStatus();
    });
    window.addEventListener('beforeunload', function () {
      if (state.chat.controller) state.chat.controller.abort();
    });
  }

  function startPolling() {
    if (healthTimer !== null) window.clearInterval(healthTimer);
    healthTimer = window.setInterval(function () {
      if (!document.hidden) void refreshHealth({ silent: true });
    }, 15000);
  }

  function init() {
    cacheDom();
    bindEvents();
    loadSettings();
    renderModelCards();
    updateTrainPreview();
    setStatusChecking();
    setChatStatus('就绪');
    autoGrowComposer();

    const initialView = window.location.hash.replace(/^#/, '');
    showView(initialView || 'models', { updateHash: false });

    void refreshHealth();
    if (state.view !== 'models') void refreshServerModels();
    startPolling();
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
