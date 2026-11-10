const { invoke } = window.__TAURI__.core;
const dialog = window.__TAURI__.dialog;
const { listen } = window.__TAURI__.event;

const IMAGE_EXTS = ["png", "jpg", "jpeg", "webp", "bmp", "gif", "ico", "tif", "tiff"];
const $ = (id) => document.getElementById(id);

const state = {
  apps: [],
  icons: new Map(),      // id -> data url（当前图标）
  selected: new Set(),
  anchor: null,
  filter: "all",
  query: "",
  image: null,           // 待应用的图片路径
  shape: "squircle",
  matchShape: "original",
  matches: [],
  settings: null,
};

// ------------------------------------------------------------ utils
function toast(msg, isErr = false, ms = 3800) {
  const el = document.createElement("div");
  el.className = "toast" + (isErr ? " err" : "");
  el.textContent = msg;
  $("toasts").appendChild(el);
  setTimeout(() => el.remove(), ms);
}

function reportToast(r, verb = t("report.changed")) {
  if (r.ok.length)
    toast(t("report.line", { verb, n: r.ok.length, names: r.ok.slice(0, 4).join(t("report.sep")), more: r.ok.length > 4 ? t("report.more") : "" }));
  if (r.failed.length) {
    toast(r.failed.slice(0, 3).map((f) => `${f.name}${t("colon")}${f.msg}`).join("\n"), true, 8000);
  }
}

async function busy(btn, fn) {
  btn.disabled = true;
  btn.classList.add("busy");
  try { return await fn(); }
  catch (e) { toast(String(e), true, 7000); }
  finally { btn.disabled = false; btn.classList.remove("busy"); }
}

const extOf = (p) => (p.split(".").pop() || "").toLowerCase();
const isImage = (p) => IMAGE_EXTS.includes(extOf(p)) && !/[\\/]$/.test(p);
const basename = (p) => p.split(/[\\/]/).pop();

function img(src, alt = "") {
  const i = document.createElement("img");
  i.src = src;
  i.alt = alt;
  i.draggable = false;
  return i;
}

function setSegmented(el, value) {
  el.querySelectorAll("button").forEach((b) => b.classList.toggle("active", b.dataset.shape === value));
}

// ------------------------------------------------------------ apps & icons
async function loadApps() {
  $("count").textContent = t("count.scanning");
  try {
    state.apps = await invoke("list_apps");
  } catch (e) {
    toast(t("scan.failed") + e, true);
    state.apps = [];
  }
  const ids = new Set(state.apps.map((a) => a.id));
  for (const id of [...state.selected]) if (!ids.has(id)) state.selected.delete(id);
  renderGrid();
  renderPanel();
  loadIcons(state.apps.map((a) => a.id));
}

async function loadIcons(ids, force = false) {
  const todo = force ? ids : ids.filter((id) => !state.icons.has(id));
  for (let i = 0; i < todo.length; i += 24) {
    const chunk = todo.slice(i, i + 24);
    try {
      const got = await invoke("app_icons", { ids: chunk, size: 128 });
      for (const [id, url] of Object.entries(got)) {
        state.icons.set(id, url);
        const el = document.querySelector(`.tile[data-id="${id}"] .ic`);
        if (el) el.replaceWith(Object.assign(img(url), { className: "ic" }));
      }
      if (chunk.some((id) => state.selected.has(id))) renderPanel();
    } catch (e) {
      console.warn(e);
    }
  }
}

function matchingApps() {
  const q = state.query.trim().toLowerCase();
  return state.apps.filter((a) => {
    if (state.filter === "custom" && !a.customized) return false;
    if (!q) return true;
    return a.name.toLowerCase().includes(q) || a.path.toLowerCase().includes(q);
  });
}

// 系统保护的应用（改不了）单独收进页面底部的一个可折叠分组
const GROUP_KEY = "protectedGroupOpen";
function groupOpen() {
  if (state.query.trim()) return true; // 搜索时自动展开
  try { return localStorage.getItem(GROUP_KEY) === "1"; } catch { return false; }
}
function setGroupOpen(v) {
  try { localStorage.setItem(GROUP_KEY, v ? "1" : "0"); } catch {}
}

