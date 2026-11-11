// Panel enhancements. Every page works without this script; it adds:
// - a "Recovering connection" screen while the panel restarts (redeploys);
// - a bar for unsaved changes on settings forms, with Save and Cancel.
(() => {
  'use strict';

  document.documentElement.classList.add('js');
  const offlinePage = document.querySelector('[data-offline]') !== null;
  let leaving = false;
  const $ = (tag, cls, text) => {
    const node = document.createElement(tag);
    if (cls) node.className = cls;
    if (text) node.textContent = text;
    return node;
  };

  // ------------------------------------------------------------ connection

  // Answers from the panel carry this header; a proxy error page does not.
  const isPanel = (response) => response.ok && response.headers.has('x-dc-panel');
  const healthy = async () => {
    try {
      return isPanel(await fetch('/healthz', { cache: 'no-store' }));
    } catch {
      return false;
    }
  };

  let overlay = null;
  let waiting = null;
  const showOverlay = () => {
    if (overlay) return;
    overlay = $('div', 'recovering');
    overlay.setAttribute('role', 'alert');
    const card = $('div', 'recovering-card');
    card.append(
      $('span', 'spinner'),
      $('strong', '', 'Recovering connection to the panel'),
      $('span', 'muted', 'The panel is restarting. This page comes back by itself.'),
    );
    overlay.append(card);
    document.body.append(overlay);
    document.documentElement.classList.add('offline');
  };
  const hideOverlay = () => {
    overlay?.remove();
    overlay = null;
    document.documentElement.classList.remove('offline');
  };

  /** Resolves once the panel answers again. */
  const waitForPanel = () => {
    waiting ??= new Promise((resolve) => {
      const retry = async () => {
        if (await healthy()) {
          waiting = null;
          resolve();
        } else {
          setTimeout(retry, 2000);
        }
      };
      retry();
    });
    return waiting;
  };

  const lost = async () => {
    showOverlay();
    await waitForPanel();
    hideOverlay();
  };

  if (offlinePage) {
    // A placeholder shown by the service worker: load the real page once the
    // panel answers again.
    waitForPanel().then(() => location.reload());
  } else {
    setInterval(async () => {
      if (!overlay && document.visibilityState === 'visible' && !(await healthy())) lost();
    }, 5000);
  }

  // Never post a form to a panel that is down: the changes would be lost.
  let confirmedSubmit = false;
  document.addEventListener('submit', async (event) => {
    if (confirmedSubmit) return;
    const form = event.target;
    const submitter = event.submitter;
    event.preventDefault();
    if (!(await healthy())) {
      await lost();
    }
    confirmedSubmit = true;
    if (form.dataset.track !== undefined) leaving = true;
    form.requestSubmit(submitter && submitter.form === form ? submitter : undefined);
    confirmedSubmit = false;
  });

  // Offline page for navigations while the panel restarts.
  if ('serviceWorker' in navigator && window.isSecureContext) {
    navigator.serviceWorker.register('/sw.js').catch(() => {});
  }

  // ------------------------------------------------------------ unsaved changes

  const tracked = [...document.querySelectorAll('form[data-track]')];
  if (tracked.length === 0) return;

  const snapshot = (form) => {
    const parts = [];
    for (const [name, value] of new FormData(form)) {
      if (name === 'csrf') continue;
      parts.push(`${name}=${value instanceof File ? `${value.name}:${value.size}` : value}`);
    }
    return parts.join('\n');
  };
  const initial = new Map(tracked.map((form) => [form, snapshot(form)]));
  const isDirty = (form) => snapshot(form) !== initial.get(form);

  let current = null;
  const bar = $('div', 'savebar');
  bar.setAttribute('role', 'status');
  const message = $('span', 'savebar-text', 'You have unsaved changes');
  const cancel = $('button', 'btn ghost', 'Cancel');
  const save = $('button', 'btn primary', 'Save');
  cancel.type = 'button';
  save.type = 'button';
  const actions = $('div', 'savebar-actions');
  actions.append(cancel, save);
  bar.append(message, actions);
  document.body.append(bar);

  const refresh = (form) => {
    if (form) current = isDirty(form) ? form : current === form ? null : current;
    current ??= tracked.find(isDirty) ?? null;
    bar.classList.toggle('show', current !== null);
  };

  for (const form of tracked) {
    form.addEventListener('input', () => refresh(form));
    form.addEventListener('change', () => refresh(form));
  }

  cancel.addEventListener('click', () => {
    if (!current) return;
    current.reset();
    const form = current;
    current = null;
    refresh(form);
  });
  save.addEventListener('click', () => {
    if (!current) return;
    if (!current.reportValidity()) return;
    current.requestSubmit();
  });

  window.addEventListener('beforeunload', (event) => {
    if (leaving || !tracked.some(isDirty)) return;
    event.preventDefault();
    // Older browsers show the dialog only when this is set.
    event.returnValue = '';
  });

  // Shake the bar when someone tries to leave through a link.
  document.addEventListener('click', (event) => {
    const link = event.target.closest('a[href]');
    if (!link || !current || link.target === '_blank') return;
    bar.classList.remove('shake');
    void bar.offsetWidth;
    bar.classList.add('shake');
  });
})();
