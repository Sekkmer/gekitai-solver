import { $ } from './view.js';

export function initializeTheme() {
  function themeButton() {
    const dark = document.documentElement.dataset.theme === 'dark';
    $('theme').textContent = dark ? 'Light mode' : 'Dark mode';
    $('theme').setAttribute(
      'aria-label',
      dark ? 'Switch to light mode' : 'Switch to dark mode',
    );
  }
  $('theme').onclick = () => {
    const theme =
      document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark';
    document.documentElement.dataset.theme = theme;
    try {
      localStorage.setItem('gekitai-theme', theme);
    } catch {}
    themeButton();
  };
  themeButton();
  matchMedia('(prefers-color-scheme: dark)').addEventListener('change', (e) => {
    try {
      if (localStorage.getItem('gekitai-theme')) return;
    } catch {}
    document.documentElement.dataset.theme = e.matches ? 'dark' : 'light';
    themeButton();
  });
}
