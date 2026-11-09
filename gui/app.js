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
      sizeBytes: 4683073632,
      vram: '≈8.5 GB @32k',
      fit: 'ok',
      license: 'Apache-2.0',
      role: 'default',
      tags: ['chat'],
      purpose: '默认推理模型：日常聊天、问答与常规生成，7B 能力与显存占用较均衡。',
    },
    {
      id: 'qwen3-8b-q4km',
      name: 'Qwen3-8B-GGUF',
      repo: 'Qwen/Qwen3-8B-GGUF',
      file: 'Qwen3-8B-Q4_K_M.gguf',
      quant: 'Q4_K_M',
      size: '约 5.0 GB',
      sizeBytes: 5027783488,
      vram: '≈9.2 GB @32k',
      fit: 'ok',
      license: 'Apache-2.0',
      role: 'balanced',
      tags: ['chat'],
      purpose: '7B 的升级档：同价位质量更好，24GB/32GB 都从容，适合作为日常默认。',
    },
    {
      id: 'qwen3-4b-q4km',
      name: 'Qwen3-4B-GGUF',
      repo: 'Qwen/Qwen3-4B-GGUF',
      file: 'Qwen3-4B-Q4_K_M.gguf',
      quant: 'Q4_K_M',
      size: '约 2.5 GB',
      sizeBytes: 2497280256,
      vram: '≈6.8 GB @32k',
      fit: 'ok',
      license: 'Apache-2.0',
      role: 'lite',
      tags: ['chat'],
      purpose: '轻量通用档：小显存/CPU 也能跑，响应快，适合草稿、分类与批量任务。',
    },
    {
      id: 'qwen2.5-coder-7b-q4km',
      name: 'Qwen2.5-Coder-7B-Instruct-GGUF',
      repo: 'Qwen/Qwen2.5-Coder-7B-Instruct-GGUF',
      file: 'qwen2.5-coder-7b-instruct-q4_k_m.gguf',
      quant: 'Q4_K_M',
      size: '约 4.68 GB',
      sizeBytes: 4683073536,
      vram: '≈8.5 GB @32k',
      fit: 'ok',
      license: 'Apache-2.0',
      role: 'coder',
      tags: ['chat', 'coding'],
      purpose: '代码专用档：补全、重构与多语言问答，体积与 7B 通用档相当。',
    },
    {
      id: 'phi-4-14b-q4k',
      name: 'Phi-4-GGUF',
      repo: 'microsoft/phi-4-gguf',
      file: 'phi-4-Q4_K.gguf',
      quant: 'Q4_K',
      size: '约 9.05 GB',
      sizeBytes: 9053114560,
      vram: '≈14.3 GB @32k',
      fit: 'ok',
      license: 'MIT',
      role: 'reasoning',
      tags: ['chat', 'reasoning', 'math'],
      purpose: 'MIT 许可的 14B 推理档：数学与逻辑较强，适合作为非 Qwen 家族的对照模型。',
    },
    {
      id: 'qwen3-30b-a3b-q4km',
      name: 'Qwen3-30B-A3B-GGUF',
      repo: 'Qwen/Qwen3-30B-A3B-GGUF',
      file: 'Qwen3-30B-A3B-Q4_K_M.gguf',
      quant: 'Q4_K_M',
      size: '约 18.56 GB',
      sizeBytes: 18556685824,
      vram: '≈22 GB @32k',
      fit: 'ok',
      license: 'Apache-2.0',
      role: 'moe',
      tags: ['chat', 'reasoning', 'moe'],
      purpose: 'MoE 档：总参数 30B、激活约 3B，decode 速度快；32GB 显存可跑，24GB 需降 ctx。',
    },
    {
      id: 'qwen3-coder-30b-a3b-q4km',
      name: 'Qwen3-Coder-30B-A3B-Instruct-GGUF',
      repo: 'unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF',
      file: 'Qwen3-Coder-30B-A3B-Instruct-Q4_K_M.gguf',
      quant: 'Q4_K_M',
      size: '约 18.56 GB',
      sizeBytes: 18556689568,
      vram: '≈21.5 GB @8k',
      fit: 'ok',
      license: 'Apache-2.0',
      role: 'coder',
      tags: ['chat', 'coding', 'moe', 'tool-use'],
      purpose: '代码专用 MoE 档：30B 总参数、约 3B 激活；配合 turbo 性能档（8k ctx / KV q4_0 / FA）目标 90–120 tok/s。',
    },
    {
      id: 'mistral-small-24b-q4km',
      name: 'Mistral-Small-24B-Instruct-2501-GGUF',
      repo: 'bartowski/Mistral-Small-24B-Instruct-2501-GGUF',
      file: 'Mistral-Small-24B-Instruct-2501-Q4_K_M.gguf',
      quant: 'Q4_K_M',
      size: '约 14.33 GB',
      sizeBytes: 14333908672,
      vram: '≈19 GB @32k',
      fit: 'ok',
      license: 'Apache-2.0',
      role: 'large',
      tags: ['chat', 'tool-use'],
      purpose: 'Apache-2.0 的 24B 通用档：多语言与长文能力均衡，32GB 卡可跑；社区 GGUF 由 bartowski 维护。',
    },
    {
      id: 'qwen2.5-0.5b-instruct-q4km',
      name: 'Qwen2.5-0.5B-Instruct-GGUF',
      repo: 'Qwen/Qwen2.5-0.5B-Instruct-GGUF',
      file: 'qwen2.5-0.5b-instruct-q4_k_m.gguf',
      quant: 'Q4_K_M',
      size: '约 0.40 GB',
      sizeBytes: 398000000,
      vram: '≈2 GB @32k',
      fit: 'ok',
      license: 'Apache-2.0',
      role: 'smoke',
      tags: ['chat'],
      purpose: '冒烟测试与低配机器验证：启动快、占用小，用于打通链路。',
    },
    {
      id: 'qwen3-14b-q4km',
      name: 'Qwen3-14B-GGUF',
      repo: 'Qwen/Qwen3-14B-GGUF',
      file: 'Qwen3-14B-Q4_K_M.gguf',
      quant: 'Q4_K_M',
      size: '约 9.0 GB',
      sizeBytes: 9000000000,
      vram: '≈13.8 GB @32k',
      fit: 'ok',
      license: 'Apache-2.0',
      role: 'large',
      tags: ['chat', 'reasoning'],
      purpose:
        '5090D 32G 主力大模型：14B 能力明显强于 7B，32k 上下文 + q8_0 KV 仍有约 18 GB 余量。',
    },
    {
      id: 'qwen3-32b-q4km',
      name: 'Qwen3-32B-GGUF',
      repo: 'Qwen/Qwen3-32B-GGUF',
      file: 'Qwen3-32B-Q4_K_M.gguf',
      quant: 'Q4_K_M',
      size: '约 20.3 GB',
      sizeBytes: 20300000000,
      vram: '≈25.4 GB @32k',
      fit: 'tight',
      license: 'Apache-2.0',
      role: 'large',
      tags: ['chat', 'reasoning'],
      purpose:
        '32G 单卡上限尝试：Q4_K_M + q8_0 KV @32k 约 25 GB，建议 ctx 降到 16k 并单路并发，避免 OOM。',
    },
    {
      id: 'qwen2.5-7b-instruct',
      name: 'Qwen2.5-7B-Instruct',
      repo: 'Qwen/Qwen2.5-7B-Instruct',
      file: '',
      quant: 'BF16',
      size: '约 15.2 GB',
      sizeBytes: 15200000000,
      vram: '',
      fit: '',
      license: 'Apache-2.0',
      role: 'train-base',
      tags: ['train-base'],
      purpose: 'Python 微调训练底座（SFT / QLoRA），不作为 GGUF 推理模型使用。',
    },
  ];

  const ROLE_LABELS = {
    default: '默认推理',
    large: '大模型',
    smoke: '冒烟验证',
    'train-base': '训练底座',
  };

  const ROLE_TONES = {
    default: 'chip--ok',
    large: 'chip--accent',
    smoke: '',
    'train-base': 'chip--warn',
  };

  const CATALOG_ROLE_LABELS = {
    default: '默认',
    balanced: '均衡',
    lite: '轻量',
    coder: '代码',
    reasoning: '推理',
    moe: 'MoE',
    large: '大模型',
    smoke: '冒烟',
    'train-base': '训练底座',
  };

  /* Stable capability tag IDs (mirrors llmgui-models::tags). */
  const TAG_LABELS = {
    chat: '通用对话',
    coding: '编程',
    reasoning: '推理',
    math: '数学',
    multimodal: '多模态',
    vision: '视觉',
    audio: '音频',
    embedding: '向量',
    rerank: '重排',
    'long-context': '长上下文',
    moe: 'MoE',
    'tool-use': '工具调用',
    'train-base': '训练底座',
  };

  const TAG_ORDER = Object.keys(TAG_LABELS);
  const TAG_RUNTIME_UNSUPPORTED = ['multimodal', 'vision', 'audio'];
  const TAG_SOURCE_LABELS = {
    user: '用户确认',
    curated: '目录策展',
    metadata: '模型元数据',
    file: '仓库文件',
    name: '名称推断',
  };
  const CATALOG_SOURCE_LABELS = {
    remote: '远程目录',
    cache: '本地缓存',
    bundled: '内置清单',
  };

  const MODEL_FILTERS = [
    { id: 'all', label: '全部' },
    { id: 'fit32', label: '32G 可跑' },
    { id: 'fit24', label: '24G 可跑' },
    { id: 'loaded', label: '已加载' },
    { id: 'inference', label: '推理' },
    { id: 'train', label: '训练底座' },
  ];

  const VIEWS = ['models', 'chat', 'train', 'settings'];

  const STORAGE_KEYS = {
    apiBase: 'llmgui.apiBase',
    apiKey: 'llmgui.apiKey',
    theme: 'llmgui.theme',
    catalogTags: 'llmgui.catalogTags',
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
    modelFilter: 'all',
    modelQuery: '',
    modelSort: 'name',
    modelTab: 'library',
    libraryModels: [],
    libraryLoaded: null,
    libraryGeneration: 0,
    libraryError: null,
    selectedModel: null,
    catalog: [],
    catalogMeta: null,
    catalogTags: [],
    repoResult: null,
    downloads: [],
    storage: null,
    modelTimer: null,
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

  function formatBytes(bytes) {
    const value = Number(bytes) || 0;
    if (!value) return '—';
    const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB'];
    let index = 0;
    let size = value;
    while (size >= 1024 && index < units.length - 1) {
      size /= 1024;
      index += 1;
    }
    return size.toFixed(index === 0 ? 0 : 2) + ' ' + units[index];
  }

  function formatWhen(unix) {
    if (!unix) return '—';
    try {
      return new Date(unix * 1000).toLocaleString();
    } catch (error) {
      return '—';
    }
  }

  function copyCommandFor(model) {
    if (model.file) {
      return 'llmgui pull ' + model.repo + ' --file ' + model.file;
    }
    return 'llmgui pull ' + model.repo;
  }

  function fitBadge(entry) {
    if (entry.fit_32gb && entry.fit_24gb) {
      return createEl('span', 'chip chip--ok', '24G / 32G 可跑');
    }
    if (entry.fit_32gb) {
      const chip = createEl('span', 'chip chip--ok', '5090D 32G 可跑');
      chip.title = '按体积 + q8_0 KV @32k 估算可加载';
      return chip;
    }
    const chip = createEl('span', 'chip chip--warn', '显存紧张');
    chip.title = '估算超过 32GB 预算；建议降 ctx / 换量化';
    return chip;
  }

  function libraryMatchesFilter(entry, filter) {
    if (filter === 'all') return true;
    if (filter === 'fit32') return Boolean(entry.fit_32gb);
    if (filter === 'fit24') return Boolean(entry.fit_24gb);
    if (filter === 'loaded') return Boolean(entry.loaded);
    if (filter === 'train') return entry.kind === 'train';
    if (filter === 'inference') return entry.kind !== 'train';
    return true;
  }

  async function updateModelTags(id, action, tag) {
    const entry = state.libraryModels.find(function (model) { return model.id === id; });
    if (!entry) return;
    const confirmed = (entry.prefs_tags || []).slice();
    const hidden = (entry.hidden_tags || []).slice();
    if (action === 'confirm-tag') {
      if (confirmed.indexOf(tag) === -1) confirmed.push(tag);
      const index = hidden.indexOf(tag);
      if (index !== -1) hidden.splice(index, 1);
    } else {
      const index = confirmed.indexOf(tag);
      if (index !== -1) confirmed.splice(index, 1);
      if (hidden.indexOf(tag) === -1) hidden.push(tag);
    }
    try {
      const response = await fetch(apiUrl('/admin/models/' + encodeURIComponent(id) + '/tags'), {
        method: 'PUT',
        headers: authHeaders({ 'Content-Type': 'application/json' }),
        body: JSON.stringify({ confirmed: confirmed, hidden: hidden }),
      });
      if (!response.ok) throw new Error(await readErrorDetail(response));
      toast(action === 'confirm-tag' ? '标签已确认' : '标签已隐藏', 'ok');
      await refreshLibrary();
    } catch (error) {
      toast('更新标签失败：' + describeFetchError(error), 'error');
    }
  }

  function visibleLibrary() {
    const query = state.modelQuery.trim().toLowerCase();
    const list = state.libraryModels.filter(function (entry) {
      if (!libraryMatchesFilter(entry, state.modelFilter)) return false;
      if (!query) return true;
      const haystack = [
        entry.id,
        entry.display_name,
        entry.repo,
        entry.quant,
        entry.alias,
        entry.license,
      ]
        .map(function (value) {
          return value || '';
        })
        .join(' ')
        .toLowerCase();
      return haystack.indexOf(query) !== -1;
    });
    if (state.modelSort === 'size') {
      list.sort(function (a, b) {
        return (b.size_bytes || 0) - (a.size_bytes || 0);
      });
    } else if (state.modelSort === 'recent') {
      list.sort(function (a, b) {
        return (b.last_used || b.installed_at || 0) - (a.last_used || a.installed_at || 0);
      });
    } else {
      list.sort(function (a, b) {
        return String(a.id).localeCompare(String(b.id));
      });
    }
    return list;
  }

  function buildModelFilters() {
    if (!dom.modelFilters) return;
    dom.modelFilters.textContent = '';
    MODEL_FILTERS.forEach(function (filter) {
      const count = state.libraryModels.filter(function (entry) {
        return libraryMatchesFilter(entry, filter.id);
      }).length;
      const button = createEl('button', 'filter-chip', filter.label + ' ' + count);
      button.type = 'button';
      button.dataset.filter = filter.id;
      dom.modelFilters.appendChild(button);
    });
    updateModelFilters();
  }

  function updateModelFilters() {
    if (!dom.modelFilters) return;
    Array.prototype.forEach.call(dom.modelFilters.children, function (button) {
      const active = button.dataset.filter === state.modelFilter;
      button.classList.toggle('is-active', active);
      button.setAttribute('aria-pressed', active ? 'true' : 'false');
    });
  }

  function buildModelTags(entry) {
    const row = createEl('div', 'model-card__chips model-card__tags');
    (entry.tags || []).forEach(function (tag) {
      if (!tag.suggested) {
        row.appendChild(catalogTagChip(tag));
        return;
      }
      const group = createEl('span', 'tag-actions');
      const confirm = createEl('button', 'chip chip--tag chip--suggested', '＋ ' + tag.label);
      confirm.type = 'button';
      confirm.dataset.action = 'confirm-tag';
      confirm.dataset.id = entry.id;
      confirm.dataset.tag = tag.id;
      confirm.title = '确认该建议标签（来源：' + (TAG_SOURCE_LABELS[tag.source] || tag.source) + '）';
      const hide = createEl('button', 'chip chip--hide', '隐藏');
      hide.type = 'button';
      hide.dataset.action = 'hide-tag';
      hide.dataset.id = entry.id;
      hide.dataset.tag = tag.id;
      hide.title = '隐藏该建议标签，不再显示';
      group.appendChild(confirm);
      group.appendChild(hide);
      row.appendChild(group);
    });
    return row.children.length ? row : null;
  }

  function buildLibraryCard(entry) {
    const card = createEl('article', 'model-card');
    card.dataset.id = entry.id;
    card.dataset.role = entry.loaded ? 'default' : entry.derived_from ? 'large' : '';

    const head = createEl('div', 'model-card__head');
    const titleWrap = createEl('div', 'model-card__title-wrap');
    titleWrap.appendChild(createEl('h3', 'model-card__title', entry.display_name || entry.id));
    const repoLine = entry.repo || entry.path || '';
    titleWrap.appendChild(createEl('p', 'model-card__repo', repoLine));

    const chips = createEl('div', 'model-card__chips');
    chips.appendChild(fitBadge(entry));
    if (entry.loaded) chips.appendChild(createEl('span', 'chip chip--accent', '已加载'));
    if (entry.kind === 'train') chips.appendChild(createEl('span', 'chip chip--warn', '训练底座'));
    if (entry.derived_from) chips.appendChild(createEl('span', 'chip chip--accent', '派生模型'));
    head.appendChild(titleWrap);
    head.appendChild(chips);

    const facts = createEl('dl', 'model-card__facts');
    [
      ['量化', entry.quant || '—'],
      ['体积', formatBytes(entry.size_bytes)],
      ['显存估算', entry.vram_human || '未估算'],
      ['上下文', entry.context && entry.context.max ? String(entry.context.max) : '—'],
    ].forEach(function (pair) {
      const row = createEl('div');
      row.appendChild(createEl('dt', '', pair[0]));
      row.appendChild(createEl('dd', '', pair[1]));
      facts.appendChild(row);
    });

    const meta = createEl('p', 'model-card__file');
    meta.appendChild(createEl('span', '', '别名'));
    meta.appendChild(createEl('code', '', entry.alias || entry.id));
    meta.appendChild(createEl('span', '', '· 许可 '));
    meta.appendChild(createEl('span', '', entry.license || '未知'));

    const actions = createEl('div', 'model-card__actions');
    const detail = createEl('button', 'btn btn--ghost btn--sm', '详情');
    detail.type = 'button';
    detail.dataset.action = 'detail';
    detail.dataset.id = entry.id;
    actions.appendChild(detail);
    const load = createEl('button', 'btn btn--sm ' + (entry.loaded ? 'btn--ghost' : 'btn--primary'), entry.loaded ? '卸载' : '加载');
    load.type = 'button';
    load.dataset.action = entry.loaded ? 'unload' : 'load';
    load.dataset.id = entry.id;
    actions.appendChild(load);
    const remove = createEl('button', 'btn btn--danger btn--sm', '删除');
    remove.type = 'button';
    remove.dataset.action = 'delete';
    remove.dataset.id = entry.id;
    actions.appendChild(remove);

    const tagsRow = buildModelTags(entry);

    card.appendChild(head);
    if (tagsRow) card.appendChild(tagsRow);
    card.appendChild(facts);
    card.appendChild(meta);
    card.appendChild(actions);
    return card;
  }

  function renderLibrary() {
    if (!dom.modelGrid) return;
    dom.modelGrid.textContent = '';
    const models = visibleLibrary();
    models.forEach(function (entry) {
      dom.modelGrid.appendChild(buildLibraryCard(entry));
    });
    buildModelFilters();
    const total = state.libraryModels.length;
    if (dom.modelEmpty) {
      dom.modelEmpty.hidden = models.length > 0 || Boolean(state.libraryError);
      if (state.libraryError) {
        dom.modelEmpty.textContent = '无法读取模型库：' + state.libraryError;
      } else {
        dom.modelEmpty.textContent =
          '还没有安装模型。去「发现」下载，或点「扫描目录」导入已有 GGUF。';
      }
    }
    if (dom.modelCount) {
      dom.modelCount.textContent =
        models.length === total
          ? '共 ' + total + ' 个模型' + (state.libraryLoaded ? ' · 已加载 ' + state.libraryLoaded : '')
          : '筛选后显示 ' + models.length + ' / ' + total + ' 个模型';
    }
  }

  async function refreshLibrary() {
    try {
      const response = await fetch(apiUrl('/admin/models'), {
        method: 'GET',
        headers: authHeaders(),
        cache: 'no-store',
      });
      if (!response.ok) throw new Error(await readErrorDetail(response));
      const body = await response.json();
      state.libraryModels = Array.isArray(body.data) ? body.data : [];
      state.libraryLoaded = body.loaded || null;
      state.libraryGeneration = body.generation || 0;
      state.libraryError = null;
    } catch (error) {
      state.libraryModels = [];
      state.libraryLoaded = null;
      state.libraryError = describeFetchError(error);
    }
    renderLibrary();
    if (state.selectedModel) {
      await openModelDetail(state.selectedModel, { silent: true });
    }
  }

  async function scanLibrary() {
    try {
      const response = await fetch(apiUrl('/admin/models/scan'), {
        method: 'POST',
        headers: authHeaders(),
        cache: 'no-store',
      });
      if (!response.ok) throw new Error(await readErrorDetail(response));
      const body = await response.json();
      const report = body.report || {};
      toast(
        '扫描完成：新增 ' + (report.added || 0) + ' / 更新 ' + (report.updated || 0),
        'ok'
      );
      await refreshLibrary();
    } catch (error) {
      toast('扫描失败：' + describeFetchError(error), 'error');
    }
  }

  async function openModelDetail(id, options) {
    const silent = Boolean(options && options.silent);
    state.selectedModel = id;
    if (!dom.modelDetail) return;
    try {
      const response = await fetch(apiUrl('/admin/models/' + encodeURIComponent(id)), {
        headers: authHeaders(),
        cache: 'no-store',
      });
      if (!response.ok) throw new Error(await readErrorDetail(response));
      const entry = await response.json();
      renderModelDetail(entry);
    } catch (error) {
      dom.modelDetail.hidden = false;
      dom.modelDetail.textContent = '';
      dom.modelDetail.appendChild(
        createEl('p', 'hint', '读取模型详情失败：' + describeFetchError(error))
      );
      if (!silent) toast('读取详情失败', 'error');
    }
  }

  function renderModelDetail(entry) {
    if (!dom.modelDetail) return;
    dom.modelDetail.hidden = false;
    dom.modelDetail.textContent = '';

    const head = createEl('div', 'panel__head');
    const title = createEl('h2', '', (entry.display_name || entry.id) + ' · 详情');
    const close = createEl('button', 'btn btn--ghost btn--sm', '关闭');
    close.type = 'button';
    close.dataset.detailAction = 'close';
    head.appendChild(title);
    head.appendChild(close);

    const facts = createEl('dl', 'model-card__facts');
    [
      ['ID', entry.id],
      ['仓库', entry.repo || '本地导入'],
      ['量化', entry.quant || '—'],
      ['架构', entry.architecture || '—'],
      ['体积', formatBytes(entry.size_bytes)],
      ['许可', entry.license || '未知'],
      ['路径', entry.path || '—'],
      ['校验', entry.verified ? '已记录哈希' : '未校验'],
      ['派生自', entry.derived_from || '—'],
      ['adapter', entry.adapter_id || '—'],
    ].forEach(function (pair) {
      const row = createEl('div');
      row.appendChild(createEl('dt', '', pair[0]));
      row.appendChild(createEl('dd', '', pair[1]));
      facts.appendChild(row);
    });

    const defaults = entry.load_defaults || {};
    const form = createEl('div', 'detail-form');
    const fields = [
      ['alias', '别名', 'text', entry.alias || ''],
      ['profile', '性能档', 'profile', defaults.profile || 'custom'],
      ['ctx', 'ctx', 'number', defaults.ctx || 32768],
      ['kv', 'KV', 'select', defaults.kv || 'q8_0'],
      ['slots', 'slots', 'number', defaults.slots || 1],
      ['ttl', 'TTL 秒', 'number', defaults.ttl_secs || 0],
    ];
    fields.forEach(function (field) {
      const label = createEl('label', 'field');
      label.appendChild(createEl('span', '', field[1]));
      let input;
      if (field[2] === 'select' || field[2] === 'profile') {
        input = createEl('select');
        const options = field[2] === 'profile'
          ? [['custom', 'custom（用下面的显式值）'], ['balanced', 'balanced'], ['turbo', 'turbo（8k / q4_0 / FA）']]
          : [['f16', 'f16'], ['q8_0', 'q8_0'], ['q4_0', 'q4_0']];
        options.forEach(function (pair) {
          const option = createEl('option', '', pair[1]);
          option.value = pair[0];
          if (pair[0] === field[3]) option.selected = true;
          input.appendChild(option);
        });
      } else {
        input = createEl('input');
        input.type = field[2];
        input.value = field[3];
      }
      input.dataset.detailField = field[0];
      label.appendChild(input);
      form.appendChild(label);
    });

    const actions = createEl('div', 'detail-actions');
    [
      ['save', '保存别名 / 默认值', 'btn--primary'],
      ['load', entry.loaded ? '重新加载' : '加载模型', 'btn--primary'],
      ['unload', '卸载', 'btn--ghost'],
      ['verify', '校验哈希', 'btn--ghost'],
      ['delete', '删除模型', 'btn--danger'],
    ].forEach(function (action) {
      const button = createEl('button', 'btn btn--sm ' + action[2], action[1]);
      button.type = 'button';
      button.dataset.detailAction = action[0];
      if (action[0] === 'unload' && !entry.loaded) button.disabled = true;
      actions.appendChild(button);
    });

    const fitTable = createEl('table', 'fit-table');
    const thead = createEl('thead');
    const headRow = createEl('tr');
    ['ctx', 'KV', '合计', '24GB', '32GB'].forEach(function (label) {
      headRow.appendChild(createEl('th', '', label));
    });
    thead.appendChild(headRow);
    fitTable.appendChild(thead);
    const tbody = createEl('tbody');
    (entry.fit_matrix || []).forEach(function (point) {
      const row = createEl('tr');
      row.appendChild(createEl('td', '', point.ctx));
      row.appendChild(createEl('td', '', point.kv));
      row.appendChild(createEl('td', '', formatBytes(point.total_bytes)));
      row.appendChild(createEl('td', '', point.fits_24gb ? '可跑' : '—'));
      row.appendChild(createEl('td', '', point.fits_32gb ? '可跑' : '—'));
      tbody.appendChild(row);
    });
    fitTable.appendChild(tbody);

    const presets = createEl('div', 'detail-presets');
    presets.appendChild(createEl('h3', 'section-title', '预设'));
    const presetList = createEl('ul', 'chip-list');
    (entry.presets || []).forEach(function (preset) {
      const item = createEl('li', 'chip', preset.name);
      const remove = createEl('button', 'btn btn--ghost btn--sm', '删除');
      remove.type = 'button';
      remove.dataset.presetDelete = preset.name;
      item.appendChild(remove);
      presetList.appendChild(item);
    });
    if (!(entry.presets || []).length) {
      presetList.appendChild(createEl('li', 'chip chip--muted', '暂无预设'));
    }
    const presetForm = createEl('div', 'detail-form');
    const presetName = createEl('input');
    presetName.type = 'text';
    presetName.placeholder = '预设名称';
    presetName.dataset.presetName = 'true';
    const presetPrompt = createEl('textarea');
    presetPrompt.placeholder = 'system prompt（可选）';
    presetPrompt.rows = 2;
    presetPrompt.dataset.presetPrompt = 'true';
    const presetSave = createEl('button', 'btn btn--sm', '保存预设');
    presetSave.type = 'button';
    presetSave.dataset.detailAction = 'preset-save';
    presetForm.appendChild(presetName);
    presetForm.appendChild(presetPrompt);
    presetForm.appendChild(presetSave);
    presets.appendChild(presetList);
    presets.appendChild(presetForm);

    dom.modelDetail.appendChild(head);
    dom.modelDetail.appendChild(facts);
    dom.modelDetail.appendChild(createEl('h3', 'section-title', '加载默认值'));
    dom.modelDetail.appendChild(form);
    dom.modelDetail.appendChild(actions);
    dom.modelDetail.appendChild(createEl('h3', 'section-title', '显存估算矩阵'));
    dom.modelDetail.appendChild(fitTable);
    dom.modelDetail.appendChild(presets);
  }

  function detailField(name) {
    if (!dom.modelDetail) return null;
    return dom.modelDetail.querySelector('[data-detail-field="' + name + '"]');
  }

  async function modelAction(action, id) {
    const modelId = id || state.selectedModel;
    if (!modelId) return;
    let path;
    let method = 'POST';
    let body = null;
    if (action === 'load') {
      path = '/admin/models/' + encodeURIComponent(modelId) + '/load';
      body = {
        profile: detailField('profile') ? detailField('profile').value : undefined,
        ctx: Number(detailField('ctx') && detailField('ctx').value) || undefined,
        kv: detailField('kv') ? detailField('kv').value : undefined,
        slots: Number(detailField('slots') && detailField('slots').value) || undefined,
        ttl_secs: Number(detailField('ttl') && detailField('ttl').value) || 0,
      };
    } else if (action === 'unload') {
      path = '/admin/models/' + encodeURIComponent(modelId) + '/unload';
    } else if (action === 'verify') {
      path = '/admin/models/' + encodeURIComponent(modelId) + '/verify';
    } else if (action === 'delete') {
      if (!window.confirm('删除模型 ' + modelId + '？托管文件与缓存将一并清理。')) return;
      path = '/admin/models/' + encodeURIComponent(modelId) + '?purge=true';
      method = 'DELETE';
    } else {
      return;
    }
    try {
      const response = await fetch(apiUrl(path), {
        method: method,
        headers: authHeaders(body ? { 'content-type': 'application/json' } : undefined),
        body: body ? JSON.stringify(body) : undefined,
      });
      if (!response.ok) throw new Error(await readErrorDetail(response));
      toast(action === 'load' ? '模型已加载' : action === 'unload' ? '模型已卸载' : '操作完成', 'ok');
      await refreshLibrary();
      await refreshHealth({ silent: true });
      await refreshServerModels();
      if (action === 'delete') {
        state.selectedModel = null;
        if (dom.modelDetail) dom.modelDetail.hidden = true;
      }
    } catch (error) {
      toast('操作失败：' + describeFetchError(error), 'error');
    }
  }

  async function saveModelDetail() {
    const id = state.selectedModel;
    if (!id) return;
    const aliasValue = detailField('alias') ? detailField('alias').value.trim() : '';
    const defaults = {
      profile: detailField('profile') ? detailField('profile').value : 'custom',
      ctx: Number(detailField('ctx') && detailField('ctx').value) || 32768,
      kv: detailField('kv') ? detailField('kv').value : 'q8_0',
      slots: Number(detailField('slots') && detailField('slots').value) || 1,
      ttl_secs: Number(detailField('ttl') && detailField('ttl').value) || 0,
    };
    try {
      let response = await fetch(apiUrl('/admin/models/' + encodeURIComponent(id) + '/alias'), {
        method: 'PUT',
        headers: authHeaders({ 'content-type': 'application/json' }),
        body: JSON.stringify({ alias: aliasValue || null }),
      });
      if (!response.ok) throw new Error(await readErrorDetail(response));
      response = await fetch(apiUrl('/admin/models/' + encodeURIComponent(id) + '/defaults'), {
        method: 'PUT',
        headers: authHeaders({ 'content-type': 'application/json' }),
        body: JSON.stringify(defaults),
      });
      if (!response.ok) throw new Error(await readErrorDetail(response));
      toast('默认值已保存', 'ok');
      await refreshLibrary();
    } catch (error) {
      toast('保存失败：' + describeFetchError(error), 'error');
    }
  }

  async function savePreset() {
    const id = state.selectedModel;
    if (!id || !dom.modelDetail) return;
    const name = dom.modelDetail.querySelector('[data-preset-name]');
    const prompt = dom.modelDetail.querySelector('[data-preset-prompt]');
    if (!name || !name.value.trim()) {
      toast('请填写预设名称', 'warn');
      return;
    }
    try {
      const response = await fetch(apiUrl('/admin/models/' + encodeURIComponent(id) + '/preset'), {
        method: 'PUT',
        headers: authHeaders({ 'content-type': 'application/json' }),
        body: JSON.stringify({
          name: name.value.trim(),
          system_prompt: prompt ? prompt.value : '',
          temperature: 0.7,
          top_p: 0.9,
          max_tokens: 1024,
        }),
      });
      if (!response.ok) throw new Error(await readErrorDetail(response));
      toast('预设已保存', 'ok');
      await openModelDetail(id);
    } catch (error) {
      toast('保存预设失败：' + describeFetchError(error), 'error');
    }
  }

  async function deletePreset(name) {
    const id = state.selectedModel;
    if (!id || !name) return;
    try {
      const response = await fetch(
        apiUrl('/admin/models/' + encodeURIComponent(id) + '/preset/' + encodeURIComponent(name)),
        { method: 'DELETE', headers: authHeaders() }
      );
      if (!response.ok) throw new Error(await readErrorDetail(response));
      toast('预设已删除', 'ok');
      await openModelDetail(id);
    } catch (error) {
      toast('删除预设失败：' + describeFetchError(error), 'error');
    }
  }

  /* ---------- discover ---------- */

  function tagObject(id, source) {
    return {
      id: id,
      label: TAG_LABELS[id] || id,
      source: source || 'curated',
      confidence: source === 'name' ? 0.7 : 1,
      suggested: source === 'metadata' || source === 'file' || source === 'name',
      runtime_supported: TAG_RUNTIME_UNSUPPORTED.indexOf(id) === -1,
    };
  }

  function seedFromStatic() {
    return DEFAULT_MODELS.map(function (model) {
      return {
        id: model.id,
        repo: model.repo,
        file: model.file,
        quant: model.quant,
        license: model.license,
        size_bytes: model.sizeBytes,
        size_human: model.size,
        role: model.role,
        tags: (model.tags || []).map(function (id) {
          return tagObject(id, 'curated');
        }),
        downloaded: false,
      };
    });
  }

  function matchesCatalogTags(item) {
    return state.catalogTags.every(function (id) {
      return (item.tags || []).some(function (tag) {
        return tag.id === id;
      });
    });
  }

  function renderCatalogStatus(meta) {
    if (!dom.catalogStatus) return;
    dom.catalogStatus.textContent = '';
    const source = meta && meta.source
      ? CATALOG_SOURCE_LABELS[meta.source] || meta.source
      : '状态未知（内置镜像）';
    const parts = ['目录：' + source];
    if (meta) {
      if (meta.updated_at) parts.push('发布 ' + String(meta.updated_at).slice(0, 10));
      if (meta.fetched_at) {
        parts.push('获取 ' + String(meta.fetched_at).replace('T', ' ').slice(0, 16));
      }
      if (meta.refreshing) parts.push('刷新中…');
      else if (meta.stale) parts.push('已过期，将后台更新');
      if (meta.skipped_reason === 'disabled') parts.push('自动更新已关闭');
    }
    dom.catalogStatus.appendChild(createEl('span', 'catalog-status__text', parts.join(' · ')));
    if (meta && meta.error) {
      dom.catalogStatus.appendChild(
        createEl('span', 'chip chip--warn', '刷新失败：' + meta.error)
      );
    }
  }

  function renderCatalogTagFilters() {
    if (!dom.catalogTagFilters) return;
    dom.catalogTagFilters.textContent = '';
    TAG_ORDER.forEach(function (id) {
      const active = state.catalogTags.indexOf(id) !== -1;
      const button = createEl(
        'button',
        'chip chip--filter' + (active ? ' is-active' : ''),
        TAG_LABELS[id]
      );
      button.type = 'button';
      button.dataset.tagFilter = id;
      button.setAttribute('aria-pressed', active ? 'true' : 'false');
      dom.catalogTagFilters.appendChild(button);
    });
  }

  function catalogTagChip(tag) {
    const unsupported = tag.runtime_supported === false;
    const text = unsupported ? tag.label + ' · 运行暂不支持' : tag.label;
    const chip = createEl('span', 'chip chip--tag', text);
    if (unsupported) chip.classList.add('chip--warn');
    if (tag.suggested) {
      chip.classList.add('chip--suggested');
      const source = TAG_SOURCE_LABELS[tag.source] || tag.source;
      const confidence = Math.round((Number(tag.confidence) || 0) * 100);
      chip.title = '建议标签 · 来源：' + source + ' · 置信度 ' + confidence + '%';
    } else {
      chip.classList.add('chip--muted');
      chip.title = '标签来源：' + (TAG_SOURCE_LABELS[tag.source] || tag.source);
    }
    return chip;
  }

  function renderCatalog(items) {
    if (!dom.catalogGrid) return;
    dom.catalogGrid.textContent = '';
    const visible = items.filter(matchesCatalogTags);
    if (!visible.length) {
      dom.catalogGrid.appendChild(
        createEl('p', 'hint', '没有符合当前标签筛选的模型。')
      );
      return;
    }
    visible.forEach(function (item) {
      const card = createEl('article', 'model-card');
      const head = createEl('div', 'model-card__head');
      const wrap = createEl('div', 'model-card__title-wrap');
      wrap.appendChild(createEl('h3', 'model-card__title', item.id || item.repo));
      wrap.appendChild(createEl('p', 'model-card__repo', item.repo));
      const chips = createEl('div', 'model-card__chips');
      chips.appendChild(createEl('span', 'chip', item.quant || '—'));
      if (item.role) {
        chips.appendChild(
          createEl('span', 'chip chip--muted', CATALOG_ROLE_LABELS[item.role] || item.role)
        );
      }
      (item.tags || []).forEach(function (tag) {
        chips.appendChild(catalogTagChip(tag));
      });
      head.appendChild(wrap);
      head.appendChild(chips);

      const facts = createEl('dl', 'model-card__facts');
      [
        ['体积', item.size_human || formatBytes(item.size_bytes)],
        ['许可', item.license || '未知'],
        ['文件', item.file || '整仓库'],
      ].forEach(function (pair) {
        const row = createEl('div');
        row.appendChild(createEl('dt', '', pair[0]));
        row.appendChild(createEl('dd', '', pair[1]));
        facts.appendChild(row);
      });

      const actions = createEl('div', 'model-card__actions');
      const download = createEl('button', 'btn btn--primary btn--sm', item.downloaded ? '重新下载' : '下载');
      download.type = 'button';
      download.dataset.catalogDownload = item.repo;
      download.dataset.catalogFile = item.file || '';
      download.dataset.catalogSource = 'auto';
      actions.appendChild(download);
      const copy = createEl('button', 'btn btn--ghost btn--sm', '复制命令');
      copy.type = 'button';
      copy.dataset.copy = copyCommandFor(item);
      actions.appendChild(copy);
      if (item.downloaded) actions.appendChild(createEl('span', 'chip chip--ok', '已安装'));

      card.appendChild(head);
      card.appendChild(facts);
      card.appendChild(actions);
      dom.catalogGrid.appendChild(card);
    });
  }

  async function refreshCatalog(options) {
    const quiet = options && options.quiet;
    try {
      const response = await fetch(apiUrl('/admin/catalog'), {
        headers: authHeaders(),
        cache: 'no-store',
      });
      if (!response.ok) throw new Error(await readErrorDetail(response));
      const body = await response.json();
      state.catalog = Array.isArray(body.data) ? body.data : [];
      state.catalogMeta = body.catalog || null;
      renderCatalogStatus(state.catalogMeta);
      renderCatalogTagFilters();
      const searchActive = dom.catalogSearch && dom.catalogSearch.value.trim();
      if (!(quiet && searchActive)) renderCatalog(state.catalog);
    } catch (error) {
      state.catalog = seedFromStatic();
      state.catalogMeta = null;
      renderCatalogStatus(null);
      renderCatalogTagFilters();
      renderCatalog(state.catalog);
      if (!quiet) {
        toast('推荐目录读取失败，使用内置清单：' + describeFetchError(error), 'warn');
      }
    }
  }

  async function refreshRemoteCatalog() {
    if (dom.catalogRefresh) dom.catalogRefresh.disabled = true;
    try {
      const headers = Object.assign({ 'content-type': 'application/json' }, authHeaders());
      const response = await fetch(apiUrl('/admin/catalog/refresh'), {
        method: 'POST',
        headers: headers,
        body: JSON.stringify({ force: true }),
      });
      if (!response.ok) throw new Error(await readErrorDetail(response));
      const body = await response.json();
      state.catalogMeta = body.catalog || null;
      renderCatalogStatus(state.catalogMeta);
      if (state.catalogMeta && state.catalogMeta.error) {
        toast('目录刷新失败，继续使用本地版本：' + state.catalogMeta.error, 'warn');
      } else if (state.catalogMeta && state.catalogMeta.skipped_reason) {
        toast('目录刷新已跳过：' + state.catalogMeta.skipped_reason, 'warn');
      } else {
        toast('推荐目录已更新', 'ok');
      }
    } catch (error) {
      toast('刷新目录失败：' + describeFetchError(error), 'error');
    } finally {
      if (dom.catalogRefresh) dom.catalogRefresh.disabled = false;
      void refreshCatalog({ quiet: true });
    }
  }

  async function searchCatalog() {
    const query = dom.catalogSearch ? dom.catalogSearch.value.trim() : '';
    const params = 'q=' + encodeURIComponent(query);
    const tags = state.catalogTags.length ? '&tags=' + encodeURIComponent(state.catalogTags.join(',')) : '';
    try {
      const response = await fetch(apiUrl('/admin/catalog/search?' + params + tags), {
        headers: authHeaders(),
        cache: 'no-store',
      });
      if (!response.ok) throw new Error(await readErrorDetail(response));
      const body = await response.json();
      if (body.catalog) state.catalogMeta = body.catalog;
      renderCatalog(Array.isArray(body.data) ? body.data : []);
    } catch (error) {
      const needle = query.toLowerCase();
      renderCatalog(
        state.catalog.filter(function (item) {
          return !needle || JSON.stringify(item).toLowerCase().indexOf(needle) !== -1;
        })
      );
    }
  }

  function renderRepoResult(result) {
    if (!dom.repoResult) return;
    dom.repoResult.hidden = false;
    dom.repoResult.textContent = '';
    const head = createEl('div', 'panel__head');
    head.appendChild(createEl('h3', 'section-title', result.repo + ' · ' + (result.license || '许可未知')));
    dom.repoResult.appendChild(head);
    if (!result.quants || !result.quants.length) {
      dom.repoResult.appendChild(createEl('p', 'hint', '该仓库没有可下载的 GGUF 文件。'));
      return;
    }
    const table = createEl('table', 'fit-table');
    const tbody = createEl('tbody');
    result.quants.forEach(function (quant) {
      const row = createEl('tr');
      row.appendChild(createEl('td', '', quant.quant));
      row.appendChild(createEl('td', '', quant.total_human || formatBytes(quant.total_size)));
      row.appendChild(createEl('td', '', String((quant.files || []).length) + ' 个文件'));
      const actionCell = createEl('td');
      const button = createEl('button', 'btn btn--primary btn--sm', '下载');
      button.type = 'button';
      button.dataset.quantDownload = result.repo;
      button.dataset.quantFiles = JSON.stringify((quant.files || []).map(function (file) { return file.path; }));
      button.dataset.quantSource = dom.repoSource ? dom.repoSource.value : 'auto';
      actionCell.appendChild(button);
      row.appendChild(actionCell);
      tbody.appendChild(row);
    });
    table.appendChild(tbody);
    dom.repoResult.appendChild(table);
    dom.repoResult.appendChild(createEl('p', 'hint', '下载前请确认许可；显存适配在入库后按 GGUF metadata 计算。'));
  }

  async function resolveRepo() {
    const repo = dom.repoInput ? dom.repoInput.value.trim() : '';
    if (!repo) {
      toast('请填写仓库 ID 或 URL', 'warn');
      return;
    }
    if (dom.repoResult) {
      dom.repoResult.hidden = false;
      dom.repoResult.textContent = '正在解析…';
    }
    try {
      const source = dom.repoSource ? dom.repoSource.value : 'auto';
      const response = await fetch(
        apiUrl('/admin/catalog/resolve?repo=' + encodeURIComponent(repo) + '&source=' + encodeURIComponent(source)),
        { headers: authHeaders(), cache: 'no-store' }
      );
      if (!response.ok) throw new Error(await readErrorDetail(response));
      const body = await response.json();
      state.repoResult = body;
      renderRepoResult(body);
    } catch (error) {
      if (dom.repoResult) {
        dom.repoResult.hidden = false;
        dom.repoResult.textContent = '解析失败：' + describeFetchError(error);
      }
      toast('解析仓库失败', 'error');
    }
  }

  async function startDownload(payload) {
    try {
      const response = await fetch(apiUrl('/admin/downloads'), {
        method: 'POST',
        headers: authHeaders({ 'content-type': 'application/json' }),
        body: JSON.stringify(payload),
      });
      if (!response.ok) throw new Error(await readErrorDetail(response));
      toast('下载任务已创建', 'ok');
      showModelTab('downloads');
      await refreshDownloads();
    } catch (error) {
      toast('下载失败：' + describeFetchError(error), 'error');
    }
  }

  /* ---------- downloads ---------- */

  const JOB_LABELS = {
    queued: '排队中',
    resolving: '解析中',
    downloading: '下载中',
    verifying: '校验中',
    installed: '已安装',
    failed: '失败',
    paused: '已暂停',
    cancelled: '已取消',
  };

  function renderDownloads() {
    if (!dom.downloadList) return;
    dom.downloadList.textContent = '';
    if (!state.downloads.length) {
      if (dom.downloadsStatus) dom.downloadsStatus.textContent = '暂无下载任务。';
      return;
    }
    if (dom.downloadsStatus) {
      dom.downloadsStatus.textContent = '共 ' + state.downloads.length + ' 个任务（每 2 秒刷新）。';
    }
    state.downloads.forEach(function (job) {
      const item = createEl('li', 'download-item');
      const head = createEl('div', 'download-item__head');
      head.appendChild(createEl('strong', '', job.repo + ' @ ' + job.revision));
      head.appendChild(createEl('span', 'chip', JOB_LABELS[job.state] || job.state));
      const ratio = job.total ? Math.min(1, job.downloaded / job.total) : 0;
      const detail = createEl(
        'p',
        'hint',
        formatBytes(job.downloaded) +
          ' / ' +
          formatBytes(job.total) +
          ' · ' +
          Math.round(ratio * 100) +
          '%' +
          (job.bytes_per_sec ? ' · ' + formatBytes(job.bytes_per_sec) + '/s' : '')
      );
      const bar = createEl('div', 'progress');
      const fill = createEl('div', 'progress__bar');
      fill.style.width = Math.round(ratio * 100) + '%';
      bar.appendChild(fill);
      const actions = createEl('div', 'download-item__actions');
      if (job.state === 'downloading' || job.state === 'resolving' || job.state === 'queued') {
        actions.appendChild(downloadButton(job.id, 'pause', '暂停'));
        actions.appendChild(downloadButton(job.id, 'cancel', '取消'));
      }
      if (job.state === 'paused' || job.state === 'failed' || job.state === 'cancelled') {
        actions.appendChild(downloadButton(job.id, 'resume', '继续'));
      }
      if (job.state === 'installed' && job.model_id) {
        const open = createEl('button', 'btn btn--ghost btn--sm', '查看模型');
        open.type = 'button';
        open.dataset.openModel = job.model_id;
        actions.appendChild(open);
      }
      if (job.error) detail.textContent += ' · ' + job.error;
      item.appendChild(head);
      item.appendChild(detail);
      item.appendChild(bar);
      item.appendChild(actions);
      dom.downloadList.appendChild(item);
    });
  }

  function downloadButton(id, action, label) {
    const button = createEl('button', 'btn btn--ghost btn--sm', label);
    button.type = 'button';
    button.dataset.downloadAction = action;
    button.dataset.downloadId = id;
    return button;
  }

  async function refreshDownloads() {
    try {
      const response = await fetch(apiUrl('/admin/downloads'), {
        headers: authHeaders(),
        cache: 'no-store',
      });
      if (!response.ok) throw new Error(await readErrorDetail(response));
      const body = await response.json();
      state.downloads = Array.isArray(body.data) ? body.data : [];
      renderDownloads();
    } catch (error) {
      if (dom.downloadsStatus) dom.downloadsStatus.textContent = '读取下载任务失败：' + describeFetchError(error);
    }
    await refreshStorage();
  }

  async function refreshStorage() {
    if (!dom.storageInfo) return;
    try {
      const response = await fetch(apiUrl('/admin/storage'), {
        headers: authHeaders(),
        cache: 'no-store',
      });
      if (!response.ok) throw new Error(await readErrorDetail(response));
      const body = await response.json();
      state.storage = body;
      dom.storageInfo.textContent =
        '模型 ' +
        (body.models_human || formatBytes(body.models_bytes)) +
        ' · 缓存 ' +
        (body.cache_human || formatBytes(body.cache_bytes)) +
        ' · 可回收 ' +
        (body.reclaimable_human || formatBytes(body.reclaimable_bytes)) +
        ' · 缓存目录 ' +
        (body.cache_root || '—');
    } catch (error) {
      dom.storageInfo.textContent = '读取磁盘信息失败：' + describeFetchError(error);
    }
  }

  async function downloadAction(id, action) {
    try {
      const response = await fetch(
        apiUrl('/admin/downloads/' + encodeURIComponent(id) + '/' + action),
        { method: 'POST', headers: authHeaders() }
      );
      if (!response.ok) throw new Error(await readErrorDetail(response));
      await refreshDownloads();
    } catch (error) {
      toast('下载操作失败：' + describeFetchError(error), 'error');
    }
  }

  async function gcCache() {
    try {
      const response = await fetch(apiUrl('/admin/cache/gc'), {
        method: 'POST',
        headers: authHeaders({ 'content-type': 'application/json' }),
        body: JSON.stringify({ dry_run: false, grace_hours: 24 }),
      });
      if (!response.ok) throw new Error(await readErrorDetail(response));
      const body = await response.json();
      toast('已回收 ' + formatBytes(body.data && body.data.freed_bytes), 'ok');
      await refreshStorage();
    } catch (error) {
      toast('缓存清理失败：' + describeFetchError(error), 'error');
    }
  }

  function showModelTab(name) {
    if (['library', 'discover', 'downloads'].indexOf(name) === -1) name = 'library';
    state.modelTab = name;
    Array.prototype.forEach.call(document.querySelectorAll('.model-tab'), function (button) {
      const active = button.dataset.mtab === name;
      button.classList.toggle('is-active', active);
      button.setAttribute('aria-selected', active ? 'true' : 'false');
    });
    ['library', 'discover', 'downloads'].forEach(function (tab) {
      const panel = document.getElementById('model-tab-' + tab);
      if (panel) panel.hidden = tab !== name;
    });
    if (name === 'library') void refreshLibrary();
    if (name === 'discover' && !state.catalog.length) void refreshCatalog();
    if (name === 'downloads') void refreshDownloads();
    updateModelPolling();
  }

  function updateModelPolling() {
    if (state.modelTimer) {
      window.clearInterval(state.modelTimer);
      state.modelTimer = null;
    }
    if (state.view === 'models' && state.modelTab === 'downloads') {
      state.modelTimer = window.setInterval(function () {
        if (!document.hidden) void refreshDownloads();
      }, 2000);
    } else if (state.view === 'models' && state.modelTab === 'discover') {
      state.modelTimer = window.setInterval(function () {
        if (!document.hidden) void refreshCatalog({ quiet: true });
      }, 2000);
    }
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
    state.catalogTags = (readStorage(STORAGE_KEYS.catalogTags) || '')
      .split(',')
      .map(function (id) { return id.trim(); })
      .filter(function (id) { return Boolean(TAG_LABELS[id]); });

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

    if (target === 'models') {
      void refreshServerModels();
      showModelTab(state.modelTab);
    } else {
      updateModelPolling();
    }
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
      modelSearch: document.getElementById('model-search'),
      modelFilters: document.getElementById('model-filters'),
      modelSort: document.getElementById('model-sort'),
      modelCount: document.getElementById('model-count'),
      modelEmpty: document.getElementById('model-empty'),
      modelDetail: document.getElementById('model-detail'),
      libraryScan: document.getElementById('library-scan'),
      catalogGrid: document.getElementById('catalog-grid'),
      catalogStatus: document.getElementById('catalog-status'),
      catalogTagFilters: document.getElementById('catalog-tag-filters'),
      catalogSearch: document.getElementById('catalog-search'),
      catalogRefresh: document.getElementById('catalog-refresh'),
      repoInput: document.getElementById('repo-input'),
      repoSource: document.getElementById('repo-source'),
      repoResolve: document.getElementById('repo-resolve'),
      repoResult: document.getElementById('repo-result'),
      downloadList: document.getElementById('download-list'),
      downloadsStatus: document.getElementById('downloads-status'),
      downloadsRefresh: document.getElementById('downloads-refresh'),
      storageInfo: document.getElementById('storage-info'),
      cacheGc: document.getElementById('cache-gc'),
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

    if (dom.modelSearch) {
      dom.modelSearch.addEventListener('input', function () {
        state.modelQuery = dom.modelSearch.value || '';
        renderLibrary();
      });
    }

    if (dom.modelFilters) {
      dom.modelFilters.addEventListener('click', function (event) {
        const button =
          event.target instanceof Element ? event.target.closest('[data-filter]') : null;
        if (!button || !button.dataset.filter) return;
        state.modelFilter = button.dataset.filter;
        renderLibrary();
      });
    }

    if (dom.modelSort) {
      dom.modelSort.addEventListener('change', function () {
        state.modelSort = dom.modelSort.value;
        renderLibrary();
      });
    }

    if (dom.libraryScan) dom.libraryScan.addEventListener('click', function () { void scanLibrary(); });
    if (dom.catalogRefresh) {
      dom.catalogRefresh.addEventListener('click', function () {
        void refreshRemoteCatalog();
      });
    }
    if (dom.catalogTagFilters) {
      dom.catalogTagFilters.addEventListener('click', function (event) {
        const target = event.target instanceof Element ? event.target.closest('[data-tag-filter]') : null;
        if (!target || !target.dataset.tagFilter) return;
        const id = target.dataset.tagFilter;
        const index = state.catalogTags.indexOf(id);
        if (index === -1) state.catalogTags.push(id);
        else state.catalogTags.splice(index, 1);
        writeStorage(STORAGE_KEYS.catalogTags, state.catalogTags.join(','));
        renderCatalogTagFilters();
        if (dom.catalogSearch && dom.catalogSearch.value.trim()) {
          void searchCatalog();
        } else {
          renderCatalog(state.catalog);
        }
      });
    }
    if (dom.catalogSearch) {
      dom.catalogSearch.addEventListener('input', function () {
        void searchCatalog();
      });
    }
    if (dom.repoResolve) dom.repoResolve.addEventListener('click', function () { void resolveRepo(); });
    if (dom.repoInput) {
      dom.repoInput.addEventListener('keydown', function (event) {
        if (event.key === 'Enter') {
          event.preventDefault();
          void resolveRepo();
        }
      });
    }
    if (dom.downloadsRefresh) {
      dom.downloadsRefresh.addEventListener('click', function () { void refreshDownloads(); });
    }
    if (dom.cacheGc) dom.cacheGc.addEventListener('click', function () { void gcCache(); });

    document.addEventListener('click', function (event) {
      const tab = event.target instanceof Element ? event.target.closest('[data-mtab]') : null;
      if (tab && tab.dataset.mtab) {
        showModelTab(tab.dataset.mtab);
        return;
      }
      const cardAction =
        event.target instanceof Element ? event.target.closest('[data-action][data-id]') : null;
      if (cardAction) {
        const action = cardAction.dataset.action;
        const id = cardAction.dataset.id;
        if (action === 'detail') void openModelDetail(id);
        else if (action === 'confirm-tag' || action === 'hide-tag') {
          void updateModelTags(id, action, cardAction.dataset.tag);
        }
        else if (action === 'load' || action === 'unload' || action === 'delete') {
          if (action === 'delete' && !window.confirm('删除模型 ' + id + '？')) return;
          const path =
            '/admin/models/' +
            encodeURIComponent(id) +
            (action === 'delete' ? '?purge=true' : '/' + action);
          const method = action === 'delete' ? 'DELETE' : 'POST';
          const headers =
            action === 'load' ? authHeaders({ 'Content-Type': 'application/json' }) : authHeaders();
          const body = action === 'load' ? JSON.stringify({}) : undefined;
          fetch(apiUrl(path), { method: method, headers: headers, body: body })
            .then(function (response) {
              if (!response.ok) return readErrorDetail(response).then(function (text) { throw new Error(text); });
              toast(action === 'load' ? '模型已加载' : action === 'unload' ? '模型已卸载' : '模型已删除', 'ok');
              state.selectedModel = null;
              if (dom.modelDetail) dom.modelDetail.hidden = true;
              return refreshLibrary()
                .then(function () { return refreshHealth({ silent: true }); })
                .then(function () { return refreshServerModels(); });
            })
            .catch(function (error) { toast('操作失败：' + describeFetchError(error), 'error'); });
        }
        return;
      }
      const detailAction =
        event.target instanceof Element ? event.target.closest('[data-detail-action]') : null;
      if (detailAction) {
        const action = detailAction.dataset.detailAction;
        if (action === 'close') {
          state.selectedModel = null;
          if (dom.modelDetail) dom.modelDetail.hidden = true;
        } else if (action === 'save') void saveModelDetail();
        else if (action === 'preset-save') void savePreset();
        else if (action === 'load' || action === 'unload' || action === 'verify' || action === 'delete') {
          void modelAction(action);
        }
        return;
      }
      const presetDelete =
        event.target instanceof Element ? event.target.closest('[data-preset-delete]') : null;
      if (presetDelete) {
        void deletePreset(presetDelete.dataset.presetDelete);
        return;
      }
      const catalogDownload =
        event.target instanceof Element ? event.target.closest('[data-catalog-download]') : null;
      if (catalogDownload) {
        void startDownload({
          repo: catalogDownload.dataset.catalogDownload,
          file: catalogDownload.dataset.catalogFile || undefined,
          source: catalogDownload.dataset.catalogSource || 'auto',
        });
        return;
      }
      const quantDownload =
        event.target instanceof Element ? event.target.closest('[data-quant-download]') : null;
      if (quantDownload) {
        let files = [];
        try {
          files = JSON.parse(quantDownload.dataset.quantFiles || '[]');
        } catch (error) {
          files = [];
        }
        void startDownload({
          repo: quantDownload.dataset.quantDownload,
          files: files,
          source: quantDownload.dataset.quantSource || 'auto',
        });
        return;
      }
      const downloadActionButton =
        event.target instanceof Element ? event.target.closest('[data-download-action]') : null;
      if (downloadActionButton) {
        void downloadAction(
          downloadActionButton.dataset.downloadId,
          downloadActionButton.dataset.downloadAction
        );
        return;
      }
      const openModel =
        event.target instanceof Element ? event.target.closest('[data-open-model]') : null;
      if (openModel) {
        showModelTab('library');
        void openModelDetail(openModel.dataset.openModel);
      }
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
    buildModelFilters();
    renderLibrary();
    void refreshLibrary();
    void refreshCatalog();
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
