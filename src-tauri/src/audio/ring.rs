//! Кольцевой буфер для передачи семплов из аудио-callback'а в consumer-поток
//! (этап 01 — `promts/01_audio_core.md`, шаг 3).
//!
//! Реализация — lock-free **SPSC** (один производитель — аудио-callback, один
//! потребитель — поток записи): запись и чтение без блокировок и без аллокаций
//! в реальном времени (буфер преаллоцирован при создании). Семплы хранятся как
//! интерливнутые кадры в нативном формате устройства (`f32`); приведение к
//! целевому формату делает consumer (см. [`crate::audio::convert`]).
//!
//! При переполнении (consumer не успевает) производитель **дропает** лишние
//! семплы и инкрементирует счётчик [`Producer::dropped`] — это сигнал xrun для
//! будущей диагностики (этап 02), а не паника: запись не прерывается.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Разделяемое хранилище кольцевого буфера. Индексы `head`/`tail` —
/// монотонно растущие счётчики семплов; слот = индекс по модулю `capacity`.
/// Заполнение = `head - tail` (в семплах), «полный» при `== capacity`.
struct RingBuffer {
    buf: Box<[UnsafeCell<f32>]>,
    capacity: usize,
    /// Индекс записи (двигает только producer).
    head: AtomicUsize,
    /// Индекс чтения (двигает только consumer).
    tail: AtomicUsize,
    /// Сколько семплов дропнуто из-за переполнения (двигает только producer).
    dropped: AtomicUsize,
}

// SAFETY: доступ к `UnsafeCell` дисциплинирован моделью SPSC — в каждую ячейку
// пишет ровно один поток (producer) и только до публикации `head` (Release);
// consumer читает ячейку только после наблюдения соответствующего `head`
// (Acquire) и до продвижения `tail`. Пересечения по одной ячейке между
// потоками нет, поэтому совместный `&RingBuffer` безопасен.
unsafe impl Send for RingBuffer {}
unsafe impl Sync for RingBuffer {}

impl RingBuffer {
    fn new(capacity: usize) -> Arc<Self> {
        // Минимум 1 слот, чтобы избежать деления на ноль и пустого буфера.
        let capacity = capacity.max(1);
        let mut v = Vec::with_capacity(capacity);
        v.resize_with(capacity, || UnsafeCell::new(0.0));
        Arc::new(Self {
            buf: v.into_boxed_slice(),
            capacity,
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            dropped: AtomicUsize::new(0),
        })
    }
}

/// Производящая половина (для аудио-callback'а). `Send`, не `Clone` — SPSC.
pub struct Producer {
    inner: Arc<RingBuffer>,
}

/// Потребляющая половина (для потока записи). `Send`, не `Clone` — SPSC.
pub struct Consumer {
    inner: Arc<RingBuffer>,
}

/// Создать кольцевой буфер ёмкостью `capacity` семплов и вернуть пару
/// производитель/потребитель.
pub fn channel(capacity: usize) -> (Producer, Consumer) {
    let inner = RingBuffer::new(capacity);
    (
        Producer {
            inner: Arc::clone(&inner),
        },
        Consumer { inner },
    )
}

impl Producer {
    /// Ёмкость буфера в семплах.
    pub fn capacity(&self) -> usize {
        self.inner.capacity
    }

    /// Сколько семплов суммарно дропнуто из-за переполнения.
    pub fn dropped(&self) -> usize {
        self.inner.dropped.load(Ordering::Relaxed)
    }

    /// Записать как можно больше семплов из `samples`; вернуть число
    /// фактически записанных. Не помещающийся «хвост» дропается (с учётом в
    /// счётчике [`Producer::dropped`]). Без блокировок и аллокаций.
    pub fn push_slice(&self, samples: &[f32]) -> usize {
        let rb = &self.inner;
        let head = rb.head.load(Ordering::Relaxed);
        let tail = rb.tail.load(Ordering::Acquire);
        let free = rb.capacity - head.wrapping_sub(tail);
        let to_write = samples.len().min(free);

        for (i, &s) in samples.iter().take(to_write).enumerate() {
            let slot = head.wrapping_add(i) % rb.capacity;
            // SAFETY: см. обоснование на `unsafe impl` — слот эксклюзивен для
            // producer'а до публикации нового `head` ниже.
            unsafe {
                *rb.buf[slot].get() = s;
            }
        }

        rb.head
            .store(head.wrapping_add(to_write), Ordering::Release);

        let dropped = samples.len() - to_write;
        if dropped > 0 {
            rb.dropped.fetch_add(dropped, Ordering::Relaxed);
        }
        to_write
    }
}