/** 页面上按顺序显示（可点选）的应用：可修改的在前，系统应用分组展开时排在后面 */
function visibleApps() {
  const all = matchingApps();
  const editable = all.filter((a) => !a.protected);
  return groupOpen() ? editable.concat(all.filter((a) => a.protected)) : editable;
}

function makeTile(a) {
  const tile = document.createElement("button");
  tile.className = "tile" + (state.selected.has(a.id) ? " selected" : "") + (a.protected ? " protected" : "");
  tile.dataset.id = a.id;
  tile.title = a.protected ? `${a.name}\n${t("tile.protected")}` : `${a.name}\n${a.path}`;
  const icon = state.icons.get(a.id);
  if (icon) tile.appendChild(Object.assign(img(icon), { className: "ic" }));
  else tile.appendChild(Object.assign(document.createElement("div"), { className: "ph ic" }));
  const n = document.createElement("div");
  n.className = "name";
  n.textContent = a.name;
  tile.appendChild(n);
  if (a.customized) tile.appendChild(Object.assign(document.createElement("span"), { className: "dot" }));
  tile.addEventListener("click", (e) => onTileClick(e, a.id));
  return tile;
}

function renderGrid() {
  const grid = $("grid");
  const apps = matchingApps();
  const custom = state.apps.filter((a) => a.customized).length;
  $("count").textContent = t("count.summary", { n: state.apps.length, c: custom });
  grid.replaceChildren();
  if (!apps.length) {
    const d = document.createElement("div");
    d.className = "grid-empty";
    d.textContent = state.apps.length ? t("grid.nomatch") : t("grid.none");
    grid.appendChild(d);
    return;
  }
  const frag = document.createDocumentFragment();
  const editable = apps.filter((a) => !a.protected);
  const locked = apps.filter((a) => a.protected);
  editable.forEach((a) => frag.appendChild(makeTile(a)));
  if (locked.length) {
    const open = groupOpen();
    const head = document.createElement("button");
    head.className = "group-head" + (open ? " open" : "");
    head.setAttribute("aria-expanded", String(open));
    const chev = Object.assign(document.createElement("span"), { className: "chev", textContent: "›" });
    const title = Object.assign(document.createElement("span"), { textContent: t("group.protected", { n: locked.length }) });
    const hint = Object.assign(document.createElement("small"), { textContent: t("group.protected.hint") });
    head.append(chev, title, hint);
    head.addEventListener("click", () => {
      const next = !groupOpen();
      setGroupOpen(next);
      if (!next) locked.forEach((a) => state.selected.delete(a.id));
      renderGrid();
      renderPanel();
    });
    frag.appendChild(head);
    if (open) locked.forEach((a) => frag.appendChild(makeTile(a)));
  }
  grid.appendChild(frag);
}

function onTileClick(e, id) {
  const multi = e.metaKey || e.ctrlKey;
  if (e.shiftKey && state.anchor) {
    const list = visibleApps().map((a) => a.id);
    const [i, j] = [list.indexOf(state.anchor), list.indexOf(id)].sort((x, y) => x - y);
    if (i >= 0 && j >= 0) list.slice(i, j + 1).forEach((x) => state.selected.add(x));
  } else if (multi) {
    state.selected.has(id) ? state.selected.delete(id) : state.selected.add(id);
    state.anchor = id;
  } else {
    state.selected = new Set([id]);
    state.anchor = id;
  }
  document.querySelectorAll(".tile").forEach((el) => el.classList.toggle("selected", state.selected.has(el.dataset.id)));
  renderPanel();
}

function selectedApps() {
  return state.apps.filter((a) => state.selected.has(a.id));
}

