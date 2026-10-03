// Builds app/ui/primer-app.css: Primer CSS 21.5.1 purged to the classes the app uses, dark theme only.
import fs from 'node:fs';
import postcss from 'postcss';
import { PurgeCSS } from 'purgecss';
const ui = process.argv[2];
const primer = fs.readFileSync('v21/node_modules/@primer/css/dist/primer.css', 'utf8');
const [res] = await new PurgeCSS().purge({
  content: [`${ui}/index.html`, `${ui}/app.js`],
  css: [{ raw: primer }],
  variables: false,
  keyframes: true,
  safelist: { standard: ['html', 'body', ':root', '*', 'button', 'input', 'label', 'p', 'strong', 'dl', 'dt', 'dd', 'svg', 'a', 'h2', 'h3'], greedy: [/data-color-mode/, /data-dark-theme/, /Progress/, /^Label--/, /^flash/, /^color-bg-/] },
});
const onlyDark = { postcssPlugin: 'only-dark', Rule(rule) {
  if (!/data-(color-mode|dark-theme|light-theme)/.test(rule.selector)) return;
  const keep = rule.selectors.filter((x) => /\[data-color-mode=dark\]\[data-dark-theme=dark\]/.test(x));
  if (!keep.length) rule.remove(); else rule.selectors = keep;
} };
const out = postcss([onlyDark]).process(res.css, { from: undefined }).css;
fs.writeFileSync(`${ui}/primer-app.css`, '/*! Primer CSS 21.5.1 (MIT, https://github.com/primer/css), purged to what this app uses. */\n' + out);
console.log('bytes', out.length);
