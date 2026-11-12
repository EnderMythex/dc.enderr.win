// Panel enhancements. Every page works without this script; it adds:
// - a "Recovering connection" screen while the panel restarts (redeploys);
// - a bar for unsaved changes on settings forms, with Save and Cancel;
// - a confirmation before saving a role pick that looks like a mistake
//   (a staff role given to members, a member role trusted as staff).
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

  const RELOADS = 'dc-offline-reloads';
  const storage = {
    get: () => { try { return Number(sessionStorage.getItem(RELOADS) || 0); } catch { return 0; } },
    set: (n) => { try { sessionStorage.setItem(RELOADS, String(n)); } catch { /* private mode */ } },
    clear: () => { try { sessionStorage.removeItem(RELOADS); } catch { /* private mode */ } },
  };

  if (offlinePage) {
    // A placeholder shown by the service worker: load the real page once the
    // panel answers again. If the placeholder keeps coming back while the
    // panel answers, the service worker is at fault: remove it.
    waitForPanel().then(async () => {
      const reloads = storage.get();
      if (reloads >= 2) {
        storage.clear();
        try {
          const registrations = await navigator.serviceWorker?.getRegistrations() ?? [];
          await Promise.all(registrations.map((registration) => registration.unregister()));
        } catch { /* nothing to remove */ }
      } else {
        storage.set(reloads + 1);
      }
      location.reload();
    });
  } else {
    storage.clear();
    setInterval(async () => {
      if (!overlay && document.visibilityState === 'visible' && !(await healthy())) lost();
    }, 5000);
  }

  // ------------------------------------------------------------ risky roles

  /** Picked roles that look like a mistake, as sentences. */
  const riskyPicks = (form) => {
    const found = [];
    for (const box of form.querySelectorAll('[data-warn]')) {
      const kind = box.dataset.warn;
      const where = box.dataset.warnLabel || 'this setting';
      const picked = box.tagName === 'SELECT'
        ? [...box.selectedOptions].filter((option) => option.value)
        : [...box.querySelectorAll('input:checked')];
      for (const item of picked) {
        const reason = kind === 'give' ? item.dataset.giveRisk : item.dataset.staffRisk;
        if (!reason) continue;
        const name = item.dataset.role ? `@${item.dataset.role}` : item.textContent.trim();
        const line = kind === 'give'
          ? `${name} in “${where}”: members get it, and ${reason}.`
          : `${name} in “${where}”: it gets staff rights, but ${reason}.`;
        if (!found.includes(line)) found.push(line);
      }
    }
    return found;
  };

  /** Asks before saving risky picks; resolves to true to save anyway. */
  const confirmRisks = (lines) => new Promise((resolve) => {
    const dialog = $('dialog', 'confirm');
    dialog.setAttribute('aria-labelledby', 'confirm-title');
    const title = $('h2', '', 'Is this role right?');
    title.id = 'confirm-title';
    const intro = $('p', 'muted', 'These choices are often a typo. Check them before saving:');
    const list = $('ul', 'confirm-list');
    for (const line of lines) list.append($('li', '', line));
    const back = $('button', 'btn ghost', 'Go back');
    const go = $('button', 'btn danger', 'Save anyway');
    back.type = 'button';
    go.type = 'button';
    const actions = $('div', 'confirm-actions');
    actions.append(back, go);
    dialog.append(title, intro, list, actions);
    document.body.append(dialog);
    const close = (answer) => {
      dialog.close();
      dialog.remove();
      resolve(answer);
    };
    back.addEventListener('click', () => close(false));
    go.addEventListener('click', () => close(true));
    dialog.addEventListener('cancel', (event) => {
      event.preventDefault();
      close(false);
    });
    dialog.showModal();
    back.focus();
  });

  // Never post a form to a panel that is down: the changes would be lost.
  let confirmedSubmit = false;
  document.addEventListener('submit', async (event) => {
    if (confirmedSubmit) return;
    const form = event.target;
    const submitter = event.submitter;
    event.preventDefault();
    const risks = riskyPicks(form);
    if (risks.length > 0 && !(await confirmRisks(risks))) return;
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

  // ------------------------------------------------------------ previews

  // Side color of the Discord previews (set from data-accent: no inline style).
  document.querySelectorAll('[data-accent]').forEach((box) => {
    if (/^#[0-9a-f]{6}$/.test(box.dataset.accent)) box.style.borderLeftColor = box.dataset.accent;
  });

  // ------------------------------------------------------------ theme

  document.querySelectorAll('[data-theme-toggle]').forEach((button) => {
    button.addEventListener('click', () => {
      const next = document.documentElement.dataset.theme === 'light' ? 'dark' : 'light';
      document.documentElement.dataset.theme = next;
      try {
        localStorage.setItem('theme', next);
      } catch {}
    });
  });

  // ------------------------------------------------------------ patch notes

  // A dot on the button until the newest notes were opened.
  const seenNotes = document.querySelector('[data-notes-seen]');
  try {
    if (seenNotes) localStorage.setItem('notes-seen', seenNotes.dataset.notesSeen);
    const seen = localStorage.getItem('notes-seen');
    document.querySelectorAll('[data-notes]').forEach((link) => {
      link.classList.toggle('unseen', link.dataset.notes !== seen);
    });
  } catch {}

  // ------------------------------------------------------------ import review

  document.querySelectorAll('[data-review]').forEach((section) => {
    const all = section.querySelector('[data-review-all]');
    const boxes = [...section.querySelectorAll('input[name="accept"]')];
    const refresh = () => {
      boxes.forEach((box) => box.closest('.review-row').classList.toggle('refused', !box.checked));
      const ticked = boxes.filter((box) => box.checked).length;
      all.checked = ticked === boxes.length;
      all.indeterminate = ticked > 0 && ticked < boxes.length;
    };
    all.addEventListener('change', () => {
      boxes.forEach((box) => { box.checked = all.checked; });
      refresh();
    });
    boxes.forEach((box) => box.addEventListener('change', refresh));
  });

  // ------------------------------------------------------------ retry

  // Discord asked to slow down: the error page reloads after a few seconds.
  const retry = document.querySelector('[data-retry]');
  if (retry) {
    const seconds = Number(retry.dataset.retry) || 5;
    setTimeout(() => location.reload(), seconds * 1000);
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
