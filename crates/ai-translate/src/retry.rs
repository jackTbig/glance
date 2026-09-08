//! 指数退避重试：429/5xx/网络错误重试，其他 4xx 直接返回
use crate::openai::HttpStatusError;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_retries: u32,
    pub base_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self { max_retries: 3, base_delay: Duration::from_secs(1) }
    }
}

impl RetryPolicy {
    pub fn none() -> Self {
        Self { max_retries: 0, base_delay: Duration::ZERO }
    }
}

pub fn is_retryable(e: &anyhow::Error) -> bool {
    match e.downcast_ref::<HttpStatusError>() {
        Some(h) => h.status == 429 || h.status >= 500,
        None => true,
    }
}

/// 第 k 次重试（k 从 0 起）前的等待：429 带 Retry-After 用它，否则 base_delay * 2^k
fn delay_for(policy: &RetryPolicy, e: &anyhow::Error, k: u32) -> Duration {
    if let Some(h) = e.downcast_ref::<HttpStatusError>() {
        if h.status == 429 {
            if let Some(ra) = h.retry_after {
                return ra;
            }
        }
    }
    policy.base_delay.saturating_mul(1u32 << k.min(10))
}

pub fn with_retry<T>(policy: &RetryPolicy, mut f: impl FnMut() -> anyhow::Result<T>) -> anyhow::Result<T> {
    let mut k = 0u32;
    loop {
        match f() {
            Ok(v) => return Ok(v),
            Err(e) => {
                if k >= policy.max_retries || !is_retryable(&e) {
                    return Err(e);
                }
                let d = delay_for(policy, &e, k);
                tracing::debug!("第 {} 次重试，等待 {:?}：{e}", k + 1, d);
                std::thread::sleep(d);
                k += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fast() -> RetryPolicy {
        RetryPolicy { max_retries: 3, base_delay: Duration::from_millis(1) }
    }

    fn http(status: u16) -> anyhow::Error {
        anyhow::Error::new(HttpStatusError { status, retry_after: None, body: String::new() })
    }

    #[test]
    fn retries_then_succeeds() {
        let mut n = 0;
        let r = with_retry(&fast(), || {
            n += 1;
            if n < 3 {
                Err(http(503))
            } else {
                Ok(n)
            }
        });
        assert_eq!(r.unwrap(), 3);
    }

    #[test]
    fn non_retryable_returns_immediately() {
        let mut n = 0;
        let r: anyhow::Result<()> = with_retry(&fast(), || {
            n += 1;
            Err(http(400))
        });
        assert!(r.is_err());
        assert_eq!(n, 1);
    }

    #[test]
    fn gives_up_after_max() {
        let mut n = 0;
        let r: anyhow::Result<()> = with_retry(&fast(), || {
            n += 1;
            Err(anyhow::anyhow!("network"))
        });
        assert!(r.is_err());
        assert_eq!(n, 4, "1 次 + 3 次重试");
    }

    #[test]
    fn retry_after_overrides_backoff() {
        let e = anyhow::Error::new(HttpStatusError { status: 429, retry_after: Some(Duration::from_millis(7)), body: String::new() });
        assert_eq!(delay_for(&fast(), &e, 2), Duration::from_millis(7));
        assert_eq!(delay_for(&fast(), &http(503), 2), Duration::from_millis(4));
    }
}
