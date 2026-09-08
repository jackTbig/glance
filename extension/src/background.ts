/**
 * service worker：插件里唯一与本地服务通信的地方。
 * content script 不直接连本地服务，避免每个页面都要声明 host 权限。
 */
import { DEFAULT_SETTINGS, SERVICE_DOWN, type Message, type Reply, type Settings } from "./shared";

async function getSettings(): Promise<Settings> {
  const got = await chrome.storage.sync.get(DEFAULT_SETTINGS);
  return { ...DEFAULT_SETTINGS, ...got } as Settings;
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
          const h = await callLocal("/v1/health");
          return { ok: true, health: h };
        }
        case "settings": {
          return { ok: true, settings: await getSettings() };
        }
        case "translate": {
          const body = JSON.stringify({ scene: msg.scene, items: msg.items, context: msg.context });
          const r = await callLocal("/v1/translate", {
            method: "POST",
            headers: { "Content-Type": "application/json" },
            body,
          });
          return { ok: true, results: r.results ?? [] };
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

chrome.commands?.onCommand.addListener(async (command) => {
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  if (tab?.id === undefined) return;
  const kind =
    command === "translate-page"
      ? "toggle_page"
      : command === "translate-selection"
        ? "translate_selection"
        : command === "translate-block"
          ? "translate_block"
          : null;
  if (kind) {
    try {
      await chrome.tabs.sendMessage(tab.id, { kind });
    } catch {
      /* 页面不支持注入（chrome:// 等），忽略 */
    }
  }
});
