// MISAKA Model Transport: the window. All state lives in misaka-torrentd; this file only draws it
// and sends the user's choices. Nothing here starts a download without the user's click.
'use strict';
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const $ = (s, r) => (r || document).querySelector(s);
const esc = (v) => String(v == null ? '' : v).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
const icon = (n, cls) => '<svg class="octicon' + (cls ? ' ' + cls : '') + '" viewBox="0 0 16 16" width="16" height="16" fill="currentColor" aria-hidden="true">' + (window.OCTICONS[n] || '') + '</svg>';
function fillIcons(root) { (root || document).querySelectorAll('svg[data-icon]').forEach((s) => { s.outerHTML = icon(s.dataset.icon, s.getAttribute('class').replace('octicon', '').trim()); }); }
function bytes(n) { const u = ['B', 'KiB', 'MiB', 'GiB', 'TiB']; let v = Number(n || 0), i = 0; while (v >= 1024 && i < u.length - 1) { v /= 1024; i++; } return (i ? v.toFixed(i > 2 ? 2 : 1) : String(v)) + ' ' + u[i]; }
function flash(msg, kind) {
  $('#flash').innerHTML = '<div class="flash flash-' + (kind || 'error') + ' mb-3 d-flex flex-items-center"><span class="flex-auto">' + esc(msg) + '</span><button class="btn-octicon" aria-label="Dismiss">' + icon('x') + '</button></div>';
  $('#flash button').onclick = () => { $('#flash').innerHTML = ''; };
}

const STATE = { metadata: ['Finding peers', 'Label--secondary'], admitted: ['Checked', 'Label--accent'], downloading: ['Downloading', 'Label--accent'], stalled: ['Stalled', 'Label--attention'], verifying: ['Verifying', 'Label--accent'], sealed: ['Complete', 'Label--success'], seeding: ['Seeding', 'Label--success'], idle: ['Complete', 'Label--secondary'], paused: ['Paused', 'Label--secondary'], refused: ['Refused', 'Label--danger'], mismatch: ['Failed check', 'Label--danger'], failed: ['Failed', 'Label--danger'] };
const lib = new Map();
let daemon = null;

function row(b) {
  const [word, cls] = STATE[b.state] || [b.state, 'Label--secondary'];
  const pct = b.total_bytes ? Math.min(100, Math.floor((b.done_bytes * 100) / b.total_bytes)) : 0;
  const active = ['downloading', 'stalled', 'metadata'].includes(b.state);
  const ratio = b.total_bytes ? (b.uploaded / b.total_bytes).toFixed(2) : '—';
  const paused = b.state === 'paused';
  const failed = ['refused', 'mismatch', 'failed'].includes(b.state);
  return `<div class="Box-row" data-ih="${esc(b.infohash)}">
    <div class="d-flex flex-items-center mb-1">
      ${icon('package', 'color-fg-muted mr-2')}<strong class="f4 flex-auto css-truncate css-truncate-target app-title">${esc(b.title || b.infohash.slice(0, 16) + '…')}</strong>
      <span class="Label ${cls} mr-2">${word}</span>
      ${b.label === 'declared' || b.label === 'chain-verified' ? `<span class="Label Label--success mr-2" title="The bytes match the bundle commitment the index declared">${icon('verified', 'mr-1')}checked</span>` : ''}
      <div class="BtnGroup">
        ${failed ? '' : paused ? `<button class="btn btn-sm BtnGroup-item" data-act="resume" title="Resume">${icon('play')}</button>` : `<button class="btn btn-sm BtnGroup-item" data-act="pause" title="Pause">${icon('pause')}</button>`}
        ${b.path ? `<button class="btn btn-sm BtnGroup-item" data-act="open" title="Open folder">${icon('file-directory')}</button>` : ''}
        <button class="btn btn-sm BtnGroup-item" data-act="remove" title="Remove">${icon('trash')}</button>
      </div>
    </div>
    ${active ? `<span class="Progress my-2"><span class="Progress-item color-bg-accent-emphasis" style="width:${pct}%"></span></span>` : ''}
    <div class="f6 color-fg-muted d-flex flex-wrap app-meta">
      <span>${b.total_bytes ? (active ? bytes(b.done_bytes) + ' of ' + bytes(b.total_bytes) + ' (' + pct + '%)' : bytes(b.total_bytes)) : 'size not known yet'}</span>
      <span>${icon('download', 'mr-1')}${bytes(b.download_rate)}/s</span>
      <span>${icon('upload', 'mr-1')}${bytes(b.upload_rate)}/s</span>
      <span title="Uploaded / size">ratio ${ratio}</span>
      <span>${icon('people', 'mr-1')}${b.peers} peers · ${b.seeds} seeds</span>
      <span title="Uploaded in total">${icon('broadcast', 'mr-1')}${bytes(b.uploaded)} shared</span>
    </div>
    ${failed && b.reason ? `<div class="flash flash-error mt-2 f6">${esc(b.reason)}</div>` : ''}
  </div>`;
}

