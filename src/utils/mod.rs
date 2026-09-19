pub mod layout;
pub mod output;
pub mod platform;
pub mod process;

/// 统一调试日志：仅当 `GG_DEBUG` 存在时输出。
///
/// 固定 `[DEBUG]` 前缀 + RFC3339 时间戳，并携带调用点上下文，
/// 便于排查「候选程序一个都没成功」这类多分支回退问题。
macro_rules! debug_log {
    ($($arg:tt)*) => {
        if std::env::var_os("GG_DEBUG").is_some() {
            eprintln!(
                "[DEBUG] {} {}",
                humantime::format_rfc3339_seconds(std::time::SystemTime::now()),
                format!($($arg)*)
            );
        }
    };
}

pub(crate) use debug_log;