impl Consumer {
    /// Ёмкость буфера в семплах.
    pub fn capacity(&self) -> usize {
        self.inner.capacity
    }

    /// Сколько семплов доступно для чтения прямо сейчас.
    pub fn len(&self) -> usize {
        let rb = &self.inner;
        let head = rb.head.load(Ordering::Acquire);
        let tail = rb.tail.load(Ordering::Relaxed);
        head.wrapping_sub(tail)
    }

    /// Буфер пуст?
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Прочитать до `out.len()` семплов в `out`; вернуть число прочитанных.
    pub fn pop_slice(&self, out: &mut [f32]) -> usize {
        let rb = &self.inner;
        let head = rb.head.load(Ordering::Acquire);
        let tail = rb.tail.load(Ordering::Relaxed);
        let available = head.wrapping_sub(tail);
        let to_read = out.len().min(available);

        for (i, dst) in out.iter_mut().take(to_read).enumerate() {
            let slot = tail.wrapping_add(i) % rb.capacity;
            // SAFETY: ячейка опубликована producer'ом (наблюдаем через `head`
            // с Acquire) и не пишется заново до продвижения `tail` ниже.
            unsafe {
                *dst = *rb.buf[slot].get();
            }
        }

        rb.tail.store(tail.wrapping_add(to_read), Ordering::Release);
        to_read
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_then_pop_roundtrip() {
        let (p, c) = channel(8);
        assert_eq!(p.push_slice(&[1.0, 2.0, 3.0]), 3);
        assert_eq!(c.len(), 3);

        let mut out = [0.0f32; 4];
        let n = c.pop_slice(&mut out);
        assert_eq!(n, 3);
        assert_eq!(&out[..3], &[1.0, 2.0, 3.0]);
        assert!(c.is_empty());
    }

    #[test]
    fn wraps_around_capacity_boundary() {
        // Ёмкость 4; продвигаем head/tail за границу буфера и проверяем, что
        // данные читаются корректно после wrap'а слота.
        let (p, c) = channel(4);
        let mut out = [0.0f32; 4];

        // Заполнили 3, прочитали 3 — tail сместился к 3.
        assert_eq!(p.push_slice(&[1.0, 2.0, 3.0]), 3);
        assert_eq!(c.pop_slice(&mut out), 3);

        // Теперь запись 4 семплов перепрыгнет границу (слоты 3,0,1,2).
        assert_eq!(p.push_slice(&[10.0, 11.0, 12.0, 13.0]), 4);
        assert_eq!(c.len(), 4);
        assert_eq!(c.pop_slice(&mut out), 4);
        assert_eq!(&out, &[10.0, 11.0, 12.0, 13.0]);
    }

    #[test]
    fn overflow_drops_tail_and_counts() {
        // Ёмкость 4: пишем 6 — влезают 4, два дропаются со счётчиком.
        let (p, c) = channel(4);
        let written = p.push_slice(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(written, 4);
        assert_eq!(p.dropped(), 2);
        assert_eq!(c.len(), 4);

        let mut out = [0.0f32; 8];
        // Сохраняются первые 4 (хвост дропается, а не голова).
        assert_eq!(c.pop_slice(&mut out), 4);
        assert_eq!(&out[..4], &[1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn capacity_is_at_least_one() {
        let (p, _c) = channel(0);
        assert_eq!(p.capacity(), 1);
    }

    #[test]
    fn pop_from_empty_returns_zero() {
        let (_p, c) = channel(4);
        let mut out = [0.0f32; 2];
        assert_eq!(c.pop_slice(&mut out), 0);
    }
}