function renderLibrary() {
  const list = [...lib.values()].sort((a, b) => (a.title || '').localeCompare(b.title || ''));
  $('#libCount').textContent = list.length;
  const up = list.reduce((s, b) => s + (b.upload_rate || 0), 0), down = list.reduce((s, b) => s + (b.download_rate || 0), 0);
  const seeding = list.filter((b) => b.state === 'seeding').length;
  $('#totals').innerHTML = `${icon('broadcast', 'mr-1')}<span class="mr-3">${seeding} seeding</span>${icon('download', 'mr-1')}<span class="mr-3">${bytes(down)}/s</span>${icon('upload', 'mr-1')}<span class="mr-3">${bytes(up)}/s</span>` + (daemon ? `<span class="flex-auto"></span><span title="${esc(daemon.confinement)}">${icon('shield-check', 'mr-1')}engine ${esc(daemon.engine)} · port ${daemon.listen_port}</span>` : '');
  $('#library').innerHTML = list.length ? list.map(row).join('') : `<div class="blankslate blankslate-spacious">${icon('package', 'blankslate-icon')}<h3 class="blankslate-heading">No models yet</h3><p>Open a model on misakaoptions.com and choose <b>Download with MISAKA</b>, or add model files you already have.</p><div class="mt-3"><a class="btn btn-primary mr-2" href="https://misakaoptions.com/#/models" target="_blank" rel="noopener">Browse models</a><button class="btn" id="adoptEmpty">Add existing model…</button></div></div>`;
  const ae = $('#adoptEmpty'); if (ae) ae.onclick = doAdopt;
}

async function refresh() {
  try {
    const s = await invoke('status');
    daemon = s.daemon;
    lib.clear();
    for (const b of s.bundles) lib.set(b.infohash, b);
    renderLibrary();
  } catch (e) { flash(String(e)); }
}

$('#library').addEventListener('click', async (ev) => {
  const btn = ev.target.closest('button[data-act]'); if (!btn) return;
  const ih = btn.closest('[data-ih]').dataset.ih, act = btn.dataset.act;
  try {
    if (act === 'open') await invoke('open_folder', { infohash: ih });
    else if (act === 'remove') askRemove(ih);
    else { await invoke('control', { action: act, infohash: ih }); await refresh(); }
  } catch (e) { flash(String(e)); }
});

function askRemove(ih) {
  const b = lib.get(ih);
  $('#rText').textContent = 'Remove ' + (b.title || ih) + ' from your library? It stops seeding.';
  $('#rDelete').checked = false;
  $('#removeDlg').hidden = false;
  $('#rCancel').onclick = () => { $('#removeDlg').hidden = true; };
  $('#rOk').onclick = async () => {
    $('#removeDlg').hidden = true;
    try { await invoke('control', { action: $('#rDelete').checked ? 'delete' : 'remove', infohash: ih }); lib.delete(ih); renderLibrary(); } catch (e) { flash(String(e)); }
  };
}

