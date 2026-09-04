live-caption 桌面悬浮实时字幕（英文语音 → 中文字幕）

一、怎么用
1. 开始菜单或桌面启动 live-caption，屏幕上出现半透明字幕条（可拖动）。
2. 播放任何英文语音（线上会议、视频），说完一句约 1 秒出英文、再约 1.5 秒出中文；说话过程中会先逐字打出中间结果。
3. 鼠标悬停字幕条右上角有按钮：设置 / 穿透 / 记录 / 透明度 / 字号 / 显示段数 / 暂停 / 退出。
   点"穿透"后鼠标会点到下面的窗口，按 Ctrl+Alt+C 切回。

二、翻译后端（设置窗里选，立即生效）
- Claude Code CLI（默认）：需要已安装并登录 claude 命令。安装包可代装，登录要自己跑一次 claude。
- Codex CLI：需要已安装并登录 codex 命令；每句起一个进程，慢。
- OpenAI 兼容接口：DeepSeek、通义千问、自建网关、本地 Ollama 都行，填 Base URL、API Key、模型。

三、文件位置
- 程序与模型：安装目录（默认 D:\Program Files\live-caption，模型在 models\）。
- 配置、字幕记录、日志：%APPDATA%\live-caption\（config.toml / transcripts\ / logs\）。

四、依赖
- 显卡驱动自带的 Vulkan（识别用 GPU）；VC++ 运行库（安装包自动装）。
- 中文字体用系统自带的微软雅黑。

五、隐私
- 会议音频不出本机；只有识别出的英文文本会送到你选的翻译后端。
