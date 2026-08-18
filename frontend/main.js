/* QMC Decoder frontend — talks to the Tauri shell through the global
   __TAURI__ API (no bundler, no imports). Every heavy operation happens in
   a Rust command, so the page only drives the UI and the command results. */

(() => {
  'use strict';

  const TAURI = window.__TAURI__;
  const invoke = (TAURI && TAURI.core && TAURI.core.invoke)
    ? TAURI.core.invoke
    : () => Promise.reject(new Error('Tauri 环境不可用'));

  // -------------------------------------------------------------------------
  // State
  // -------------------------------------------------------------------------
  const state = {
    files: new Map(), // path -> { path, name, status, message, output, isDir }
    outputDir: '',
    ekey: '',
    autoFetch: false,
    showEkey: false,
    busy: false,
  };

  // -------------------------------------------------------------------------
  // DOM
  // -------------------------------------------------------------------------
  const $ = (id) => document.getElementById(id);
  const filelist = $('filelist');
  const emptyHint = $('empty-hint');
  const countEl = $('count');
  const dropzone = $('dropzone');
  const toastEl = $('toast');
  const toastMsg = $('toast-msg');
  const toastClose = $('toast-close');
  const progress = $('progress');
  const progressFill = $('progressFill');
  const progressText = $('progressText');
  const btnFiles = $('btn-files');
  const btnFolder = $('btn-folder');
  const btnOutput = $('btn-output');
  const btnDecrypt = $('btn-decrypt');
  const btnClear = $('btn-clear');
  const inputOutput = $('outputDir');
  const inputEkey = $('ekey');
  const btnToggleEkey = $('btn-toggle-ekey');
  const autoFetch = $('autoFetch');
  const credNote = $('cred-note');
  const modal = $('modal');
  const infoDetails = $('info-details');
  const infoPath = $('info-path');
  const modalClose = $('modal-close');

  // -------------------------------------------------------------------------
  // Inline SVG icons (stroke-based, inherit currentColor)
  // -------------------------------------------------------------------------
  const ICONS = {
    folder: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z"/></svg>',
    info: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="10"/><line x1="12" y1="16" x2="12" y2="12"/><line x1="12" y1="8" x2="12.01" y2="8"/></svg>',
    close: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>',
  };

  // -------------------------------------------------------------------------
  // Tiny helpers
  // -------------------------------------------------------------------------
  function basename(p) {
    const norm = String(p).replace(/[\\/]+$/, '');
    const parts = norm.split(/[\\/]/);
    return parts[parts.length - 1] || p;
  }

  // -------------------------------------------------------------------------
  // Toast 通知（固定定位，不挤动布局；err 常驻带关闭，ok 自动消失）
  // -------------------------------------------------------------------------
  let toastTimer = null;
  function hideToast() { toastEl.hidden = true; }
  function notify(msg, kind, sticky) {
    toastMsg.textContent = msg;
    toastEl.className = 'toast' + (kind ? ' ' + kind : '');
    toastEl.hidden = false;
    toastClose.hidden = !(kind === 'err' || sticky);
    clearTimeout(toastTimer);
    if (!sticky && kind !== 'err') {
      toastTimer = setTimeout(hideToast, 4500);
    }
  }
  toastClose.addEventListener('click', hideToast);

  // -------------------------------------------------------------------------
  // 全局进度（由 Rust 端逐文件事件驱动，事件不可用时降级为不确定态动画）
  // -------------------------------------------------------------------------
  let progressTotal = 0;
  let progressDone = 0;
  function showProgress(active) {
    progress.hidden = !active;
    if (!active) {
      progress.classList.remove('busy');
      return;
    }
    progress.classList.add('busy');
    progressTotal = state.files.size;
    progressDone = 0;
    progressFill.style.width = '0%';
    progressText.textContent = '0/' + progressTotal;
  }
  function updateProgress(done, total) {
    if (!total || total <= 0) return;
    progressTotal = total;
    progressDone = done;
    progress.classList.remove('busy');
    const pct = Math.min(100, Math.round((done / total) * 100));
    progressFill.style.width = pct + '%';
    progressText.textContent = done + '/' + total;
  }

  function setBusy(b) {
    state.busy = b;
    [btnDecrypt, btnFiles, btnFolder, btnClear].forEach((el) => { el.disabled = b; });
    if (b) {
      const s = document.createElement('span');
      s.className = 'spinner';
      s.id = 'decrypt-spinner';
      btnDecrypt.prepend(s);
      btnDecrypt.classList.add('loading');
    } else {
      const s = $('decrypt-spinner');
      if (s) s.remove();
      btnDecrypt.classList.remove('loading');
    }
  }

  // -------------------------------------------------------------------------
  // File list
  // -------------------------------------------------------------------------
  // Folders are expanded on the Rust side into their contained supported
  // songs so the UI lists the songs, not a single folder row.
  async function addPaths(paths) {
    const clean = (paths || []).filter((p) => p && !state.files.has(p));
    if (!clean.length) return;

    let items;
    try {
      items = await invoke('inspect_paths', { paths: clean });
    } catch (e) {
      notify('读取路径失败：' + e, 'err');
      return;
    }

    let added = 0;
    let emptyDirs = [];
    for (const item of items || []) {
      if (!item) continue;
      if (item.isDir) {
        if (!item.songs.length) {
          emptyDirs.push(item.path);
          continue;
        }
        for (const s of item.songs) {
          if (state.files.has(s)) continue;
          state.files.set(s, {
            path: s,
            name: basename(s),
            status: 'queued',
            message: '',
            output: '',
            isDir: false,
          });
          added += 1;
        }
      } else {
        state.files.set(item.path, {
          path: item.path,
          name: basename(item.path),
          status: 'queued',
          message: '',
          output: '',
          isDir: false,
        });
        added += 1;
      }
    }
    render();
    if (emptyDirs.length) {
      notify(`${emptyDirs.length} 个文件夹里没有找到支持的加密文件`, 'warn');
    } else if (added) {
      notify(`已添加 ${added} 个文件`, 'ok');
    }
  }

  function removePath(path) { state.files.delete(path); render(); }
  function clearAll() { state.files.clear(); render(); }

  const STATUS_TEXT = { queued: '待处理', working: '解密中', ok: '成功', err: '失败' };

  function render() {
    filelist.innerHTML = '';
    emptyHint.textContent = state.files.size === 0 ? '尚未添加任何文件' : '';
    emptyHint.hidden = state.files.size > 0;
    countEl.textContent = state.files.size + ' 项';
    countEl.hidden = state.files.size === 0;
    document.querySelector('.files-card').classList.toggle('has-files', state.files.size > 0);

    for (const f of state.files.values()) {
      const li = document.createElement('li');
      li.className = 'file ' + f.status;
      if (f.isDir) li.dataset.dir = '1';

      const name = document.createElement('div');
      name.className = 'file-name';
      name.title = f.path;
      if (f.isDir) {
        const ico = document.createElement('span');
        ico.className = 'file-ico';
        ico.setAttribute('aria-hidden', 'true');
        ico.innerHTML = ICONS.folder;
        name.appendChild(ico);
      }
      const label = document.createElement('span');
      label.className = 'file-label';
      label.textContent = f.name;
      name.appendChild(label);
      li.appendChild(name);

      const pathLine = document.createElement('div');
      pathLine.className = 'file-path';
      pathLine.textContent = f.path;
      li.appendChild(pathLine);

      const badge = document.createElement('span');
      badge.className = 'badge ' + f.status;
      if (f.status === 'working') {
        badge.innerHTML = '<span class="mini-spinner"></span>解密中…';
      } else {
        badge.textContent = STATUS_TEXT[f.status] || f.status;
      }
      li.appendChild(badge);

      const infoBtn = document.createElement('button');
      infoBtn.className = 'file-info-btn';
      infoBtn.setAttribute('aria-label', '查看文件信息');
      infoBtn.innerHTML = ICONS.info;
      infoBtn.title = '查看文件信息';
      infoBtn.disabled = state.busy;
      infoBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        showInfo(f.path);
      });
      if (!f.isDir) li.appendChild(infoBtn);

      const removeBtn = document.createElement('button');
      removeBtn.className = 'file-remove';
      removeBtn.setAttribute('aria-label', '移除');
      removeBtn.innerHTML = ICONS.close;
      removeBtn.title = '移除';
      removeBtn.disabled = state.busy;
      removeBtn.addEventListener('click', (e) => {
        e.stopPropagation();
        removePath(f.path);
      });
      li.appendChild(removeBtn);

      if (f.output) {
        const out = document.createElement('div');
        out.className = 'file-output';
        out.textContent = '→ ' + f.output;
        out.title = f.output;
        li.appendChild(out);
      }
      if (f.message) {
        const msg = document.createElement('div');
        msg.className = 'file-msg';
        msg.textContent = f.message;
        msg.title = f.message;
        li.appendChild(msg);
      }

      if (!f.isDir && !state.busy) {
        li.addEventListener('click', () => showInfo(f.path));
      }
      filelist.appendChild(li);
    }
  }

  // -------------------------------------------------------------------------
  // Info modal
  // -------------------------------------------------------------------------
  async function showInfo(path) {
    try {
      const info = await invoke('get_file_info', { path });
      infoDetails.innerHTML = '';
      for (const [k, v] of info.details) {
        const dt = document.createElement('dt');
        dt.textContent = k;
        const dd = document.createElement('dd');
        dd.textContent = v;
        infoDetails.appendChild(dt);
        infoDetails.appendChild(dd);
      }
      infoPath.textContent = path;
      modal.hidden = false;
    } catch (e) {
      notify('读取文件信息失败：' + e, 'err');
    }
  }

  function closeModal() { modal.hidden = true; }

  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && !modal.hidden) closeModal();
  });

  // -------------------------------------------------------------------------
  // Decrypt
  // -------------------------------------------------------------------------
  async function decrypt() {
    if (state.files.size === 0) { notify('请先添加文件或文件夹', 'err'); return; }

    const inputs = [...state.files.keys()];
    setBusy(true);
    inputs.forEach((p) => {
      const f = state.files.get(p);
      f.status = 'working';
      f.message = '';
      f.output = '';
    });
    render();
    showProgress(true);
    progressTotal = inputs.length;
    notify('正在解密…', '', true);

    try {
      const results = await invoke('decrypt_paths', {
        paths: inputs,
        outputDir: state.outputDir ? state.outputDir : null,
        ekey: state.ekey ? state.ekey : null,
        fetchEkey: state.autoFetch,
      });
      mergeResults(results, inputs);
    } catch (e) {
      inputs.forEach((p) => {
        const f = state.files.get(p);
        if (f) { f.status = 'err'; f.message = String(e); }
      });
      notify('解密失败：' + e, 'err');
    } finally {
      setBusy(false);
      showProgress(false);
      render();
    }
  }

  function mergeResults(results, inputs) {
    let ok = 0;
    let fail = 0;
    const covered = new Set();

    for (const r of results) {
      covered.add(r.inputPath);
      let f = state.files.get(r.inputPath);
      if (!f) {
        f = {
          path: r.inputPath,
          name: basename(r.inputPath),
          status: r.success ? 'ok' : 'err',
          message: r.message || '',
          output: r.outputPath || '',
          isDir: false,
        };
        state.files.set(r.inputPath, f);
      } else {
        f.status = r.success ? 'ok' : 'err';
        f.message = r.message || '';
        f.output = r.outputPath || '';
      }
      if (r.success) ok += 1;
      else fail += 1;
    }

    // Reconcile entries that produced no result row:
    //  - directories that were expanded into child file results -> drop (children shown)
    //  - plain files that came back empty -> mark failed
    for (const p of inputs) {
      const f = state.files.get(p);
      if (!f || f.status !== 'working' || covered.has(p)) continue;
      if (f.isDir) {
        state.files.delete(p);
      } else {
        f.status = 'err';
        f.message = '没有可解密的文件';
        fail += 1;
      }
    }

    notify(`完成：${ok} 成功 / ${fail} 失败`, fail > 0 && ok === 0 ? 'err' : 'ok');
  }

  // -------------------------------------------------------------------------
  // Pickers
  // -------------------------------------------------------------------------
  function pickerStartDir() {
    if (state.outputDir) return state.outputDir;
    return null;
  }

  btnFiles.addEventListener('click', async () => {
    try {
      const files = await invoke('pick_files', { defaultPath: pickerStartDir() });
      if (files && files.length) await addPaths(files);
    } catch (e) {
      notify('选择文件失败：' + e, 'err');
    }
  });

  btnFolder.addEventListener('click', async () => {
    try {
      const dir = await invoke('pick_folder', { defaultPath: pickerStartDir() });
      if (dir) await addPaths([dir]);
    } catch (e) {
      notify('选择文件夹失败：' + e, 'err');
    }
  });

  btnOutput.addEventListener('click', async () => {
    try {
      const dir = await invoke('pick_folder', { defaultPath: state.outputDir || null });
      if (dir) {
        state.outputDir = dir;
        inputOutput.value = dir;
      }
    } catch (e) {
      notify('选择输出目录失败：' + e, 'err');
    }
  });

  // -------------------------------------------------------------------------
  // Options & inputs
  // -------------------------------------------------------------------------
  inputOutput.addEventListener('input', () => { state.outputDir = inputOutput.value; });
  inputEkey.addEventListener('input', () => { state.ekey = inputEkey.value; });
  autoFetch.addEventListener('change', () => { state.autoFetch = autoFetch.checked; });

  btnToggleEkey.addEventListener('click', () => {
    state.showEkey = !state.showEkey;
    inputEkey.type = state.showEkey ? 'text' : 'password';
    btnToggleEkey.textContent = state.showEkey ? '隐藏' : '显示';
  });

  btnClear.addEventListener('click', clearAll);
  btnDecrypt.addEventListener('click', decrypt);
  modal.addEventListener('click', (e) => { if (e.target === modal) closeModal(); });
  modalClose.addEventListener('click', closeModal);

  // -------------------------------------------------------------------------
  // External links: Tauri webviews cannot open the system browser on their
  // own, so route clicks through the opener plugin (falls back to
  // window.open when not running inside Tauri, e.g. plain browser preview).
  // -------------------------------------------------------------------------
  function openExternal(url) {
    invoke('plugin:opener|open_url', { url })
      .catch(() => {
        if (!TAURI) window.open(url, '_blank', 'noopener');
      });
  }

  document.querySelectorAll('a[href^="http"]').forEach((a) => {
    a.addEventListener('click', (e) => {
      e.preventDefault();
      openExternal(a.href);
    });
  });

  // -------------------------------------------------------------------------
  // Drag & drop: Tauri delivers native file drops to the webview as the
  // `tauri://drag-drop` event, surfaced to JS via `onDragDropEvent`.
  // -------------------------------------------------------------------------
  ['dragenter', 'dragover'].forEach((evt) =>
    dropzone.addEventListener(evt, (e) => {
      e.preventDefault();
      dropzone.classList.add('over');
    })
  );
  ['dragleave', 'drop'].forEach((evt) =>
    dropzone.addEventListener(evt, (e) => {
      e.preventDefault();
      dropzone.classList.remove('over');
    })
  );

  // -------------------------------------------------------------------------
  // Rust 端进度事件（tauri v2 `emit` → 前端 `listen`）
  // -------------------------------------------------------------------------
  let listening = false;
  function listenProgressEvents() {
    const evt = TAURI && TAURI.event;
    if (!evt || typeof evt.listen !== 'function' || listening) return false;
    listening = true;
    const safe = (name, cb) => evt.listen(name, cb).catch(() => {});
    safe('decrypt-started', (e) => {
      if (!progress.hidden) updateProgress(0, Number(e.payload) || 0);
    });
    safe('decrypt-progress', (e) => {
      if (!progress.hidden) {
        const done = Number(e.payload) || 0;
        updateProgress(done, Math.max(done, progressTotal));
      }
    });
    return true;
  }

  async function attachDropListener() {
    const webview = (TAURI && TAURI.webview && TAURI.webview.getCurrentWebview)
      ? TAURI.webview.getCurrentWebview()
      : null;
    if (!webview || typeof webview.onDragDropEvent !== 'function') {
      console.warn('拖放事件 API 不可用（可能非 Tauri 环境）');
      return;
    }
    try {
      await webview.onDragDropEvent(async (event) => {
        const payload = event && event.payload;
        if (!payload || payload.type !== 'drop') return;
        const paths = payload.paths;
        if (Array.isArray(paths) && paths.length) await addPaths(paths);
      });
    } catch (e) {
      console.warn('拖放监听不可用：', e);
    }
  }

  // -------------------------------------------------------------------------
  // Startup
  // -------------------------------------------------------------------------
  async function loadPersisted() {
    try {
      state.outputDir = localStorage.getItem('outputDir') || '';
      inputOutput.value = state.outputDir;
      state.ekey = localStorage.getItem('ekey') || '';
      inputEkey.value = state.ekey;
      state.autoFetch = localStorage.getItem('autoFetch') === '1';
      autoFetch.checked = state.autoFetch;
    } catch (e) { /* storage may be disabled; ignore */ }
  }

  function persist() {
    try {
      localStorage.setItem('outputDir', state.outputDir);
      localStorage.setItem('ekey', state.ekey);
      localStorage.setItem('autoFetch', state.autoFetch ? '1' : '0');
    } catch (e) { /* ignore */ }
  }
  setInterval(persist, 1500);
  window.addEventListener('beforeunload', persist);

  async function checkCredentials() {
    try {
      const cred = await invoke('check_credentials');
      if (!cred) return;
      if (cred.found) {
        credNote.textContent = `已检测到本机 QQ 音乐凭据（uin：${cred.uin}），可自动获取 EKey`;
        credNote.className = 'cred-note ok';
      } else {
        credNote.textContent = '未检测到 QQ 音乐凭据：' + (cred.reason || '未知原因') +
          '。自动获取可能失败，可勾选后手动输入 EKey。';
        credNote.className = 'cred-note warn';
      }
      credNote.hidden = false;
    } catch (e) {
      credNote.textContent = '凭据检查失败：' + e;
      credNote.className = 'cred-note warn';
      credNote.hidden = false;
    }
  }

  (async function init() {
    await loadPersisted();
    await attachDropListener();
    await checkCredentials();
    listenProgressEvents();
    render();
  })();
})();