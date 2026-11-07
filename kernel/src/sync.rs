use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Tek çekirdekli boot boyunca yeterli. SMP geldiğinde `compare_exchange`
/// döngüsü gerçek bir spinlock olur; şimdiden doğru şekilde yazıldı.
pub struct SpinLock<T> {
    locked: AtomicBool,
    value: core::cell::UnsafeCell<T>,
}

unsafe impl<T: Send> Sync for SpinLock<T> {}
unsafe impl<T: Send> Send for SpinLock<T> {}

impl<T> SpinLock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            value: core::cell::UnsafeCell::new(value),
        }
    }

    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
        SpinLockGuard { lock: self }
    }

    pub fn try_lock(&self) -> Option<SpinLockGuard<'_, T>> {
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Some(SpinLockGuard { lock: self })
        } else {
            None
        }
    }

    /// # Safety
    /// Kesme bağlamında alınmış bir kilidi bırakmak için kullanılmamalı.
    pub unsafe fn force_unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }
}

pub struct SpinLockGuard<'a, T> {
    lock: &'a SpinLock<T>,
}

impl<T> core::ops::Deref for SpinLockGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lock.value.get() }
    }
}

impl<T> core::ops::DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.value.get() }
    }
}

impl<T> Drop for SpinLockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}

/// Tek yazar / çok okuyucu gerektirmeyen sayaçlar için.
pub struct Counter(AtomicU32);

impl Counter {
    pub const fn new() -> Self {
        Self(AtomicU32::new(0))
    }
    pub fn increment(&self) -> u32 {
        self.0.fetch_add(1, Ordering::Relaxed)
    }
    pub fn get(&self) -> u32 {
        self.0.load(Ordering::Relaxed)
    }
}
