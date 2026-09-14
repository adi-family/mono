/* The only script on this site, and it decides nothing a reader cannot see.
 *
 * A web page cannot look at somebody's disk. The two mechanisms that exist for "is this app
 * installed" are both closed to us today: `http://app.adi` cannot be *fetched* from an https page
 * (mixed content) and adi-app refuses an /api request whose Origin is not its own Host anyway,
 * and there is no `adi://` scheme registered to deep-link into. Guessing from the user agent
 * would be a guess.
 *
 * So the site asks once and remembers. Every page carries both answers in its markup — "get adi"
 * and "here are the commands" — and this file only decides which of the two is in front. With
 * no script at all the page shows the first, which is the right default for a stranger, and the
 * second is still there to read. The controls that do the asking are marked `js-only`, so they
 * are never dead buttons.
 */
(() => {
  const KEY = 'adi.installed';

  const remember = (value) => {
    try {
      if (value) localStorage.setItem(KEY, value);
      else localStorage.removeItem(KEY);
    } catch (e) {
      /* Private browsing, or storage turned off: the answer lasts for this page and no longer,
         which is a worse experience and not a broken one. */
    }
  };

  document.addEventListener('click', (event) => {
    const control = event.target.closest('[data-adi-set]');
    if (!control) return;
    event.preventDefault();
    const value = control.getAttribute('data-adi-set');
    remember(value);
    document.documentElement.dataset.adi = value;
  });

  /* `/get/?from=<marketplace>/<slug>` — which item somebody left to come and install adi. Checked
     against the address shape rather than trusted: it is written into the page as text, so the
     worst a bad one could do is print nonsense, and printing nonsense is still worth refusing. */
  const from = new URLSearchParams(location.search).get('from') || '';
  if (!/^[A-Za-z0-9._-]+\/[A-Za-z0-9._-]+$/.test(from)) return;

  document.querySelectorAll('[data-from]').forEach((el) => {
    switch (el.getAttribute('data-from')) {
      case 'address':
        el.textContent = from;
        break;
      case 'install':
        el.textContent = `adi-mono marketplace install ${from}`;
        break;
      case 'back':
        el.setAttribute('href', `../${from}/`);
        el.hidden = false;
        break;
      default:
        break;
    }
  });
})();