// ------------------------------------------------------------ panel
async function renderPanel() {
  const apps = selectedApps();
  $("panel-empty").hidden = apps.length > 0;
  $("panel-body").hidden = apps.length === 0;
  if (!apps.length) return;

  const icons = $("target-icons");
  icons.replaceChildren(...apps.slice(0, 3).map((a) => img(state.icons.get(a.id) || "")));
  const single = apps.length === 1 ? apps[0] : null;
  $("target-name").textContent = single ? single.name : t("target.many", { n: apps.length });
  const pathBtn = $("target-path");
  pathBtn.textContent = single ? single.path : apps.map((a) => a.name).join(t("report.sep"));
  pathBtn.onclick = single ? () => invoke("reveal", { path: single.path }) : null;

  const badges = $("target-badges");
  badges.replaceChildren();
  const addBadge = (text, cls = "") => badges.appendChild(Object.assign(document.createElement("span"), { className: "badge " + cls, textContent: text }));
  const customized = apps.filter((a) => a.customized);
  if (customized.length) addBadge(single ? t("badge.custom") : t("badge.customMany", { n: customized.length }), "custom");
  if (apps.some((a) => a.protected)) addBadge(t("badge.protected"));
  if (single && state.settings?.platform === "windows") addBadge(t("badge.shortcuts", { n: single.files }));

  const cur = $("well-current");
  cur.replaceChildren();
  if (single && state.icons.get(single.id)) cur.appendChild(img(state.icons.get(single.id)));

  $("btn-restore").hidden = customized.length === 0;
  $("btn-restore").textContent = customized.length > 1 ? t("restore.many", { n: customized.length }) : t("restore.one");
  $("btn-apply").textContent = apps.length > 1 ? t("apply.many", { n: apps.length }) : t("apply.one");
  await renderPreview();
}

let previewSeq = 0;
async function renderPreview() {
  const well = $("well-new");
  $("btn-apply").disabled = !state.image || selectedApps().every((a) => a.protected);
  if (!state.image) {
    well.innerHTML = `<span class="drop-text">${t("well.drop")}</span>`;
    $("new-caption").textContent = t("well.new");
    return;
  }
  const seq = ++previewSeq;
  try {
    const url = await invoke("preview", { path: state.image, shape: state.shape });
    if (seq !== previewSeq) return;
    well.replaceChildren(img(url));
    $("new-caption").textContent = basename(state.image);
  } catch (e) {
    toast(String(e), true);
    state.image = null;
    renderPreview();
  }
}

async function pickImage() {
  const p = await dialog.open({
    multiple: false,
    directory: false,
    filters: [{ name: t("filter.images"), extensions: IMAGE_EXTS }],
  });
  if (p) setImage(Array.isArray(p) ? p[0] : p);
}

function setImage(path) {
  state.image = path;
  renderPreview();
}

async function applySelected() {
  const ids = [...state.selected];
  await busy($("btn-apply"), async () => {
    const r = await invoke("apply_icon", { ids, image: state.image, shape: state.shape });
    reportToast(r);
    if (r.ok.length) {
      state.image = null;
      await afterChange(ids);
    }
  });
}

async function restoreSelected() {
  const ids = selectedApps().filter((a) => a.customized).map((a) => a.id);
  const ok = await dialog.ask(t("restore.confirm", { n: ids.length }), { title: t("restore.title"), kind: "warning" });
  if (!ok) return;
  await busy($("btn-restore"), async () => {
    const r = await invoke("restore_icon", { ids });
    reportToast(r, t("report.restored"));
    await afterChange(ids);
  });
}

async function afterChange(ids) {
  ids.forEach((id) => state.icons.delete(id));
  await loadApps();
  // 系统需要一点时间更新图标缓存
  setTimeout(() => loadIcons(ids, true), 1200);
}

