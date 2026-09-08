/** 设置页：只管插件侧的行为，Key 与模型在桌面端配 */
import { DEFAULT_SETTINGS, parseHotkey, type Settings } from "./shared";

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

async function load() {
  const s = { ...DEFAULT_SETTINGS, ...(await chrome.storage.sync.get(DEFAULT_SETTINGS)) } as Settings;
  $<HTMLInputElement>("port").value = String(s.port);
  $<HTMLSelectElement>("hoverModifier").value = s.hoverModifier;
  $<HTMLInputElement>("hoverDelayMs").value = String(s.hoverDelayMs);
  $<HTMLInputElement>("selectionIcon").checked = s.selectionIcon;
  $<HTMLSelectElement>("defaultDisplay").value = s.defaultDisplay;
  $<HTMLTextAreaElement>("disabledHosts").value = s.disabledHosts.join("\n");
  $<HTMLInputElement>("keyFullPage").value = s.keyFullPage;
  $<HTMLInputElement>("keySelection").value = s.keySelection;
  $<HTMLInputElement>("keyBlock").value = s.keyBlock;
}

/** 写法不合法就退回默认值，免得用户把自己锁死 */
function readKey(id: string, fallback: string): string {
  const v = $<HTMLInputElement>(id).value.trim();
  return parseHotkey(v) ? v : fallback;
}

async function save() {
  const s: Settings = {
    port: Math.max(1024, Math.min(65535, Number($<HTMLInputElement>("port").value) || DEFAULT_SETTINGS.port)),
    hoverModifier: $<HTMLSelectElement>("hoverModifier").value as Settings["hoverModifier"],
    hoverDelayMs: Math.max(80, Math.min(3000, Number($<HTMLInputElement>("hoverDelayMs").value) || 300)),
    selectionIcon: $<HTMLInputElement>("selectionIcon").checked,
    defaultDisplay: $<HTMLSelectElement>("defaultDisplay").value as Settings["defaultDisplay"],
    disabledHosts: $<HTMLTextAreaElement>("disabledHosts").value.split("\n").map((x) => x.trim()).filter(Boolean),
    keyFullPage: readKey("keyFullPage", DEFAULT_SETTINGS.keyFullPage),
    keySelection: readKey("keySelection", DEFAULT_SETTINGS.keySelection),
    keyBlock: readKey("keyBlock", DEFAULT_SETTINGS.keyBlock),
  };
  await chrome.storage.sync.set(s);
  await load(); // 非法写法被退回默认值时，界面上要能看出来
  status("已保存，立即生效。");
}

function status(msg: string) {
  $("status").textContent = msg;
  setTimeout(() => ($("status").textContent = ""), 4000);
}

async function checkHealth() {
  const r = await chrome.runtime.sendMessage({ kind: "health" });
  if (r?.ok && r.health) {
    status(`桌面端已连接：版本 ${r.health.version}，并发 ${r.health.concurrency}，每请求 ${r.health.batch_size} 段`);
  } else {
    status(r?.error ?? "连不上桌面端");
  }
}

document.addEventListener("DOMContentLoaded", () => {
  void load();
  $("save").addEventListener("click", () => void save());
  $("check").addEventListener("click", () => void checkHealth());
});
