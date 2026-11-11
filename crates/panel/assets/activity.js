// Verification page: inside a Discord Activity, or opened from a Verify link.
// Shows the server's captcha (a code to type, or a puzzle piece to slide).
(() => {
  'use strict';

  const { base, clientId } = document.body.dataset;
  const query = new URLSearchParams(location.search);
  const frameId = query.get('frame_id');
  const guildId = query.get('guild_id');
  const link = query.get('v');
  const inActivity = Boolean(clientId && frameId);
  const $ = (id) => document.getElementById(id);

  const STEPS = ['loading', 'challenge', 'done', 'failed'];
  const show = (step, message) => {
    for (const name of STEPS) $(name).hidden = name !== step;
    if (message) $(`${step}-text`).textContent = message;
  };

  if (!link && !(inActivity && guildId)) {
    show('failed', 'Open this page with the Verify button of a server.');
    $('retry').hidden = true;
    return;
  }

  // ---- Minimal client for the Discord client's RPC (window.postMessage),
  // the protocol of Discord's Embedded App SDK: a handshake, then commands.
  const OP_HANDSHAKE = 0;
  const OP_FRAME = 1;
  const OP_CLOSE = 2;
  const DISCORD_ORIGINS = new Set([
    'https://discord.com',
    'https://discordapp.com',
    'https://ptb.discord.com',
    'https://ptb.discordapp.com',
    'https://canary.discord.com',
    'https://canary.discordapp.com',
    'null', // the mobile apps
  ]);
  const client = window.parent.opener ?? window.parent;
  const clientOrigin = document.referrer || '*';
  const pending = new Map();
  let markReady;
  const ready = new Promise((resolve) => { markReady = resolve; });
  const nonce = () => (crypto.randomUUID ? crypto.randomUUID() : `${Date.now()}-${Math.random()}`);

  if (inActivity) {
    addEventListener('message', (event) => {
      if (!DISCORD_ORIGINS.has(event.origin) || !Array.isArray(event.data)) return;
      const [opcode, data] = event.data;
      if (opcode !== OP_FRAME || !data) return;
      if (data.cmd === 'DISPATCH') {
        if (data.evt === 'READY') markReady();
        return;
      }
      const waiter = pending.get(data.nonce);
      if (!waiter) return;
      pending.delete(data.nonce);
      if (data.evt === 'ERROR') waiter.reject(new Error(data.data?.message || 'Discord refused the request.'));
      else waiter.resolve(data.data);
    });
  }

  const command = (cmd, args) => new Promise((resolve, reject) => {
    const id = nonce();
    pending.set(id, { resolve, reject });
    client.postMessage([OP_FRAME, { cmd, args, nonce: id }], clientOrigin);
  });

  const within = (promise, ms, message) => Promise.race([
    promise,
    new Promise((_, reject) => { setTimeout(() => reject(new Error(message)), ms); }),
  ]);

  // ---- Our own endpoints (through Discord's proxy inside the Activity).
  const post = async (path, body) => {
    try {
      const response = await fetch(base + path, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
      });
      return await response.json();
    } catch {
      return { ok: false, error: 'Something went wrong. Try again in a moment.' };
    }
  };

  // ---- Captchas
  let token = null;
  let version = 0;
  let puzzle = null;

  const setError = (text) => { $('challenge-text').textContent = text || ''; };

  const showPuzzle = (description) => {
    puzzle = description;
    version += 1;
    const slide = description.kind === 'slide';
    $('text-captcha').hidden = slide;
    $('slide-captcha').hidden = !slide;
    $('intro-text').textContent = slide
      ? 'Slide the piece into the hole to get access to the server.'
      : 'Type the characters shown in the image to get access to the server.';
    if (slide) {
      $('slide-bg').src = `${base}slide.png?t=${token}&v=${version}`;
      $('slide-piece').src = `${base}piece.png?t=${token}&v=${version}`;
      resetSlider();
      layoutSlide();
    } else {
      $('captcha').src = `${base}captcha.png?t=${token}&v=${version}`;
      $('answer').value = '';
      $('answer').focus();
    }
  };

  const finished = () => {
    show('done');
    if (inActivity) {
      // Give the member a moment to read, then close the Activity.
      setTimeout(() => {
        client.postMessage([OP_CLOSE, { code: 1000, message: 'Verified', nonce: nonce() }], clientOrigin);
      }, 2500);
    } else {
      $('done-text').textContent = 'You are verified. Welcome! You can close this page and go back to Discord.';
    }
  };

  let busy = false;
  const submit = async (answer) => {
    if (busy || !token) return;
    busy = true;
    setError('');
    const result = await post('answer', { token, answer: String(answer) });
    busy = false;
    if (result.ok) return finished();
    if (result.restart) return show('failed', result.error);
    setError(result.error);
    if (result.newImage) showPuzzle(result);
  };

  const another = async () => {
    if (busy || !token) return;
    busy = true;
    const result = await post('refresh', { token });
    busy = false;
    setError('');
    if (result.ok) showPuzzle(result);
    else show('failed', result.error);
  };

  $('form').addEventListener('submit', (event) => {
    event.preventDefault();
    const answer = $('answer').value.trim();
    if (answer) submit(answer);
  });
  $('another').addEventListener('click', another);
  $('another-slide').addEventListener('click', another);
  $('retry').addEventListener('click', () => location.reload());

  // ---- Slide captcha: drag the handle; the piece follows.
  const slider = $('slider');
  const handle = $('slider-handle');
  const fill = $('slider-fill');
  let position = 0; // in puzzle pixels
  let dragging = null;

  const scale = () => $('slide-frame').clientWidth / (puzzle?.width || 460);
  const maxPosition = () => (puzzle ? puzzle.width - puzzle.piece : 0);
  const trackRange = () => slider.clientWidth - handle.offsetWidth;

  const draw = () => {
    const ratio = maxPosition() ? position / maxPosition() : 0;
    const offset = ratio * trackRange();
    handle.style.transform = `translateX(${offset}px)`;
    fill.style.width = `${offset + handle.offsetWidth / 2}px`;
    $('slide-piece').style.transform = `translateX(${position * scale()}px)`;
    slider.setAttribute('aria-valuenow', String(Math.round(position)));
  };

  const layoutSlide = () => {
    if (!puzzle) return;
    const piece = $('slide-piece');
    const factor = scale();
    piece.style.width = `${puzzle.piece * factor}px`;
    piece.style.top = `${puzzle.top * factor}px`;
    slider.setAttribute('aria-valuemax', String(maxPosition()));
    draw();
  };

  const resetSlider = () => {
    position = 0;
    slider.classList.remove('done');
    draw();
  };

  const moveTo = (clientX) => {
    const rect = slider.getBoundingClientRect();
    const offset = Math.min(Math.max(clientX - rect.left - dragging.grab, 0), trackRange());
    position = trackRange() ? (offset / trackRange()) * maxPosition() : 0;
    draw();
  };

  handle.addEventListener('pointerdown', (event) => {
    if (busy) return;
    const rect = handle.getBoundingClientRect();
    dragging = { grab: event.clientX - rect.left };
    handle.setPointerCapture(event.pointerId);
    slider.classList.add('active');
  });
  handle.addEventListener('pointermove', (event) => {
    if (dragging) moveTo(event.clientX);
  });
  const release = () => {
    if (!dragging) return;
    dragging = null;
    slider.classList.remove('active');
    if (position > 0) {
      slider.classList.add('done');
      submit(Math.round(position));
    }
  };
  handle.addEventListener('pointerup', release);
  handle.addEventListener('pointercancel', release);

  // Keyboard: arrows move, Enter submits.
  slider.addEventListener('keydown', (event) => {
    const step = event.shiftKey ? 10 : 2;
    if (event.key === 'ArrowRight') position = Math.min(position + step, maxPosition());
    else if (event.key === 'ArrowLeft') position = Math.max(position - step, 0);
    else if (event.key === 'Enter') return submit(Math.round(position));
    else return;
    event.preventDefault();
    draw();
  });

  addEventListener('resize', layoutSlide);
  // The frame only has its final size once the background is loaded.
  $('slide-bg').addEventListener('load', layoutSlide);

  // ---- Start
  const begin = (result) => {
    if (!result.ok) {
      show('failed', result.error);
      if (result.restart && link) $('retry').hidden = true;
      return;
    }
    token = result.token;
    $('who').textContent = result.name;
    show('challenge');
    showPuzzle(result);
  };

  (async () => {
    try {
      if (link) {
        begin(await post('link', { link }));
        return;
      }
      client.postMessage(
        [OP_HANDSHAKE, { v: 1, encoding: 'json', client_id: clientId, frame_id: frameId }],
        clientOrigin,
      );
      await within(ready, 15000, 'Discord did not answer. Close this and try again.');
      const { code } = await command('AUTHORIZE', {
        client_id: clientId,
        response_type: 'code',
        state: '',
        prompt: 'none',
        scope: ['identify'],
      });
      begin(await post('session', { code, guild_id: guildId }));
    } catch (error) {
      show('failed', error.message || 'Something went wrong.');
    }
  })();
})();