// ---- a misaka-model:// link: resolve, show, and wait for the user ----
async function onLink(url) {
  showTab('library');
  $('#cTitle').textContent = 'Download model?';
  $('#cBody').innerHTML = '<div class="d-flex flex-items-center color-fg-muted">' + icon('sync', 'mr-2 anim-rotate') + 'Looking up ' + esc(url) + '…</div>';
  $('#cOk').disabled = true;
  $('#confirm').hidden = false;
  let r;
  try { r = await invoke('resolve', { link: url }); } catch (e) { $('#cBody').innerHTML = '<div class="flash flash-error">' + esc(e) + '</div>'; return; }
  const have = r.existing && !['refused', 'mismatch', 'failed'].includes(r.existing.state);
  $('#cTitle').textContent = have ? 'Already in your library' : 'Download ' + (r.repo || r.title) + '?';
  $('#cBody').innerHTML = `
    <div class="d-flex flex-items-center mb-3">${icon('package', 'mr-2 color-fg-muted')}<span class="f3"><span class="color-fg-muted">${esc((r.repo || '').split('/')[0])}${r.repo ? ' / ' : ''}</span><strong>${esc((r.repo || r.title).split('/').pop())}</strong></span></div>
    <dl class="app-dl f5">
      <dt>Size</dt><dd><strong>${bytes(r.total_bytes)}</strong> (${Number(r.total_bytes).toLocaleString()} bytes) · ${r.files.length + 1} files</dd>
      <dt>Version</dt><dd>v${r.version} · ${esc(r.kind)} · ${esc(r.network)}</dd>
      ${r.base_model ? `<dt>Base model</dt><dd>${esc(r.base_model)}${r.params ? ' · ' + esc(r.params) : ''}</dd>` : ''}
      <dt>License</dt><dd>${esc(r.license || '—')}</dd>
      <dt>Save to</dt><dd class="text-mono f6 wb-break-all">${esc(r.destination)}</dd>
      <dt>Checked against</dt><dd class="text-mono f6">commitment ${esc(r.bundle_commitment.slice(0, 16))}… <span class="color-fg-muted">from ${esc(new URL(r.index).host)}</span></dd>
    </dl>
    <p class="f6 color-fg-muted mt-3 mb-0">${icon('info', 'mr-1')}Downloaded over BitTorrent v2 from other MISAKA users; every block is checked, and the whole bundle against its commitment, before it is kept. When it completes, your copy is shared back while the app runs (Settings → Seed completed downloads).</p>
    ${have ? `<div class="flash mt-3">This model is already in your library (${esc((STATE[r.existing.state] || [r.existing.state])[0])}).</div>` : ''}`;
  $('#cOk').disabled = !!have;
  $('#cOk').onclick = async () => {
    $('#cOk').disabled = true;
    try {
      await invoke('fetch', { infohash: r.infohash, bundleCommitment: r.bundle_commitment, totalBytes: r.total_bytes, kind: r.kind });
      $('#confirm').hidden = true;
      await refresh();
    } catch (e) { $('#cBody').insertAdjacentHTML('beforeend', '<div class="flash flash-error mt-3">' + esc(e) + '</div>'); $('#cOk').disabled = false; }
  };
}
for (const id of ['#cCancel', '#cClose']) $(id).onclick = () => { $('#confirm').hidden = true; };
document.addEventListener('keydown', (e) => { if (e.key === 'Escape') { $('#confirm').hidden = true; $('#removeDlg').hidden = true; } });

async function doAdopt() {
  try {
    const r = await invoke('adopt');
    if (r) { flash('Checking ' + (r.repo || r.title) + ': every byte is re-hashed, then it is shared in place. Nothing was copied.', 'success'); await refresh(); }
  } catch (e) { flash(String(e)); }
}
$('#adoptBtn').onclick = doAdopt;

// ---- settings ----
function showTab(t) {
  document.querySelectorAll('[data-tab]').forEach((a) => a.toggleAttribute('aria-current', a.dataset.tab === t));
  document.querySelectorAll('[data-tab]').forEach((a) => { if (a.dataset.tab === t) a.setAttribute('aria-current', 'page'); });
  $('#tab-library').hidden = t !== 'library';
  $('#tab-settings').hidden = t !== 'settings';
  if (t === 'settings') loadSettings();
}
document.querySelectorAll('[data-tab]').forEach((a) => a.addEventListener('click', (e) => { e.preventDefault(); showTab(a.dataset.tab); }));
async function loadSettings() {
  try {
    const s = await invoke('settings_get');
    $('#sUp').value = s.upload_mib; $('#sDown').value = s.download_mib; $('#sConn').value = s.connections;
    $('#sSeed').checked = s.seeding; $('#sUpnp').checked = s.upnp; $('#sAuto').checked = s.autostart; $('#sIndex').value = s.index_url;
    $('#daemonInfo').textContent = daemon ? 'misaka-torrentd ' + daemon.version + ' · ' + daemon.confinement : '';
  } catch (e) { flash(String(e)); }
}
$('#settingsForm').addEventListener('submit', async (e) => {
  e.preventDefault();
  const s = { upload_mib: Number($('#sUp').value) || 0, download_mib: Number($('#sDown').value) || 0, connections: Number($('#sConn').value) || 200, seeding: $('#sSeed').checked, upnp: $('#sUpnp').checked, autostart: $('#sAuto').checked, index_url: $('#sIndex').value.trim() };
  try { await invoke('settings_set', { s }); flash('Settings saved.', 'success'); await refresh(); } catch (err) { flash(String(err)); }
});

// ---- start ----
fillIcons();
listen('bundle', (ev) => { const b = ev.payload; lib.set(b.infohash, b); renderLibrary(); });
listen('deep-link', (ev) => onLink(ev.payload));
listen('daemon-error', (ev) => flash('The transport daemon did not start: ' + ev.payload));
(async () => {
  await refresh();
  const pending = await invoke('take_pending_link');
  if (pending) onLink(pending);
  setInterval(refresh, 3000);
})();
