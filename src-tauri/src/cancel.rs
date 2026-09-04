//! 下载取消 —— 按 `task_id` 维护一个可被「停止」命令触发的取消令牌。
//!
//! 由于 `pull_image` 是独立异步命令，无法直接中断其 Future，这里用一个全局
//! 注册表保存每个下载任务的 `AtomicBool`，「停止」时置位；下载流程在流式读取
//! blob 的每个块之间、以及各阶段之间检查该标志，命中即以错误终止。

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// 取消令牌：`true` 表示已请求停止。
pub type CancelToken = Arc<AtomicBool>;

fn registry() -> &'static Mutex<HashMap<String, CancelToken>> {
    static REG: OnceLock<Mutex<HashMap<String, CancelToken>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 为某个下载任务注册一个取消令牌（覆盖同名旧令牌），返回其引用。
pub fn register(task_id: &str) -> CancelToken {
    let token = Arc::new(AtomicBool::new(false));
    registry()
        .lock()
        .unwrap()
        .insert(task_id.to_string(), Arc::clone(&token));
    token
}

/// 下载结束（成功或失败）后移除令牌，避免注册表膨胀。
pub fn unregister(task_id: &str) {
    registry().lock().unwrap().remove(task_id);
}

/// 请求停止指定任务。返回该任务是否存在。
pub fn stop(task_id: &str) -> bool {
    registry()
        .lock()
        .unwrap()
        .get(task_id)
        .map(|t| t.store(true, Ordering::SeqCst))
        .is_some()
}

/// 该任务是否已被请求停止。
pub fn is_canceled(token: &CancelToken) -> bool {
    token.load(Ordering::SeqCst)
}

/// 阶段/块间隙检查：已停止则返回统一中文错误，用于终止下载。
pub fn check(token: &CancelToken) -> Result<(), String> {
    if is_canceled(token) {
        Err("下载已被停止".to_string())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_stop_and_check() {
        let tok = register("t1");
        assert!(!is_canceled(&tok));
        assert!(stop("t1"));
        assert!(is_canceled(&tok));
        assert_eq!(check(&tok), Err("下载已被停止".to_string()));
    }

    #[test]
    fn stop_unknown_returns_false() {
        assert!(!stop("nope"));
    }

    #[test]
    fn unregister_removes() {
        let _ = register("t2");
        unregister("t2");
        assert!(!stop("t2"));
    }

    #[test]
    fn register_overwrites_previous_token() {
        let a = register("t3");
        let b = register("t3");
        // 旧令牌与新令牌解耦：停止新令牌不影响旧令牌。
        stop("t3");
        assert!(!is_canceled(&a));
        assert!(is_canceled(&b));
    }
}