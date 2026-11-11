// Verification page shown inside a Discord Activity. This is the only
// JavaScript of the panel: an Activity has to talk to the Discord client.
(() => {
  'use strict';

  const { base, clientId } = document.body.dataset;
  const query = new URLSearchParams(location.search);
  const frameId = query.get('frame_id');
  const guildId = query.get('guild_id');
  const $ = (id) => document.getElementById(id);

  const STEPS = ['loading', 'challenge', 'done', 'failed'];
  const show = (step, message) => {
    for (const name of STEPS) $(name).hidden = name !== step;
    if (message) $(`${step}-text`).textContent = message;
  };

  if (!frameId || !guildId) {
    show('failed', 'Open this page with the Verify button of a server, inside Discord.');
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

  const command = (cmd, args) => new Promise((resolve, reject) => {
    const id = nonce();
    pending.set(id, { resolve, reject });
    client.postMessage([OP_FRAME, { cmd, args, nonce: id }], clientOrigin);
  });

  const within = (promise, ms, message) => Promise.race([
    promise,
    new Promise((_, reject) => { setTimeout(() => reject(new Error(message)), ms); }),
  ]);

  // ---- Our own endpoints, reached through Discord's proxy.
  const post = async (path, body) => {
    const response = await fetch(base + path, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
    });
    try {
      return await response.json();
    } catch {
      return { ok: false, error: 'Something went wrong. Try again in a moment.' };
    }
  };

  let token = null;
  let imageVersion = 0;
  const loadImage = () => {
    imageVersion += 1;
    $('captcha').src = `${base}captcha.png?t=${token}&v=${imageVersion}`;
    $('answer').value = '';
    $('answer').focus();
  };
  const setBusy = (busy) => {
    $('submit').disabled = busy;
    $('another').disabled = busy;
  };
  const fail = (result) => {
    if (result.restart) show('failed', result.error);
    else $('challenge-text').textContent = result.error;
  };

  $('form').addEventListener('submit', async (event) => {
    event.preventDefault();
    const answer = $('answer').value.trim();
    if (!answer || !token) return;
    setBusy(true);
    const result = await post('answer', { token, answer });
    setBusy(false);
    if (result.ok) {
      show('done');
      // Give the member a moment to read, then close the Activity.
      setTimeout(() => {
        client.postMessage([OP_CLOSE, { code: 1000, message: 'Verified', nonce: nonce() }], clientOrigin);
      }, 2500);
      return;
    }
    fail(result);
    if (result.newImage) loadImage();
  });

  $('another').addEventListener('click', async () => {
    if (!token) return;
    setBusy(true);
    const result = await post('refresh', { token });
    setBusy(false);
    if (result.ok) loadImage();
    else fail(result);
  });

  $('retry').addEventListener('click', () => location.reload());

  (async () => {
    try {
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
      const result = await post('session', { code, guild_id: guildId });
      if (!result.ok) {
        show('failed', result.error);
        return;
      }
      token = result.token;
      $('who').textContent = result.name;
      show('challenge');
      loadImage();
    } catch (error) {
      show('failed', error.message || 'Something went wrong.');
    }
  })();
})();