// ------------------------------------------------------------ batch (folder)
async function openFolderMatch(folder) {
  if (!folder) {
    folder = await dialog.open({ directory: true, multiple: false, title: t("match.pick") });
    if (!folder) return;
  }
  let matches;
  try {
    matches = await invoke("match_folder_cmd", { folder });
  } catch (e) {
    return toast(String(e), true);
  }
  if (!matches.length) {
    return toast(t("match.none"), true, 7000);
  }
  state.matches = matches;
  $("match-title").innerHTML = t("match.title");
  $("match-count").textContent = matches.length;
  $("match-source").textContent = folder;
  const list = $("match-list");
  list.replaceChildren();
  const protectedIds = new Set(state.apps.filter((a) => a.protected).map((a) => a.id));
  for (const [i, m] of matches.entries()) {
    const row = document.createElement("label");
    row.className = "match-row";
    const cb = Object.assign(document.createElement("input"), { type: "checkbox", checked: !protectedIds.has(m.id), disabled: protectedIds.has(m.id) });
    cb.dataset.i = i;
    const nm = document.createElement("div");
    nm.className = "nm";
    nm.textContent = m.name;
    const small = document.createElement("small");
    small.textContent = protectedIds.has(m.id) ? t("match.protected") : m.image_name;
    nm.appendChild(small);
    const newImg = img("");
    row.append(cb, img(state.icons.get(m.id) || ""), nm, Object.assign(document.createElement("span"), { className: "arrow", textContent: "→" }), newImg);
    list.appendChild(row);
    invoke("preview", { path: m.image, shape: state.matchShape }).then((u) => (newImg.src = u)).catch(() => {});
  }
  setSegmented($("match-shape"), state.matchShape);
  $("dlg-match").showModal();
}

async function refreshMatchPreviews() {
  const imgs = [...$("match-list").querySelectorAll(".match-row img:last-child")];
  state.matches.forEach((m, i) => {
    invoke("preview", { path: m.image, shape: state.matchShape }).then((u) => (imgs[i].src = u)).catch(() => {});
  });
}

async function applyMatches() {
  const pairs = [...$("match-list").querySelectorAll("input:checked")].map((cb) => {
    const m = state.matches[+cb.dataset.i];
    return { id: m.id, image: m.image };
  });
  if (!pairs.length) return;
  const btn = $("btn-match-apply");
  await busy(btn, async () => {
    const r = await invoke("apply_matches", { pairs, shape: state.matchShape });
    reportToast(r);
    $("dlg-match").close();
    await afterChange(pairs.map((p) => p.id));
  });
}

// ------------------------------------------------------------ themes
async function openThemes() {
  await renderThemes();
  $("dlg-themes").showModal();
}

async function renderThemes() {
  const list = $("theme-list");
  list.replaceChildren();
  const themes = await invoke("list_themes");
  if (!themes.length) {
    list.appendChild(Object.assign(document.createElement("div"), { className: "empty-note", textContent: t("themes.empty") }));
    return;
  }
  for (const theme of themes) {
    const row = document.createElement("div");
    row.className = "theme";
    const pv = document.createElement("div");
    pv.className = "theme-previews";
    theme.previews.forEach((u) => pv.appendChild(img(u)));
    const info = document.createElement("div");
    info.className = "theme-info";
    info.innerHTML = "<b></b><small></small>";
    info.querySelector("b").textContent = theme.name;
    info.querySelector("small").textContent = t("themes.count", { n: theme.count });
    const apply = Object.assign(document.createElement("button"), { type: "button", className: "btn primary", textContent: t("themes.apply") });
    apply.onclick = () => {
      $("dlg-themes").close();
      state.matchShape = "original"; // 主题里存的是处理好的图标
      openFolderMatch(theme.path);
    };
    const open = Object.assign(document.createElement("button"), { type: "button", className: "btn ghost", textContent: t("themes.open") });
    open.onclick = () => invoke("reveal", { path: theme.path });
    const del = Object.assign(document.createElement("button"), { type: "button", className: "btn ghost danger", textContent: t("themes.delete") });
    del.onclick = async () => {
      if (!(await dialog.ask(t("themes.delete.confirm", { name: theme.name }), { title: t("themes.delete.title"), kind: "warning" }))) return;
      await invoke("delete_theme", { name: theme.name });
      renderThemes();
    };
    row.append(pv, info, apply, open, del);
    list.appendChild(row);
  }
}

