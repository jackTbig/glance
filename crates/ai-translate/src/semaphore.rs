//! 简单计数信号量（std 没有），用于限制 OpenAI 兼容后端的并发
use std::sync::{Arc, Condvar, Mutex};

pub struct Semaphore {
    permits: Mutex<usize>,
    cv: Condvar,
}

pub struct Permit {
    sem: Arc<Semaphore>,
}

impl Semaphore {
    pub fn new(n: usize) -> Arc<Semaphore> {
        Arc::new(Semaphore { permits: Mutex::new(n.max(1)), cv: Condvar::new() })
    }

    pub fn acquire(self: &Arc<Self>) -> Permit {
        let mut p = self.permits.lock().unwrap();
        while *p == 0 {
            p = self.cv.wait(p).unwrap();
        }
        *p -= 1;
        Permit { sem: self.clone() }
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        *self.sem.permits.lock().unwrap() += 1;
        self.sem.cv.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn limits_concurrent_holders() {
        let sem = Semaphore::new(2);
        let cur = Arc::new(AtomicUsize::new(0));
        let max = Arc::new(AtomicUsize::new(0));
        let hs: Vec<_> = (0..8)
            .map(|_| {
                let (sem, cur, max) = (sem.clone(), cur.clone(), max.clone());
                std::thread::spawn(move || {
                    let _p = sem.acquire();
                    let n = cur.fetch_add(1, Ordering::SeqCst) + 1;
                    max.fetch_max(n, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(20));
                    cur.fetch_sub(1, Ordering::SeqCst);
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        assert_eq!(max.load(Ordering::SeqCst), 2);
    }
}
