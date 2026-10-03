import fs from 'node:fs';
import postcss from 'postcss';
import prefixer from 'postcss-prefix-selector';
import { PurgeCSS } from 'purgecss';
const primer = fs.readFileSync('v21/node_modules/@primer/css/dist/primer.css', 'utf8');
const [res] = await new PurgeCSS().purge({
  content: [{ raw: fs.readFileSync('../site/hub2.js', 'utf8'), extension: 'js' }],
  css: [{ raw: primer }],
  variables: false,
  keyframes: true,
  safelist: { standard: [/^h[1-6]$/, 'p', 'ul', 'ol', 'li', 'blockquote', 'pre', 'code', 'table', 'thead', 'tbody', 'tr', 'th', 'td', 'hr', 'strong', 'em', 'a', 'img', 'svg', 'html', 'body', ':root', '*'], greedy: [/markdown-body/, /data-color-mode/, /data-dark-theme/, /Progress/, /^color-bg-success/] },
});
// Only Primer's dark theme: drop every other theme's variable block.
const onlyDark = { postcssPlugin: 'only-dark', Rule(rule) {
  if (!/data-(color-mode|dark-theme|light-theme)/.test(rule.selector)) return;
  const keep = rule.selectors.filter((x) => /\[data-color-mode=dark\]\[data-dark-theme=dark\]/.test(x));
  if (!keep.length) rule.remove(); else rule.selectors = keep;
} };
const scoped = postcss([onlyDark, prefixer({
  prefix: '.hub',
  transform(prefix, sel) {
    if (/^\[data-(color-mode|dark-theme|light-theme)/.test(sel)) return prefix + sel;
    if (/^(html|body|:root)$/.test(sel)) return prefix;
    if (/^(html|body)\s/.test(sel)) return sel.replace(/^(html|body)/, prefix);
    return prefix + ' ' + sel;
  },
})]).process(res.css, { from: undefined }).css;
const banner = '/*! Primer CSS 21.5.1 (MIT, https://github.com/primer/css), purged to what the model hub uses and scoped to .hub. Octicons 19.38 (MIT). */\n';
fs.writeFileSync('primer-scoped.css', banner + scoped);
console.log('bytes', scoped.length);
