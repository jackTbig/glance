/** 设置页：浏览器里的行为 + 直连模式下的接口配置（API Key 存本地，不参与同步） */
import { API_KEY_KEY, DEFAULT_SETTINGS, parseHotkey, type Backend, type Settings } from "./shared";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

async function load() {
  const s = { ...DEFAULT_SETTINGS, ...(await chrome.storage.sync.get(DEFAULT_SETTINGS)) } as Settings;
  const local = await chrome.storage.local.get(API_KEY_KEY);
  const key = local[API_KEY_KEY];
  $<HTMLInputElement>("port").value = String(s.port);
  $<HTMLSelectElement>("hoverModifier").value = s.hoverModifier;
  $<HTMLInputElement>("hoverDelayMs").value = String(s.hoverDelayMs);
  $<HTMLInputElement>("selectionIcon").checked = s.selectionIcon;
  $<HTMLSelectElement>("defaultDisplay").value = s.defaultDisplay;
  $<HTMLTextAreaElement>("disabledHosts").value = s.disabledHosts.join("\n");
  $<HTMLInputElement>("keyFullPage").value = s.keyFullPage;
  $<HTMLInputElement>("keySelection").value = s.keySelection;
  $<HTMLSelectElement>("backend").value = s.backend;
  $<HTMLInputElement>("apiBase").value = s.apiBase;
  $<HTMLInputElement>("apiModel").value = s.apiModel;
  $<HTMLInputElement>("apiKey").value = typeof key === "string" ? key : "";
  syncPreset();
  toggleDirect();
}

/** 写法不合法就退回默认值，免得用户把自己锁死 */
function readKey(id: string, fallback: string): string {
  const v = $<HTMLInputElement>(id).value.trim();
  return parseHotkey(v) ? v : fallback;
}

function toggleDirect() {
  $<HTMLElement>("directBox").hidden = $<HTMLSelectElement>("backend").value !== "direct";
}

/** 接口地址对上某个预设就把下拉选中，对不上就是「自定义」 */
function syncPreset() {
  const base = $<HTMLInputElement>("apiBase").value.trim();
  const sel = $<HTMLSelectElement>("apiPreset");
  let hit = "";
  for (const o of Array.from(sel.options)) {
    if (o.value && o.value === base) hit = o.value;
  }
  sel.value = hit;
}

/** 接口地址 → Chrome 的站点权限模式，如 https://api.deepseek.com/* */
function originPattern(base: string): string | null {
  try {
    const u = new URL(base);
    if (u.protocol !== "http:" && u.protocol !== "https:") return null;
    return `${u.protocol}//${u.host}/*`;
  } catch {
    return null;
  }
}

/**
 * 申请访问该接口地址的权限。跨域请求要靠 host_permissions 放行，
 * 不然浏览器会拿 CORS 挡下来。已经授过权的这里返回 true。
 * 必须在用户手势里调用，所以直接调 request（已授权时它自己返回 true）。
 */
async function askPermission(base: string): Promise<boolean> {
  const pattern = originPattern(base);
  if (!pattern) return false;
  try {
    return await chrome.permissions.request({ origins: [pattern] });
  } catch {
    return false;
  }
}

async function save() {
  const backend = $<HTMLSelectElement>("backend").value as Backend;
  const apiBase = $<HTMLInputElement>("apiBase").value.trim();
  if (backend === "direct") {
    if (!apiBase) {
      status("直连模式要先填接口地址");
      return;
    }
    if (!(await askPermission(apiBase))) {
      status("没拿到该接口地址的访问权限，直连用不了。");
      return;
    }
  }
  const s: Settings = {
    port: Math.max(1024, Math.min(65535, Number($<HTMLInputElement>("port").value) || DEFAULT_SETTINGS.port)),
    hoverModifier: $<HTMLSelectElement>("hoverModifier").value as Settings["hoverModifier"],
    hoverDelayMs: Math.max(80, Math.min(3000, Number($<HTMLInputElement>("hoverDelayMs").value) || 300)),
    selectionIcon: $<HTMLInputElement>("selectionIcon").checked,
    defaultDisplay: $<HTMLSelectElement>("defaultDisplay").value as Settings["defaultDisplay"],
    disabledHosts: $<HTMLTextAreaElement>("disabledHosts").value.split("\n").map((x) => x.trim()).filter(Boolean),
    keyFullPage: readKey("keyFullPage", DEFAULT_SETTINGS.keyFullPage),
    keySelection: readKey("keySelection", DEFAULT_SETTINGS.keySelection),
    backend,
    apiBase,
    apiModel: $<HTMLInputElement>("apiModel").value.trim(),
  };
  await chrome.storage.sync.set(s);
  // Key 单独存 local：sync 会同步到 Google 的服务器
  await chrome.storage.local.set({ [API_KEY_KEY]: $<HTMLInputElement>("apiKey").value.trim() });
  await load(); // 非法写法被退回默认值时，界面上要能看出来
  status("已保存，立即生效。");
}

function status(msg: string) {
  $("status").textContent = msg;
  setTimeout(() => ($("status").textContent = ""), 6000);
}

/**
 * 把 service worker 的回包错误翻译成人能看懂的话。
 *
 * 「未知消息」只有一个来处：正在跑的 service worker 不认识这条消息——
 * 也就是**插件文件换了、但扩展没重新加载**（解压加载的扩展，service worker
 * 用的是启动时那份代码，设置页却是从磁盘现读的，两边对不上）。
 * 这种情况必须让用户知道去点「重新加载」，光说「未知消息」没法自查。
 */
