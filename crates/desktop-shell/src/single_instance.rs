//! 单实例闸：命名互斥量，已有实例在跑就让新进程退出。
//!
//! 没有这道闸的后果两个应用都会中招：
//! - 简译实测过——第二个实例的全局热键全部注册失败、本地服务端口也被占，
//!   用户看到的是「装好了但快捷键没反应」。
//! - 字幕更糟——两个进程会同时每秒把窗口位置写进同一份 config.toml、
//!   同时往同一个日志文件里交错写。

/// 抢占名字，抢到返回 true。已经有人占着返回 false，调用方应当直接退出。
///
/// 句柄故意不关：进程退出时系统自动释放，正好是我们要的语义。
#[cfg(windows)]
pub fn claim(name: &str) -> bool {
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
    use windows_sys::Win32::System::Threading::CreateMutexW;
    let full: Vec<u16> = format!("Global\\{name}").encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let h = CreateMutexW(std::ptr::null(), 1, full.as_ptr());
        !h.is_null() && GetLastError() != ERROR_ALREADY_EXISTS
    }
}

#[cfg(not(windows))]
pub fn claim(_name: &str) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_claim_of_the_same_name_fails() {
        let name = format!("ds-test-{}", std::process::id());
        assert!(claim(&name), "第一次该抢到");
        #[cfg(windows)]
        assert!(!claim(&name), "第二次必须被挡住");
        // 换个名字不受影响：两个应用各用各的闸
        assert!(claim(&format!("{name}-other")));
    }
}
