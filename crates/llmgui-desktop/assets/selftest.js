/*
 * llmgui desktop acceptance driver (injected by `llmgui-desktop --selftest`).
 *
 * Runs inside the real WebView that shows the shipped Web GUI, so every step
 * exercises the product exactly like a user: health check, view switch, typing,
 * send, SSE streaming, rendered stats. The result is posted back to a loopback
 * socket owned by the shell (sendBeacon keeps it a no-CORS simple request).
 */
(async function () {
  'use strict';

  var REPORT_URL = '__REPORT_URL__';
  var PROMPT = __PROMPT_JSON__;
  var TIMEOUT_MS = __TIMEOUT_MS__;
  var FINAL_VIEW = '__FINAL_VIEW__';
  var startedAt = Date.now();

  var result = {
    ok: false,
    checks: {},
    prompt: PROMPT,
    model: null,
    reply: null,
    stats: null,
    serverModels: null,
    health: null,
    errors: [],
    elapsedMs: 0,
  };

  function sleep(ms) {
    return new Promise(function (resolve) {
      setTimeout(resolve, ms);
    });
  }

  function q(selector) {
    return document.querySelector(selector);
  }

  async function waitFor(label, predicate, budgetMs) {
    var deadline = Date.now() + budgetMs;
    while (Date.now() < deadline) {
      var value = null;
      try {
        value = predicate();
      } catch (error) {
        value = null;
      }
      if (value) return value;
      await sleep(100);
    }
    throw new Error('timeout waiting for ' + label);
  }

  function report() {
    result.elapsedMs = Date.now() - startedAt;
    var payload = JSON.stringify(result);
    try {
      if (navigator.sendBeacon) {
        navigator.sendBeacon(REPORT_URL, payload);
        return;
      }
    } catch (error) {
      /* fall through to fetch */
    }
    try {
      void fetch(REPORT_URL, { method: 'POST', mode: 'no-cors', body: payload });
    } catch (error) {
      /* nothing else we can do */
    }
  }

  try {
    /* 1. the shell loaded the GUI from the runtime origin */
    await waitFor('app bootstrap', function () {
      return q('#status') && q('#chat-input') && q('#model-grid');
    }, Math.min(TIMEOUT_MS, 20000));

    await waitFor('health online', function () {
      return q('#status').classList.contains('is-online');
    }, Math.min(TIMEOUT_MS, 20000));
    result.checks.healthOnline = true;
    result.health = q('#status-meta').textContent || '';

    /* 2. chat view, real model id, real user input */
    q('.nav__item[data-view="chat"]').click();
    await waitFor('chat view', function () {
      return !q('#view-chat').hidden;
    }, 5000);

    result.model = (q('#chat-model').textContent || '').replace(/^模型[:：]\s*/, '');
    result.checks.modelVisible = result.model.length > 0;

    var input = q('#chat-input');
    input.value = PROMPT;
    input.dispatchEvent(new Event('input', { bubbles: true }));
    q('#chat-send').click();

    /* 3. wait for the streamed answer and the rendered stats line */
    var answer = await waitFor('assistant answer', function () {
      var notes = document.querySelectorAll('.msg--assistant .msg__note');
      if (!notes.length) return null;
      var note = notes[notes.length - 1];
      if (note.textContent === '生成中…') return null;
      var article = note.closest('.msg--assistant');
      var body = article ? article.querySelector('.msg__body') : null;
      if (!body || !body.textContent) return null;
      return { text: body.textContent, note: note.textContent, streaming: article.classList.contains('is-streaming') };
    }, TIMEOUT_MS);
    result.reply = answer.text;
    result.stats = answer.note;

    result.checks.gotReply = answer.text.length > 0;
    result.checks.notStreaming = answer.streaming === false;
    result.checks.ttftReported = answer.note.indexOf('TTFT') !== -1;
    result.checks.tokensPerSecond = answer.note.indexOf('tok/s') !== -1;
    result.checks.noErrorBubble = document.querySelectorAll('.msg--error').length === 0;

    /* 4. models page shows what the runtime actually loaded */
    q('.nav__item[data-view="models"]').click();
    await waitFor('models view', function () {
      return !q('#view-models').hidden;
    }, 5000);
    try {
      await waitFor('server model list', function () {
        return document.querySelectorAll('#models-server-list .chip').length > 0;
      }, 15000);
    } catch (error) {
      result.errors.push(String((error && error.message) || error));
    }
    var chips = Array.prototype.map.call(
      document.querySelectorAll('#models-server-list .chip'),
      function (chip) {
        return chip.textContent;
      }
    );
    result.serverModels = chips.join(' | ');
    result.checks.serverModelListed = chips.length > 0;

    /* 5. leave the requested page on screen (models proves what the runtime loaded) */
    if (FINAL_VIEW === 'chat') {
      q('.nav__item[data-view="chat"]').click();
      await waitFor('chat view again', function () {
        return !q('#view-chat').hidden;
      }, 5000);
    }

    result.ok =
      result.errors.length === 0 &&
      Object.keys(result.checks).every(function (key) {
        return result.checks[key] === true;
      });
  } catch (error) {
    result.errors.push(String((error && error.message) || error));
  }

  report();
})();