function replyError(r: { error?: string } | undefined, fallback: string): string {
  const e = r?.error ?? "";
  if (e === "未知消息") {
    return "插件版本对不上：请到 chrome://extensions 点一下「重新加载」，再打开这个设置页";
  }
  return e || fallback;
}

/** 是不是「扩展没重新加载」这种版本对不上的情况 */
function staleWorker(r: { error?: string } | undefined): boolean {
  return r?.error === "未知消息";
}

async function checkHealth() {
  const r = await chrome.runtime.sendMessage({ kind: "health" });
  if (r?.ok && r.health) {
    const h = r.health as { version: string; backend?: string; model?: string; concurrency: number; batch_size: number };
    if (h.backend === "direct") {
      status(`当前是直连模式：${h.model || "没填模型"}。点「测试直连接口」验一下。`);
    } else {
      status(`桌面端已连接：版本 ${h.version}，并发 ${h.concurrency}，每请求 ${h.batch_size} 段`);
    }
  } else {
    status(replyError(r, "连不上桌面端"));
  }
}

/** 用当前填的地址/Key/模型打一次接口，验 Key 与模型能不能真用 */
async function testDirect() {
  const base = $<HTMLInputElement>("apiBase").value.trim();
  const key = $<HTMLInputElement>("apiKey").value.trim();
  const model = $<HTMLInputElement>("apiModel").value.trim();
  if (!base) {
    status("先填接口地址");
    return;
  }
  if (!key) {
    status("先填 API Key");
    return;
  }
  if (!(await askPermission(base))) {
    status("没拿到该接口地址的访问权限");
    return;
  }
  status("正在测试…");
  const r = await chrome.runtime.sendMessage({ kind: "direct_test", base, key, model });
  if (!r?.ok) {
    status(replyError(r, "测试失败"));
    return;
  }
  status(`连接正常${r.reply ? `：${r.reply}` : ""}`);
}

/* ---------------- 模型列表（照桌面端 models.rs 的那一套） ---------------- */

/** 列表归属「地址|Key」：改了任一项，旧列表自动作废、下次重新拉 */
let modelCache: { key: string; models: string[] } | null = null;

function listKey(base: string, key: string): string {
  return `${base.trim()}|${key.trim()}`;
}

function fillModels(models: string[]) {
  const dl = $<HTMLDataListElement>("modelList");
  dl.replaceChildren(
    ...models.map((m) => {
      const o = document.createElement("option");
      o.value = m;
      o.textContent = m; // 只设 value 的话 option 没有文本，datalist 里显示不出来
      return o;
    }),
  );
  $<HTMLButtonElement>("listModels").textContent = models.length > 0 ? `${models.length} 个模型` : "模型列表";
}

/** 拉模型列表；缓存命中就直接用。失败把原因写进提示行，不静默 */
async function fetchModels(force: boolean) {
  const base = $<HTMLInputElement>("apiBase").value.trim();
  const key = $<HTMLInputElement>("apiKey").value.trim();
  const k = listKey(base, key);
  if (!force && modelCache && modelCache.key === k) {
    fillModels(modelCache.models);
    return;
  }
  const btn = $<HTMLButtonElement>("listModels");
  btn.textContent = "拉取…";
  const r = await chrome.runtime.sendMessage({ kind: "list_models", base, key });
  btn.textContent = "模型列表";
  if (!r?.ok) {
    status(staleWorker(r) ? replyError(r, "") : `拉模型列表失败：${r?.error ?? "未知原因"}`);
    return;
  }
  const models = (r.models as string[]) ?? [];
  modelCache = { key: k, models };
  fillModels(models);
  status(`拉到 ${models.length} 个模型，点模型框右侧的箭头选`);
}

/** 点「模型列表」按钮：先申请访问权限（这里才有用户手势），再拉 */
async function listModelsFlow() {
  const base = $<HTMLInputElement>("apiBase").value.trim();
  if (!base) {
    status("先填接口地址");
    return;
  }
  if (!$<HTMLInputElement>("apiKey").value.trim()) {
    status("先填 API Key");
    return;
  }
  if (!(await askPermission(base))) {
    status("没拿到该接口地址的访问权限");
    return;
  }
  await fetchModels(true);
}

/**
 * 点开模型框就拉列表（桌面端是「点下拉的那一刻拉」）。
 * 但申请权限需要用户手势，focus 里申请不了——没授权过就先提示点按钮。
 */
async function onModelFocus() {
  const base = $<HTMLInputElement>("apiBase").value.trim();
  if (!base) return;
  const k = listKey(base, $<HTMLInputElement>("apiKey").value.trim());
  if (modelCache && modelCache.key === k) {
    fillModels(modelCache.models);
    return;
  }
  const pattern = originPattern(base);
  const granted = pattern ? await chrome.permissions.contains({ origins: [pattern] }) : false;
  if (!granted) {
    status("还没授权访问这个地址：点「模型列表」按钮授权后就能拉");
    return;
  }
  await fetchModels(false);
}

document.addEventListener("DOMContentLoaded", () => {
  void load();
  $("save").addEventListener("click", () => void save());
  $("check").addEventListener("click", () => void checkHealth());
  $("test").addEventListener("click", () => void testDirect());
  $("listModels").addEventListener("click", () => void listModelsFlow());
  $("apiModel").addEventListener("focus", () => void onModelFocus());
  $("backend").addEventListener("change", () => toggleDirect());
  $("apiPreset").addEventListener("change", () => {
    const v = $<HTMLSelectElement>("apiPreset").value;
    if (v) $<HTMLInputElement>("apiBase").value = v;
  });
  $("apiBase").addEventListener("input", () => syncPreset());
});
