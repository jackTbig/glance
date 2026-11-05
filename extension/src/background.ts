/**
 * service worker：插件里唯一与本地服务通信的地方。
 * content script 不直接连本地服务，避免每个页面都要声明 host 权限。
 *
 * 另有一条「直连」路（backend = direct）：插件自己请求第三方模型接口，
 * 给只装插件、没装桌面端的场景用（Chrome 商店版）。本地服务那条路一个字没改。
 */
import { API_KEY_KEY, DEFAULT_SETTINGS, SERVICE_DOWN, type Message, type Reply, type Settings } from "./shared";
import { chat, listModels, translateDirect } from "./direct";

async function getSettings(): Promise<Settings> {
  const got = await chrome.storage.sync.get(DEFAULT_SETTINGS);
  return { ...DEFAULT_SETTINGS, ...got } as Settings;
}

/** API Key 存在 local 存储（不同步到 Google） */
async function getApiKey(): Promise<string> {
  const got = await chrome.storage.local.get(API_KEY_KEY);
  const k = got[API_KEY_KEY];
  return typeof k === "string" ? k : "";
}

async function callLocal(path: string, init?: RequestInit): Promise<any> {
  const { port } = await getSettings();
  const url = `http://127.0.0.1:${port}${path}`;
  let resp: Response;
  try {
    resp = await fetch(url, init);
  } catch {
    throw new Error(SERVICE_DOWN);
  }
  if (!resp.ok) {
    let detail = "";
    try {
      detail = (await resp.json())?.error ?? "";
    } catch {
      /* 服务端没返回 JSON 就算了 */
    }
    throw new Error(detail || `本地服务返回 ${resp.status}`);
  }
  return resp.json();
}

chrome.runtime.onMessage.addListener((msg: Message, _sender, sendResponse) => {
  (async (): Promise<Reply> => {
    try {
      switch (msg.kind) {
        case "health": {
          const s = await getSettings();
          if (s.backend === "direct") {
            // 直连模式不查本地服务，只报配置齐不齐（真连不上的话由翻译请求报错）
            const key = await getApiKey();
            return {
              ok: true,
              health: {
                version: chrome.runtime.getManifest().version,
                backend: "direct",
                model: s.apiModel,
                concurrency: 8,
                batch_size: 10,
                configured: Boolean(s.apiBase && key && s.apiModel),
              },
            };
          }
          const h = await callLocal("/v1/health");
          return { ok: true, health: h };
        }
        case "settings": {
          return { ok: true, settings: await getSettings() };
        }
        case "translate": {
          const s = await getSettings();
          if (s.backend === "direct") {
            const key = await getApiKey();
            const results = await translateDirect(
              { base: s.apiBase, key, model: s.apiModel },
              msg.scene,
              msg.items,
              msg.context,
            );
            return { ok: true, results };
          }
          const body = JSON.stringify({ scene: msg.scene, items: msg.items, context: msg.context });
          const r = await callLocal("/v1/translate", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body,
          });
          return { ok: true, results: r.results ?? [] };
        }
        case "lookup_offline": {
          const s = await getSettings();
          if (s.backend === "direct") {
            // 离线词库在桌面端，直连模式没有；调用方拿 found=false 就走模型
            return { ok: true, offline: { found: false } };
          }
          const body = JSON.stringify({ text: msg.text });
          const r = await callLocal("/v1/lookup_offline", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body,
          });
          return { ok: true, offline: r as { found: boolean; text?: string } };
        }
        case "direct_test": {
          const cfg = { base: msg.base, key: msg.key, model: msg.model };
          if (!cfg.base || !cfg.key) return { ok: false, error: "接口地址或 API Key 没填" };
          // 发一句话验 Key 与模型能不能真的用
          const reply = await chat(cfg, "只回复 OK", "只回复 OK", 20000);
          return { ok: true, models: [], reply: reply.trim() };
        }
        case "list_models": {
          const r = await listModels({ base: msg.base, key: msg.key, model: "" });
          if ("error" in r) return { ok: false, error: r.error };
          return { ok: true, models: r.models };
        }
        default:
          return { ok: false, error: "未知消息" };
      }
    } catch (e) {
      return { ok: false, error: e instanceof Error ? e.message : String(e) };
    }
  })().then(sendResponse);
  return true; // 异步回复
});

// 点插件图标弹的是 popup.html（manifest 里的 action.default_popup），
// 声明了 default_popup 之后 chrome.action.onClicked 不再触发，所以这里不挂它。

/**
 * 装好或更新后，给已经打开着的标签页重新注入 content script。
 * 不这么做的话，老页面里跑的还是上一版脚本，它的扩展上下文已失效，
 * 用户一划词就得到 "Extension context invalidated"，只能自己刷新页面。
 */
chrome.runtime.onInstalled.addListener(async () => {
  const tabs = await chrome.tabs.query({ url: ["http://*/*", "https://*/*", "file://*/*"] });
  for (const tab of tabs) {
    if (tab.id === undefined) continue;
    try {
      await chrome.scripting.executeScript({ target: { tabId: tab.id }, files: ["content.js"] });
    } catch {
      // 有些页面不让注入（下载页、PDF 查看器、用户没勾「允许访问文件网址」），跳过就是
    }
  }
});

chrome.commands?.onCommand.addListener(async (command) => {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  if (tab?.id === undefined) return;
  const kind =
    command === "translate-page"
      ? "toggle_page"
      : command === "translate-selection"
        ? "translate_selection"
        : null;
  if (kind) {
    try {
      await chrome.tabs.sendMessage(tab.id, { kind });
    } catch {
      /* 页面不支持注入（chrome:// 等），忽略 */
    }
  }
});