async function saveTheme() {
  const name = $("theme-name").value.trim();
  if (!name) return $("theme-name").focus();
  await busy($("btn-theme-save"), async () => {
    const n = await invoke("save_theme", { name });
    if (!n) toast(t("themes.nothing"), true);
    else toast(t("themes.saved", { name, n }));
    $("theme-name").value = "";
    await renderThemes();
  });
}

// ------------------------------------------------------------ settings
async function loadSettings() {
  state.settings = await invoke("get_settings");
  state.shape = await invoke("default_shape");
  setSegmented($("shape"), state.shape);
  refreshTexts();
}

/** 按当前语言重新填写所有文字 */
function refreshTexts() {
  const s = state.settings;
  applyI18n(s.lang);
  const mac = s.platform === "macos";
  $("s-refresh-label").textContent = t(mac ? "settings.refresh.mac" : "settings.refresh.win");
  $("btn-hard-refresh").textContent = t(mac ? "settings.hard.mac" : "settings.hard.win");
  $("fineprint").textContent = t(mac ? "fineprint.mac" : "fineprint.win");
  $("u-current").textContent = t("update.current", { v: s.version });
}

function openSettings() {
  const s = state.settings;
  $("s-auto").checked = s.auto_reapply;
  $("s-interval").value = s.interval_min;
  $("s-refresh").checked = s.refresh_shell;
  $("s-tray").checked = s.keep_in_tray;
  $("s-autostart").checked = s.autostart;
  $("s-language").value = s.language || "system";
  $("u-current").textContent = t("update.current", { v: s.version });
  if (!state.update?.available) { $("u-status").textContent = ""; $("btn-u-install").hidden = true; }
  $("dlg-settings").showModal();
}

async function saveSettings() {
  const s = {
    ...state.settings,
    auto_reapply: $("s-auto").checked,
    interval_min: Math.max(1, parseInt($("s-interval").value, 10) || 10),
    refresh_shell: $("s-refresh").checked,
    keep_in_tray: $("s-tray").checked,
    autostart: $("s-autostart").checked,
    language: $("s-language").value,
  };
  try {
    await invoke("set_settings", { settings: s });
    const prevLang = state.settings.lang;
    state.settings = await invoke("get_settings");
    if (state.settings.lang !== prevLang) {
      refreshTexts();
      renderGrid();
      renderPanel();
      if (state.update?.available) checkUpdate(false);
    }
    toast(t("settings.saved"));
  } catch (e) {
    toast(t("settings.failed") + e, true);
  }
}

// ------------------------------------------------------------ updates
async function checkUpdate(silent = false) {
  const status = $("u-status"), btn = $("btn-u-install");
  if (!silent) { status.textContent = t("update.checking"); btn.hidden = true; }
  try {
    const u = await invoke("check_update");
    state.update = u;
    $("btn-settings").classList.toggle("has-update", u.available);
    if (u.available) {
      status.textContent = t("update.available", { v: u.latest }) + (u.notes ? "\n\n" + u.notes : "");
      btn.textContent = t("update.install", { v: u.latest });
      btn.hidden = false;
      if (silent) toast(t("update.toast", { v: u.latest }), false, 6000);
    } else if (!silent) {
      status.textContent = t("update.latest");
    }
  } catch (e) {
    if (!silent) status.textContent = t("update.failed") + e;
  }
}

async function installUpdate() {
  const btn = $("btn-u-install");
  $("u-status").textContent = t("update.installing");
  await busy(btn, async () => {
    const msg = await invoke("install_update");
    if (msg) { $("u-status").textContent = msg; toast(msg, false, 9000); }
  });
}

// ------------------------------------------------------------ drag & drop
function setupDragDrop() {
  const overlay = $("drop-overlay");
  window.__TAURI__.webview.getCurrentWebview().onDragDropEvent((ev) => {
    const p = ev.payload;
    if (p.type === "over" || p.type === "enter") overlay.classList.add("show");
    else if (p.type === "leave") overlay.classList.remove("show");
    else if (p.type === "drop") {
      overlay.classList.remove("show");
      const path = p.paths?.[0];
      if (!path) return;
      if (isImage(path)) {
        if (!state.selected.size) return toast(t("drop.selectFirst"), true);
        setImage(path);
      } else {
        openFolderMatch(path); // 文件夹 → 批量匹配
      }
    }
  });
}

