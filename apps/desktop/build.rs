//! 把应用图标与版本信息写进 exe 的资源表（文件管理器、任务栏、属性页都读它）。
fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=../../assets/icon.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icon.ico");
        res.set("ProductName", "简译");
        res.set("FileDescription", "简译 Glance — 桌面与浏览器翻译工具");
        res.set("LegalCopyright", "Jack");
        if let Err(e) = res.compile() {
            // 没有 Windows SDK 的资源编译器时不阻断构建，只是 exe 没图标
            println!("cargo:warning=图标资源未能编入 exe：{e}");
        }
    }
}
