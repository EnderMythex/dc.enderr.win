// Runs before the page is drawn, so it never flashes the wrong theme: the
// theme chosen with the button, else the one of the system.
(() => {
  let theme = null;
  try {
    theme = localStorage.getItem('theme');
  } catch {}
  if (theme !== 'light' && theme !== 'dark') {
    theme = matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
  }
  document.documentElement.dataset.theme = theme;
})();