// ------------------------------------------------------------ wire up
function bind() {
  $("search").addEventListener("input", (e) => { state.query = e.target.value; renderGrid(); });
  $("filters").addEventListener("click", (e) => {
    const b = e.target.closest(".chip");
    if (!b) return;
    state.filter = b.dataset.filter;
    document.querySelectorAll(".chip").forEach((c) => c.classList.toggle("active", c === b));
    renderGrid();
  });
  $("well-new").addEventListener("click", pickImage);
  $("well-new").addEventListener("keydown", (e) => (e.key === "Enter" || e.key === " ") && pickImage());
  $("shape").addEventListener("click", (e) => {
    const b = e.target.closest("button");
    if (!b) return;
    state.shape = b.dataset.shape;
    setSegmented($("shape"), state.shape);
    renderPreview();
  });
  $("match-shape").addEventListener("click", (e) => {
    const b = e.target.closest("button");
    if (!b) return;
    state.matchShape = b.dataset.shape;
    setSegmented($("match-shape"), state.matchShape);
    refreshMatchPreviews();
  });
  $("btn-apply").addEventListener("click", applySelected);
  $("btn-restore").addEventListener("click", restoreSelected);
  $("btn-folder").addEventListener("click", () => openFolderMatch());
  $("btn-themes").addEventListener("click", openThemes);
  $("btn-theme-save").addEventListener("click", saveTheme);
  $("theme-name").addEventListener("keydown", (e) => { if (e.key === "Enter") { e.preventDefault(); saveTheme(); } });
  $("btn-check").addEventListener("click", (e) =>
    busy(e.currentTarget, async () => {
      const r = await invoke("reapply_now");
      if (!r.ok.length && !r.failed.length) toast(t("check.allgood"));
      else { reportToast(r, t("report.reapplied")); await afterChange(r.ok.length ? state.apps.map((a) => a.id) : []); }
    })
  );
  $("btn-settings").addEventListener("click", openSettings);
  $("dlg-settings").addEventListener("close", () => { if ($("dlg-settings").returnValue === "save") saveSettings(); });
  $("dlg-match").addEventListener("close", () => {});
  $("btn-match-apply").addEventListener("click", (e) => { e.preventDefault(); applyMatches(); });
  $("btn-hard-refresh").addEventListener("click", (e) => busy(e.currentTarget, () => invoke("refresh_shell", { hard: true })));
  $("btn-u-check").addEventListener("click", () => checkUpdate(false));
  $("btn-u-install").addEventListener("click", installUpdate);
  $("btn-data-dir").addEventListener("click", () => invoke("reveal", { path: state.settings.data_dir }));
  document.addEventListener("keydown", (e) => {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "f") { e.preventDefault(); $("search").focus(); }
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "a" && document.activeElement.tagName !== "INPUT") {
      e.preventDefault();
      visibleApps().forEach((a) => state.selected.add(a.id));
      renderGrid();
      renderPanel();
    }
  });
  listen("auto-reapplied", (ev) => {
    const r = ev.payload;
    if (r.ok.length) { reportToast(r, t("report.auto")); afterChange(state.apps.map((a) => a.id)); }
    else if (r.failed.length) reportToast(r);
  });
  window.addEventListener("focus", () => {
    // 回到窗口时刷新一下列表（可能刚装/卸了应用）
    if (Date.now() - (window._lastScan || 0) > 60_000) { window._lastScan = Date.now(); loadApps(); }
  });
}

(async function init() {
  applyI18n();
  bind();
  setupDragDrop();
  await loadSettings();
  window._lastScan = Date.now();
  await loadApps();
  setTimeout(() => checkUpdate(true), 3000);
})();
