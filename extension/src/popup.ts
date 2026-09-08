/** 点插件图标弹出的功能菜单：开关功能、切显示模式、触发整页翻译 */
import { DEFAULT_SETTINGS, type DisplayMode, type Health, type PageStatus, type Settings } from "./shared";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

let settings: Settings = DEFAULT_SETTINGS;
let host = "";
let tabId: number | undefined;
let injectable = true;
let status: PageStatus = { translated: false, display: "original", blocks: 0, host: "" };

/** 给 content script 发消息；页面不支持注入（chrome:// 等）时返回 undefined */
async function toContent<T>(msg: unknown): Promise<T | undefined> {
  if (tabId === undefined) return undefined;
  try {
    return (await chrome.tabs.sendMessage(tabId, msg)) as T;
  } catch {
    return undefined;
  }
}

async function save(patch: Partial<Settings>) {
  settings = { ...settings, ...patch };
  await chrome.storage.sync.set(patch);
  render();
}

function siteDisabled(): boolean {
  return host !== "" && settings.disabledHosts.includes(host);
}

function render() {
  const off = siteDisabled();

  const main = $<HTMLButtonElement>("translate");
  main.textContent = status.translated ? "恢复原文" : "翻译此页";
  main.classList.toggle("secondary", status.translated);
  main.disabled = !injectable;

  $("modes").hidden = !status.translated;
  for (const m of ["bilingual", "translation_only"] as DisplayMode[]) {
    $(`mode-${m}`).classList.toggle("active", status.display === m);
  }

  const hoverOn = settings.hoverModifier !== "none";
  const hoverTile = $<HTMLButtonElement>("tile-hover");
  hoverTile.classList.toggle("on", hoverOn && !off);
  hoverTile.disabled = off;
  const keyName = { alt: "Alt", ctrl: "Ctrl", shift: "Shift", none: "Alt" }[settings.hoverModifier];
  // 这个格子管的是「自动触发」：关掉后快捷键照样能翻当前段，所以文案要写清楚
  $("hoverDesc").innerHTML = hoverOn
    ? `按住 <b class="key" id="hoverKey">${keyName}</b> 停留自动翻`
    : `自动触发已关；仍可按 ${settings.keyBlock}`;
  // innerHTML 重建后要重新绑事件
  document.getElementById("hoverKey")?.addEventListener("click", (e) => {
    e.stopPropagation(); // 别把点击传给格子（那是开关）
    const order = ["alt", "ctrl", "shift"] as const;
    const next = order[(order.indexOf(settings.hoverModifier as (typeof order)[number]) + 1) % order.length];
    void save({ hoverModifier: next });
  });

  const iconTile = $<HTMLButtonElement>("tile-icon");
  iconTile.classList.toggle("on", settings.selectionIcon && !off);
  iconTile.disabled = off;
  $("iconDesc").textContent = settings.selectionIcon
    ? "选中后弹「译」按钮"
    : `图标已关；仍可按 ${settings.keySelection}`;

  // 拿不到域名（当前页不能注入，或没有 activeTab 授权）时没有站点可停用，整行藏起来
  $("siteRow").hidden = host === "";
  $<HTMLInputElement>("siteOff").checked = off;
  $("siteLabel").textContent = `在 ${host} 停用`;
  $("siteHint").hidden = !off;

  // 底部显示实际生效的三个快捷键（页面自己监听的那套，可在设置里改）
  $("keys").innerHTML = [
    `<kbd>${settings.keyBlock}</kbd> 本段`,
    `<kbd>${settings.keySelection}</kbd> 划词`,
    `<kbd>${settings.keyFullPage}</kbd> 整页`,
  ].join(" · ");
}

async function init() {
  settings = { ...DEFAULT_SETTINGS, ...(await chrome.storage.sync.get(DEFAULT_SETTINGS)) } as Settings;
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  tabId = tab?.id;
  // tab.url 要 tabs 权限或 activeTab 授权才读得到，拿不到就等页面自己报（见下面的 status）
  try {
    host = tab?.url ? new URL(tab.url).hostname : "";
  } catch {
    host = "";
  }

  const reply = await chrome.runtime.sendMessage({ kind: "health" });
  const conn = $("conn");
  if (reply?.ok && reply.health) {
    const h = reply.health as Health;
    conn.textContent = `桌面端已连接 · v${h.version}`;
    conn.className = "conn ok";
    $("serviceName").textContent = h.model ? `${h.model}` : h.backend ?? "—";
    $("serviceName").title = `后端 ${h.backend ?? "?"} · 模型 ${h.model ?? "?"}`;
  } else {
    conn.textContent = "桌面端未运行";
    conn.className = "conn bad";
    $("downHint").hidden = false;
    $("serviceName").textContent = "未连接";
  }

  const s = await toContent<PageStatus>({ kind: "status" });
  if (s) {
    status = s;
    if (s.host) host = s.host;
  } else {
    injectable = false;
    $("pageHint").hidden = false;
  }
  render();
}

document.addEventListener("DOMContentLoaded", () => {
  void init();

  $("translate").addEventListener("click", async () => {
    if (status.translated) {
      await toContent({ kind: "restore_page" });
      status = { ...status, translated: false, display: "original" };
      render();
    } else {
      await toContent({ kind: "translate_page" });
      window.close(); // 让开位置看翻译进度
    }
  });

  for (const m of ["bilingual", "translation_only"] as DisplayMode[]) {
    $(`mode-${m}`).addEventListener("click", async () => {
      await toContent({ kind: "set_display", mode: m });
      status = { ...status, display: m };
      await save({ defaultDisplay: m });
    });
  }

  // 点格子切开关；描述里的键名单独可点，循环换修饰键（见 render 里的绑定）
  $("tile-hover").addEventListener("click", () => {
    void save({ hoverModifier: settings.hoverModifier === "none" ? "alt" : "none" });
  });

  $("tile-icon").addEventListener("click", () => {
    void save({ selectionIcon: !settings.selectionIcon });
  });

  $("siteOff").addEventListener("change", (e) => {
    const off = (e.target as HTMLInputElement).checked;
    const list = settings.disabledHosts.filter((h) => h !== host);
    void save({ disabledHosts: off && host ? [...list, host] : list });
  });

  $("options").addEventListener("click", () => chrome.runtime.openOptionsPage());
});
