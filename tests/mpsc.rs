//! Tests borrowed from `std::sync::mpsc`.
//!
//! This is a channel implementation mimicking MPSC channels from the standard library, but the
//! internals actually use `crossbeam-channel`. There's an auxilliary channel `disconnected`, which
//! becomes closed once the receiver gets dropped, thus notifying the senders that the MPSC channel
//! is disconnected.
//!
//! This channel is then tested against the original tests for MPSC channels.
//!
//! Two minor tweaks were needed to make the tests compile:
//!
//! - Replace `box` syntax with `Box::new`.
//! - Replace all uses of `Select` with `select!`.

#[macro_use]
extern crate crossbeam_channel as channel;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{RecvError, RecvTimeoutError, SendError, TryRecvError, TrySendError};
use std::time::Duration;

pub struct Sender<T> {
    pub inner: channel::Sender<T>,
    disconnected: channel::Receiver<()>,
    is_disconnected: Arc<AtomicBool>,
}

impl<T> Sender<T> {
    pub fn send(&self, t: T) -> Result<(), SendError<T>> {
        if self.is_disconnected.load(Ordering::SeqCst) {
            Err(SendError(t))
        } else {
            self.inner.send(t);
            Ok(())
        }
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Sender<T> {
        Sender {
            inner: self.inner.clone(),
            disconnected: self.disconnected.clone(),
            is_disconnected: self.is_disconnected.clone(),
        }
    }
}

pub struct SyncSender<T> {
    pub inner: channel::Sender<T>,
    disconnected: channel::Receiver<()>,
    is_disconnected: Arc<AtomicBool>,
}

impl<T> SyncSender<T> {
    pub fn send(&self, t: T) -> Result<(), SendError<T>> {
        if self.is_disconnected.load(Ordering::SeqCst) {
            Err(SendError(t))
        } else {
            select! {
                send(self.inner, t) => Ok(()),
                default => {
                    select! {
                        send(self.inner, t) => Ok(()),
                        recv(self.disconnected) => Err(SendError(t)),
                    }
                }
            }
        }
    }

    pub fn try_send(&self, t: T) -> Result<(), TrySendError<T>> {
        if self.is_disconnected.load(Ordering::SeqCst) {
            Err(TrySendError::Disconnected(t))
        } else {
            select! {
                send(self.inner, t) => Ok(()),
                default => Err(TrySendError::Full(t)),
            }
        }
    }
}

impl<T> Clone for SyncSender<T> {
    fn clone(&self) -> SyncSender<T> {
        SyncSender {
            inner: self.inner.clone(),
            disconnected: self.disconnected.clone(),
            is_disconnected: self.is_disconnected.clone(),
        }
    }
}

pub struct Receiver<T> {
    pub inner: channel::Receiver<T>,
    _disconnected: channel::Sender<()>,
    is_disconnected: Arc<AtomicBool>,
}

impl<T> Receiver<T> {
    pub fn try_recv(&self) -> Result<T, TryRecvError> {
        select! {
            recv(self.inner, msg) => match msg {
                None => Err(TryRecvError::Disconnected),
                Some(msg) => Ok(msg),
            }
            default => Err(TryRecvError::Empty),
        }
    }

    pub fn recv(&self) -> Result<T, RecvError> {
        match self.inner.recv() {
            None => Err(RecvError),
            Some(msg) => Ok(msg),
        }
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Result<T, RecvTimeoutError> {
        select! {
            recv(self.inner, msg) => match msg {
                None => Err(RecvTimeoutError::Disconnected),
                Some(msg) => Ok(msg),
            }
            default => {
                select! {
                    recv(self.inner, msg) => match msg {
                        None => Err(RecvTimeoutError::Disconnected),
                        Some(msg) => Ok(msg),
                    }
                    recv(channel::after(timeout)) => Err(RecvTimeoutError::Timeout),
                }
            }
        }
    }

    pub fn iter(&self) -> Iter<T> {
        Iter { inner: self }
    }

    pub fn try_iter(&self) -> TryIter<T> {
        TryIter { inner: self }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        self.is_disconnected.store(true, Ordering::SeqCst);
    }
}

impl<'a, T> IntoIterator for &'a Receiver<T> {
    type Item = T;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Iter<'a, T> {
        self.iter()
    }
}

impl<T> IntoIterator for Receiver<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;

    fn into_iter(self) -> IntoIter<T> {
        IntoIter { inner: self }
    }
}

pub struct TryIter<'a, T: 'a> {
    inner: &'a Receiver<T>,
}

impl<'a, T> Iterator for TryIter<'a, T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        self.inner.try_recv().ok()
    }
}

pub struct Iter<'a, T: 'a> {
    inner: &'a Receiver<T>,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        self.inner.recv().ok()
    }
}

pub struct IntoIter<T> {
    inner: Receiver<T>,
}

impl<T> Iterator for IntoIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        self.inner.recv().ok()
    }
}

pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let (s1, r1) = channel::unbounded();
    let (s2, r2) = channel::bounded(1);
    let is_disconnected = Arc::new(AtomicBool::new(false));

    let s = Sender {
        inner: s1,
        disconnected: r2,
        is_disconnected: is_disconnected.clone(),
    };
    let r = Receiver {
        inner: r1,
        _disconnected: s2,
        is_disconnected,
    };
    (s, r)
}

pub fn sync_channel<T>(bound: usize) -> (SyncSender<T>, Receiver<T>) {
    let (s1, r1) = channel::bounded(bound);
    let (s2, r2) = channel::bounded(1);
    let is_disconnected = Arc::new(AtomicBool::new(false));

    let s = SyncSender {
        inner: s1,
        disconnected: r2,
        is_disconnected: is_disconnected.clone(),
    };
    let r = Receiver {
        inner: r1,
        _disconnected: s2,
        is_disconnected,
    };
    (s, r)
}

// https://github.com/rust-lang/rust/tree/master/src/libstd/sync/mpsc
mod channel_tests {
    use std::env;
    use super::*;
    use std::thread;
    use std::time::{Duration, Instant};

    pub fn stress_factor() -> usize {
        match env::var("RUST_TEST_STRESS") {
            Ok(val) => val.parse().unwrap(),
            Err(..) => 1,
        }
    }

    #[test]
    fn smoke() {
        let (tx, rx) = channel::<i32>();
        tx.send(1).unwrap();
        assert_eq!(rx.recv().unwrap(), 1);
    }

    #[test]
    fn drop_full() {
        let (tx, _rx) = channel::<Box<isize>>();
        tx.send(Box::new(1)).unwrap();
    }

    #[test]
    fn drop_full_shared() {
        let (tx, _rx) = channel::<Box<isize>>();
        drop(tx.clone());
        drop(tx.clone());
        tx.send(Box::new(1)).unwrap();
    }

    #[test]
    fn smoke_shared() {
        let (tx, rx) = channel::<i32>();
        tx.send(1).unwrap();
        assert_eq!(rx.recv().unwrap(), 1);
        let tx = tx.clone();
        tx.send(1).unwrap();
        assert_eq!(rx.recv().unwrap(), 1);
    }

    #[test]
    fn smoke_threads() {
        let (tx, rx) = channel::<i32>();
        let _t = thread::spawn(move|| {
            tx.send(1).unwrap();
        });
        assert_eq!(rx.recv().unwrap(), 1);
    }

    #[test]
    fn smoke_port_gone() {
        let (tx, rx) = channel::<i32>();
        drop(rx);
        assert!(tx.send(1).is_err());
    }

    #[test]
    fn smoke_shared_port_gone() {
        let (tx, rx) = channel::<i32>();
        drop(rx);
        assert!(tx.send(1).is_err())
    }

    #[test]
    fn smoke_shared_port_gone2() {
        let (tx, rx) = channel::<i32>();
        drop(rx);
        let tx2 = tx.clone();
        drop(tx);
        assert!(tx2.send(1).is_err());
    }

    #[test]
    fn port_gone_concurrent() {
        let (tx, rx) = channel::<i32>();
        let _t = thread::spawn(move|| {
            rx.recv().unwrap();
        });
        while tx.send(1).is_ok() {}
    }

    #[test]
    fn port_gone_concurrent_shared() {
        let (tx, rx) = channel::<i32>();
        let tx2 = tx.clone();
        let _t = thread::spawn(move|| {
            rx.recv().unwrap();
        });
        while tx.send(1).is_ok() && tx2.send(1).is_ok() {}
    }

    #[test]
    fn smoke_chan_gone() {
        let (tx, rx) = channel::<i32>();
        drop(tx);
        assert!(rx.recv().is_err());
    }

    #[test]
    fn smoke_chan_gone_shared() {
        let (tx, rx) = channel::<()>();
        let tx2 = tx.clone();
        drop(tx);
        drop(tx2);
        assert!(rx.recv().is_err());
    }

    #[test]
    fn chan_gone_concurrent() {
        let (tx, rx) = channel::<i32>();
        let _t = thread::spawn(move|| {
            tx.send(1).unwrap();
            tx.send(1).unwrap();
        });
        while rx.recv().is_ok() {}
    }

    #[test]
    fn stress() {
        let (tx, rx) = channel::<i32>();
        let t = thread::spawn(move|| {
            for _ in 0..10000 { tx.send(1).unwrap(); }
        });
        for _ in 0..10000 {
            assert_eq!(rx.recv().unwrap(), 1);
        }
        t.join().ok().unwrap();
    }

    #[test]
    fn stress_shared() {
        const AMT: u32 = 10000;
        const NTHREADS: u32 = 8;
        let (tx, rx) = channel::<i32>();

        let t = thread::spawn(move|| {
            for _ in 0..AMT * NTHREADS {
                assert_eq!(rx.recv().unwrap(), 1);
            }
            match rx.try_recv() {
                Ok(..) => panic!(),
                _ => {}
            }
        });

        for _ in 0..NTHREADS {
            let tx = tx.clone();
            thread::spawn(move|| {
                for _ in 0..AMT { tx.send(1).unwrap(); }
            });
        }
        drop(tx);
        t.join().ok().unwrap();
    }

    #[test]
    fn send_from_outside_runtime() {
        let (tx1, rx1) = channel::<()>();
        let (tx2, rx2) = channel::<i32>();
        let t1 = thread::spawn(move|| {
            tx1.send(()).unwrap();
            for _ in 0..40 {
                assert_eq!(rx2.recv().unwrap(), 1);
            }
        });
        rx1.recv().unwrap();
        let t2 = thread::spawn(move|| {
            for _ in 0..40 {
                tx2.send(1).unwrap();
            }
        });
        t1.join().ok().unwrap();
        t2.join().ok().unwrap();
    }

    #[test]
    fn recv_from_outside_runtime() {
        let (tx, rx) = channel::<i32>();
        let t = thread::spawn(move|| {
            for _ in 0..40 {
                assert_eq!(rx.recv().unwrap(), 1);
            }
        });
        for _ in 0..40 {
            tx.send(1).unwrap();
        }
        t.join().ok().unwrap();
    }

    #[test]
    fn no_runtime() {
        let (tx1, rx1) = channel::<i32>();
        let (tx2, rx2) = channel::<i32>();
        let t1 = thread::spawn(move|| {
            assert_eq!(rx1.recv().unwrap(), 1);
            tx2.send(2).unwrap();
        });
        let t2 = thread::spawn(move|| {
            tx1.send(1).unwrap();
            assert_eq!(rx2.recv().unwrap(), 2);
        });
        t1.join().ok().unwrap();
        t2.join().ok().unwrap();
    }

    #[test]
    fn oneshot_single_thread_close_port_first() {
        // Simple test of closing without sending
        let (_tx, rx) = channel::<i32>();
        drop(rx);
    }

    #[test]
    fn oneshot_single_thread_close_chan_first() {
        // Simple test of closing without sending
        let (tx, _rx) = channel::<i32>();
        drop(tx);
    }

    #[test]
    fn oneshot_single_thread_send_port_close() {
        // Testing that the sender cleans up the payload if receiver is closed
        let (tx, rx) = channel::<Box<i32>>();
        drop(rx);
        assert!(tx.send(Box::new(0)).is_err());
    }

    #[test]
    fn oneshot_single_thread_recv_chan_close() {
        // Receiving on a closed chan will panic
        let res = thread::spawn(move|| {
            let (tx, rx) = channel::<i32>();
            drop(tx);
            rx.recv().unwrap();
        }).join();
        // What is our res?
        assert!(res.is_err());
    }

    #[test]
    fn oneshot_single_thread_send_then_recv() {
        let (tx, rx) = channel::<Box<i32>>();
        tx.send(Box::new(10)).unwrap();
        assert!(*rx.recv().unwrap() == 10);
    }

    #[test]
    fn oneshot_single_thread_try_send_open() {
        let (tx, rx) = channel::<i32>();
        assert!(tx.send(10).is_ok());
        assert!(rx.recv().unwrap() == 10);
    }

    #[test]
    fn oneshot_single_thread_try_send_closed() {
        let (tx, rx) = channel::<i32>();
        drop(rx);
        assert!(tx.send(10).is_err());
    }

    #[test]
    fn oneshot_single_thread_try_recv_open() {
        let (tx, rx) = channel::<i32>();
        tx.send(10).unwrap();
        assert!(rx.recv() == Ok(10));
    }

    #[test]
    fn oneshot_single_thread_try_recv_closed() {
        let (tx, rx) = channel::<i32>();
        drop(tx);
        assert!(rx.recv().is_err());
    }

    #[test]
    fn oneshot_single_thread_peek_data() {
        let (tx, rx) = channel::<i32>();
        assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
        tx.send(10).unwrap();
        assert_eq!(rx.try_recv(), Ok(10));
    }

    #[test]
    fn oneshot_single_thread_peek_close() {
        let (tx, rx) = channel::<i32>();
        drop(tx);
        assert_eq!(rx.try_recv(), Err(TryRecvError::Disconnected));
        assert_eq!(rx.try_recv(), Err(TryRecvError::Disconnected));
    }

    #[test]
    fn oneshot_single_thread_peek_open() {
        let (_tx, rx) = channel::<i32>();
        assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
    }

    #[test]
    fn oneshot_multi_task_recv_then_send() {
        let (tx, rx) = channel::<Box<i32>>();
        let _t = thread::spawn(move|| {
            assert!(*rx.recv().unwrap() == 10);
        });

        tx.send(Box::new(10)).unwrap();
    }

    #[test]
    fn oneshot_multi_task_recv_then_close() {
        let (tx, rx) = channel::<Box<i32>>();
        let _t = thread::spawn(move|| {
            drop(tx);
        });
        let res = thread::spawn(move|| {
            assert!(*rx.recv().unwrap() == 10);
        }).join();
        assert!(res.is_err());
    }

    #[test]
    fn oneshot_multi_thread_close_stress() {
        for _ in 0..stress_factor() {
            let (tx, rx) = channel::<i32>();
            let _t = thread::spawn(move|| {
                drop(rx);
            });
            drop(tx);
        }
    }

    #[test]
    fn oneshot_multi_thread_send_close_stress() {
        for _ in 0..stress_factor() {
            let (tx, rx) = channel::<i32>();
            let _t = thread::spawn(move|| {
                drop(rx);
            });
            let _ = thread::spawn(move|| {
                tx.send(1).unwrap();
            }).join();
        }
    }

    #[test]
    fn oneshot_multi_thread_recv_close_stress() {
        for _ in 0..stress_factor() {
            let (tx, rx) = channel::<i32>();
            thread::spawn(move|| {
                let res = thread::spawn(move|| {
                    rx.recv().unwrap();
                }).join();
                assert!(res.is_err());
            });
            let _t = thread::spawn(move|| {
                thread::spawn(move|| {
                    drop(tx);
                });
            });
        }
    }

    #[test]
    fn oneshot_multi_thread_send_recv_stress() {
        for _ in 0..stress_factor() {
            let (tx, rx) = channel::<Box<isize>>();
            let _t = thread::spawn(move|| {
                tx.send(Box::new(10)).unwrap();
            });
            assert!(*rx.recv().unwrap() == 10);
        }
    }

    #[test]
    fn stream_send_recv_stress() {
        for _ in 0..stress_factor() {
            let (tx, rx) = channel();

            send(tx, 0);
            recv(rx, 0);

            fn send(tx: Sender<Box<i32>>, i: i32) {
                if i == 10 { return }

                thread::spawn(move|| {
                    tx.send(Box::new(i)).unwrap();
                    send(tx, i + 1);
                });
            }

            fn recv(rx: Receiver<Box<i32>>, i: i32) {
                if i == 10 { return }

                thread::spawn(move|| {
                    assert!(*rx.recv().unwrap() == i);
                    recv(rx, i + 1);
                });
            }
        }
    }

    #[test]
    fn oneshot_single_thread_recv_timeout() {
        let (tx, rx) = channel();
        tx.send(()).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_millis(1)), Ok(()));
        assert_eq!(rx.recv_timeout(Duration::from_millis(1)), Err(RecvTimeoutError::Timeout));
        tx.send(()).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_millis(1)), Ok(()));
    }

    #[test]
    fn stress_recv_timeout_two_threads() {
        let (tx, rx) = channel();
        let stress = stress_factor() + 100;
        let timeout = Duration::from_millis(100);

        thread::spawn(move || {
            for i in 0..stress {
                if i % 2 == 0 {
                    thread::sleep(timeout * 2);
                }
                tx.send(1usize).unwrap();
            }
        });

        let mut recv_count = 0;
        loop {
            match rx.recv_timeout(timeout) {
                Ok(n) => {
                    assert_eq!(n, 1usize);
                    recv_count += 1;
                }
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }

        assert_eq!(recv_count, stress);
    }

    #[test]
    fn recv_timeout_upgrade() {
        let (tx, rx) = channel::<()>();
        let timeout = Duration::from_millis(1);
        let _tx_clone = tx.clone();

        let start = Instant::now();
        assert_eq!(rx.recv_timeout(timeout), Err(RecvTimeoutError::Timeout));
        assert!(Instant::now() >= start + timeout);
    }

    #[test]
    fn stress_recv_timeout_shared() {
        let (tx, rx) = channel();
        let stress = stress_factor() + 100;

        for i in 0..stress {
            let tx = tx.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(i as u64 * 10));
                tx.send(1usize).unwrap();
            });
        }

        drop(tx);

        let mut recv_count = 0;
        loop {
            match rx.recv_timeout(Duration::from_millis(10)) {
                Ok(n) => {
                    assert_eq!(n, 1usize);
                    recv_count += 1;
                }
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }

        assert_eq!(recv_count, stress);
    }

    #[test]
    fn recv_a_lot() {
        // Regression test that we don't run out of stack in scheduler context
        let (tx, rx) = channel();
        for _ in 0..10000 { tx.send(()).unwrap(); }
        for _ in 0..10000 { rx.recv().unwrap(); }
    }

    #[test]
    fn shared_recv_timeout() {
        let (tx, rx) = channel();
        let total = 5;
        for _ in 0..total {
            let tx = tx.clone();
            thread::spawn(move|| {
                tx.send(()).unwrap();
            });
        }

        for _ in 0..total { rx.recv().unwrap(); }

        assert_eq!(rx.recv_timeout(Duration::from_millis(1)), Err(RecvTimeoutError::Timeout));
        tx.send(()).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_millis(1)), Ok(()));
    }

    #[test]
    fn shared_chan_stress() {
        let (tx, rx) = channel();
        let total = stress_factor() + 100;
        for _ in 0..total {
            let tx = tx.clone();
            thread::spawn(move|| {
                tx.send(()).unwrap();
            });
        }

        for _ in 0..total {
            rx.recv().unwrap();
        }
    }

    #[test]
    fn test_nested_recv_iter() {
        let (tx, rx) = channel::<i32>();
        let (total_tx, total_rx) = channel::<i32>();

        let _t = thread::spawn(move|| {
            let mut acc = 0;
            for x in rx.iter() {
                acc += x;
            }
            total_tx.send(acc).unwrap();
        });

        tx.send(3).unwrap();
        tx.send(1).unwrap();
        tx.send(2).unwrap();
        drop(tx);
        assert_eq!(total_rx.recv().unwrap(), 6);
    }

    #[test]
    fn test_recv_iter_break() {
        let (tx, rx) = channel::<i32>();
        let (count_tx, count_rx) = channel();

        let _t = thread::spawn(move|| {
            let mut count = 0;
            for x in rx.iter() {
                if count >= 3 {
                    break;
                } else {
                    count += x;
                }
            }
            count_tx.send(count).unwrap();
        });

        tx.send(2).unwrap();
        tx.send(2).unwrap();
        tx.send(2).unwrap();
        let _ = tx.send(2);
        drop(tx);
        assert_eq!(count_rx.recv().unwrap(), 4);
    }

    #[test]
    fn test_recv_try_iter() {
        let (request_tx, request_rx) = channel();
        let (response_tx, response_rx) = channel();

        // Request `x`s until we have `6`.
        let t = thread::spawn(move|| {
            let mut count = 0;
            loop {
                for x in response_rx.try_iter() {
                    count += x;
                    if count == 6 {
                        return count;
                    }
                }
                request_tx.send(()).unwrap();
            }
        });

        for _ in request_rx.iter() {
            if response_tx.send(2).is_err() {
                break;
            }
        }

        assert_eq!(t.join().unwrap(), 6);
    }

    #[test]
    fn test_recv_into_iter_owned() {
        let mut iter = {
          let (tx, rx) = channel::<i32>();
          tx.send(1).unwrap();
          tx.send(2).unwrap();

          rx.into_iter()
        };
        assert_eq!(iter.next().unwrap(), 1);
        assert_eq!(iter.next().unwrap(), 2);
        assert_eq!(iter.next().is_none(), true);
    }

    #[test]
    fn test_recv_into_iter_borrowed() {
        let (tx, rx) = channel::<i32>();
        tx.send(1).unwrap();
        tx.send(2).unwrap();
        drop(tx);
        let mut iter = (&rx).into_iter();
        assert_eq!(iter.next().unwrap(), 1);
        assert_eq!(iter.next().unwrap(), 2);
        assert_eq!(iter.next().is_none(), true);
    }

    #[test]
    fn try_recv_states() {
        let (tx1, rx1) = channel::<i32>();
        let (tx2, rx2) = channel::<()>();
        let (tx3, rx3) = channel::<()>();
        let _t = thread::spawn(move|| {
            rx2.recv().unwrap();
            tx1.send(1).unwrap();
            tx3.send(()).unwrap();
            rx2.recv().unwrap();
            drop(tx1);
            tx3.send(()).unwrap();
        });

        assert_eq!(rx1.try_recv(), Err(TryRecvError::Empty));
        tx2.send(()).unwrap();
        rx3.recv().unwrap();
        assert_eq!(rx1.try_recv(), Ok(1));
        assert_eq!(rx1.try_recv(), Err(TryRecvError::Empty));
        tx2.send(()).unwrap();
        rx3.recv().unwrap();
        assert_eq!(rx1.try_recv(), Err(TryRecvError::Disconnected));
    }

    // This bug used to end up in a livelock inside of the Receiver destructor
    // because the internal state of the Shared packet was corrupted
    #[test]
    fn destroy_upgraded_shared_port_when_sender_still_active() {
        let (tx, rx) = channel();
        let (tx2, rx2) = channel();
        let _t = thread::spawn(move|| {
            rx.recv().unwrap(); // wait on a oneshot
            drop(rx);  // destroy a shared
            tx2.send(()).unwrap();
        });
        // make sure the other thread has gone to sleep
        for _ in 0..5000 { thread::yield_now(); }

        // upgrade to a shared chan and send a message
        let t = tx.clone();
        drop(tx);
        t.send(()).unwrap();

        // wait for the child thread to exit before we exit
        rx2.recv().unwrap();
    }

    #[test]
    fn issue_32114() {
        let (tx, _) = channel();
        let _ = tx.send(123);
        assert_eq!(tx.send(123), Err(SendError(123)));
    }
}

// https://github.com/rust-lang/rust/tree/master/src/libstd/sync/mpsc
mod sync_channel_tests {
    use std::env;
    use std::thread;
    use super::*;
    use std::time::Duration;

    pub fn stress_factor() -> usize {
        match env::var("RUST_TEST_STRESS") {
            Ok(val) => val.parse().unwrap(),
            Err(..) => 1,
        }
    }

    #[test]
    fn smoke() {
        let (tx, rx) = sync_channel::<i32>(1);
        tx.send(1).unwrap();
        assert_eq!(rx.recv().unwrap(), 1);
    }

    #[test]
    fn drop_full() {
        let (tx, _rx) = sync_channel::<Box<isize>>(1);
        tx.send(Box::new(1)).unwrap();
    }

    #[test]
    fn smoke_shared() {
        let (tx, rx) = sync_channel::<i32>(1);
        tx.send(1).unwrap();
        assert_eq!(rx.recv().unwrap(), 1);
        let tx = tx.clone();
        tx.send(1).unwrap();
        assert_eq!(rx.recv().unwrap(), 1);
    }

    #[test]
    fn recv_timeout() {
        let (tx, rx) = sync_channel::<i32>(1);
        assert_eq!(rx.recv_timeout(Duration::from_millis(1)), Err(RecvTimeoutError::Timeout));
        tx.send(1).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_millis(1)), Ok(1));
    }

    #[test]
    fn smoke_threads() {
        let (tx, rx) = sync_channel::<i32>(0);
        let _t = thread::spawn(move|| {
            tx.send(1).unwrap();
        });
        assert_eq!(rx.recv().unwrap(), 1);
    }

    #[test]
    fn smoke_port_gone() {
        let (tx, rx) = sync_channel::<i32>(0);
        drop(rx);
        assert!(tx.send(1).is_err());
    }

    #[test]
    fn smoke_shared_port_gone2() {
        let (tx, rx) = sync_channel::<i32>(0);
        drop(rx);
        let tx2 = tx.clone();
        drop(tx);
        assert!(tx2.send(1).is_err());
    }

    #[test]
    fn port_gone_concurrent() {
        let (tx, rx) = sync_channel::<i32>(0);
        let _t = thread::spawn(move|| {
            rx.recv().unwrap();
        });
        while tx.send(1).is_ok() {}
    }

    #[test]
    fn port_gone_concurrent_shared() {
        let (tx, rx) = sync_channel::<i32>(0);
        let tx2 = tx.clone();
        let _t = thread::spawn(move|| {
            rx.recv().unwrap();
        });
        while tx.send(1).is_ok() && tx2.send(1).is_ok() {}
    }

    #[test]
    fn smoke_chan_gone() {
        let (tx, rx) = sync_channel::<i32>(0);
        drop(tx);
        assert!(rx.recv().is_err());
    }

    #[test]
    fn smoke_chan_gone_shared() {
        let (tx, rx) = sync_channel::<()>(0);
        let tx2 = tx.clone();
        drop(tx);
        drop(tx2);
        assert!(rx.recv().is_err());
    }

    #[test]
    fn chan_gone_concurrent() {
        let (tx, rx) = sync_channel::<i32>(0);
        thread::spawn(move|| {
            tx.send(1).unwrap();
            tx.send(1).unwrap();
        });
        while rx.recv().is_ok() {}
    }

    #[test]
    fn stress() {
        let (tx, rx) = sync_channel::<i32>(0);
        thread::spawn(move|| {
            for _ in 0..10000 { tx.send(1).unwrap(); }
        });
        for _ in 0..10000 {
            assert_eq!(rx.recv().unwrap(), 1);
        }
    }

    #[test]
    fn stress_recv_timeout_two_threads() {
        let (tx, rx) = sync_channel::<i32>(0);

        thread::spawn(move|| {
            for _ in 0..10000 { tx.send(1).unwrap(); }
        });

        let mut recv_count = 0;
        loop {
            match rx.recv_timeout(Duration::from_millis(1)) {
                Ok(v) => {
                    assert_eq!(v, 1);
                    recv_count += 1;
                },
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }

        assert_eq!(recv_count, 10000);
    }

    #[test]
    fn stress_recv_timeout_shared() {
        const AMT: u32 = 1000;
        const NTHREADS: u32 = 8;
        let (tx, rx) = sync_channel::<i32>(0);
        let (dtx, drx) = sync_channel::<()>(0);

        thread::spawn(move|| {
            let mut recv_count = 0;
            loop {
                match rx.recv_timeout(Duration::from_millis(10)) {
                    Ok(v) => {
                        assert_eq!(v, 1);
                        recv_count += 1;
                    },
                    Err(RecvTimeoutError::Timeout) => continue,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }

            assert_eq!(recv_count, AMT * NTHREADS);
            assert!(rx.try_recv().is_err());

            dtx.send(()).unwrap();
        });

        for _ in 0..NTHREADS {
            let tx = tx.clone();
            thread::spawn(move|| {
                for _ in 0..AMT { tx.send(1).unwrap(); }
            });
        }

        drop(tx);

        drx.recv().unwrap();
    }

    #[test]
    fn stress_shared() {
        const AMT: u32 = 1000;
        const NTHREADS: u32 = 8;
        let (tx, rx) = sync_channel::<i32>(0);
        let (dtx, drx) = sync_channel::<()>(0);

        thread::spawn(move|| {
            for _ in 0..AMT * NTHREADS {
                assert_eq!(rx.recv().unwrap(), 1);
            }
            match rx.try_recv() {
                Ok(..) => panic!(),
                _ => {}
            }
            dtx.send(()).unwrap();
        });

        for _ in 0..NTHREADS {
            let tx = tx.clone();
            thread::spawn(move|| {
                for _ in 0..AMT { tx.send(1).unwrap(); }
            });
        }
        drop(tx);
        drx.recv().unwrap();
    }

    #[test]
    fn oneshot_single_thread_close_port_first() {
        // Simple test of closing without sending
        let (_tx, rx) = sync_channel::<i32>(0);
        drop(rx);
    }

    #[test]
    fn oneshot_single_thread_close_chan_first() {
        // Simple test of closing without sending
        let (tx, _rx) = sync_channel::<i32>(0);
        drop(tx);
    }

    #[test]
    fn oneshot_single_thread_send_port_close() {
        // Testing that the sender cleans up the payload if receiver is closed
        let (tx, rx) = sync_channel::<Box<i32>>(0);
        drop(rx);
        assert!(tx.send(Box::new(0)).is_err());
    }

    #[test]
    fn oneshot_single_thread_recv_chan_close() {
        // Receiving on a closed chan will panic
        let res = thread::spawn(move|| {
            let (tx, rx) = sync_channel::<i32>(0);
            drop(tx);
            rx.recv().unwrap();
        }).join();
        // What is our res?
        assert!(res.is_err());
    }

    #[test]
    fn oneshot_single_thread_send_then_recv() {
        let (tx, rx) = sync_channel::<Box<i32>>(1);
        tx.send(Box::new(10)).unwrap();
        assert!(*rx.recv().unwrap() == 10);
    }

    #[test]
    fn oneshot_single_thread_try_send_open() {
        let (tx, rx) = sync_channel::<i32>(1);
        assert_eq!(tx.try_send(10), Ok(()));
        assert!(rx.recv().unwrap() == 10);
    }

    #[test]
    fn oneshot_single_thread_try_send_closed() {
        let (tx, rx) = sync_channel::<i32>(0);
        drop(rx);
        assert_eq!(tx.try_send(10), Err(TrySendError::Disconnected(10)));
    }

    #[test]
    fn oneshot_single_thread_try_send_closed2() {
        let (tx, _rx) = sync_channel::<i32>(0);
        assert_eq!(tx.try_send(10), Err(TrySendError::Full(10)));
    }

    #[test]
    fn oneshot_single_thread_try_recv_open() {
        let (tx, rx) = sync_channel::<i32>(1);
        tx.send(10).unwrap();
        assert!(rx.recv() == Ok(10));
    }

    #[test]
    fn oneshot_single_thread_try_recv_closed() {
        let (tx, rx) = sync_channel::<i32>(0);
        drop(tx);
        assert!(rx.recv().is_err());
    }

    #[test]
    fn oneshot_single_thread_try_recv_closed_with_data() {
        let (tx, rx) = sync_channel::<i32>(1);
        tx.send(10).unwrap();
        drop(tx);
        assert_eq!(rx.try_recv(), Ok(10));
        assert_eq!(rx.try_recv(), Err(TryRecvError::Disconnected));
    }

    #[test]
    fn oneshot_single_thread_peek_data() {
        let (tx, rx) = sync_channel::<i32>(1);
        assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
        tx.send(10).unwrap();
        assert_eq!(rx.try_recv(), Ok(10));
    }

    #[test]
    fn oneshot_single_thread_peek_close() {
        let (tx, rx) = sync_channel::<i32>(0);
        drop(tx);
        assert_eq!(rx.try_recv(), Err(TryRecvError::Disconnected));
        assert_eq!(rx.try_recv(), Err(TryRecvError::Disconnected));
    }

    #[test]
    fn oneshot_single_thread_peek_open() {
        let (_tx, rx) = sync_channel::<i32>(0);
        assert_eq!(rx.try_recv(), Err(TryRecvError::Empty));
    }

    #[test]
    fn oneshot_multi_task_recv_then_send() {
        let (tx, rx) = sync_channel::<Box<i32>>(0);
        let _t = thread::spawn(move|| {
            assert!(*rx.recv().unwrap() == 10);
        });

        tx.send(Box::new(10)).unwrap();
    }

    #[test]
    fn oneshot_multi_task_recv_then_close() {
        let (tx, rx) = sync_channel::<Box<i32>>(0);
        let _t = thread::spawn(move|| {
            drop(tx);
        });
        let res = thread::spawn(move|| {
            assert!(*rx.recv().unwrap() == 10);
        }).join();
        assert!(res.is_err());
    }

    #[test]
    fn oneshot_multi_thread_close_stress() {
        for _ in 0..stress_factor() {
            let (tx, rx) = sync_channel::<i32>(0);
            let _t = thread::spawn(move|| {
                drop(rx);
            });
            drop(tx);
        }
    }

    #[test]
    fn oneshot_multi_thread_send_close_stress() {
        for _ in 0..stress_factor() {
            let (tx, rx) = sync_channel::<i32>(0);
            let _t = thread::spawn(move|| {
                drop(rx);
            });
            let _ = thread::spawn(move || {
                tx.send(1).unwrap();
            }).join();
        }
    }

    #[test]
    fn oneshot_multi_thread_recv_close_stress() {
        for _ in 0..stress_factor() {
            let (tx, rx) = sync_channel::<i32>(0);
            let _t = thread::spawn(move|| {
                let res = thread::spawn(move|| {
                    rx.recv().unwrap();
                }).join();
                assert!(res.is_err());
            });
            let _t = thread::spawn(move|| {
                thread::spawn(move|| {
                    drop(tx);
                });
            });
        }
    }

    #[test]
    fn oneshot_multi_thread_send_recv_stress() {
        for _ in 0..stress_factor() {
            let (tx, rx) = sync_channel::<Box<i32>>(0);
            let _t = thread::spawn(move|| {
                tx.send(Box::new(10)).unwrap();
            });
            assert!(*rx.recv().unwrap() == 10);
        }
    }

    #[test]
    fn stream_send_recv_stress() {
        for _ in 0..stress_factor() {
            let (tx, rx) = sync_channel::<Box<i32>>(0);

            send(tx, 0);
            recv(rx, 0);

            fn send(tx: SyncSender<Box<i32>>, i: i32) {
                if i == 10 { return }

                thread::spawn(move|| {
                    tx.send(Box::new(i)).unwrap();
                    send(tx, i + 1);
                });
            }

            fn recv(rx: Receiver<Box<i32>>, i: i32) {
                if i == 10 { return }

                thread::spawn(move|| {
                    assert!(*rx.recv().unwrap() == i);
                    recv(rx, i + 1);
                });
            }
        }
    }

    #[test]
    fn recv_a_lot() {
        // Regression test that we don't run out of stack in scheduler context
        let (tx, rx) = sync_channel(10000);
        for _ in 0..10000 { tx.send(()).unwrap(); }
        for _ in 0..10000 { rx.recv().unwrap(); }
    }

    #[test]
    fn shared_chan_stress() {
        let (tx, rx) = sync_channel(0);
        let total = stress_factor() + 100;
        for _ in 0..total {
            let tx = tx.clone();
            thread::spawn(move|| {
                tx.send(()).unwrap();
            });
        }

        for _ in 0..total {
            rx.recv().unwrap();
        }
    }

    #[test]
    fn test_nested_recv_iter() {
        let (tx, rx) = sync_channel::<i32>(0);
        let (total_tx, total_rx) = sync_channel::<i32>(0);

        let _t = thread::spawn(move|| {
            let mut acc = 0;
            for x in rx.iter() {
                acc += x;
            }
            total_tx.send(acc).unwrap();
        });

        tx.send(3).unwrap();
        tx.send(1).unwrap();
        tx.send(2).unwrap();
        drop(tx);
        assert_eq!(total_rx.recv().unwrap(), 6);
    }

    #[test]
    fn test_recv_iter_break() {
        let (tx, rx) = sync_channel::<i32>(0);
        let (count_tx, count_rx) = sync_channel(0);

        let _t = thread::spawn(move|| {
            let mut count = 0;
            for x in rx.iter() {
                if count >= 3 {
                    break;
                } else {
                    count += x;
                }
            }
            count_tx.send(count).unwrap();
        });

        tx.send(2).unwrap();
        tx.send(2).unwrap();
        tx.send(2).unwrap();
        let _ = tx.try_send(2);
        drop(tx);
        assert_eq!(count_rx.recv().unwrap(), 4);
    }

    #[test]
    fn try_recv_states() {
        let (tx1, rx1) = sync_channel::<i32>(1);
        let (tx2, rx2) = sync_channel::<()>(1);
        let (tx3, rx3) = sync_channel::<()>(1);
        let _t = thread::spawn(move|| {
            rx2.recv().unwrap();
            tx1.send(1).unwrap();
            tx3.send(()).unwrap();
            rx2.recv().unwrap();
            drop(tx1);
            tx3.send(()).unwrap();
        });

        assert_eq!(rx1.try_recv(), Err(TryRecvError::Empty));
        tx2.send(()).unwrap();
        rx3.recv().unwrap();
        assert_eq!(rx1.try_recv(), Ok(1));
        assert_eq!(rx1.try_recv(), Err(TryRecvError::Empty));
        tx2.send(()).unwrap();
        rx3.recv().unwrap();
        assert_eq!(rx1.try_recv(), Err(TryRecvError::Disconnected));
    }

    // This bug used to end up in a livelock inside of the Receiver destructor
    // because the internal state of the Shared packet was corrupted
    #[test]
    fn destroy_upgraded_shared_port_when_sender_still_active() {
        let (tx, rx) = sync_channel::<()>(0);
        let (tx2, rx2) = sync_channel::<()>(0);
        let _t = thread::spawn(move|| {
            rx.recv().unwrap(); // wait on a oneshot
            drop(rx);  // destroy a shared
            tx2.send(()).unwrap();
        });
        // make sure the other thread has gone to sleep
        for _ in 0..5000 { thread::yield_now(); }

        // upgrade to a shared chan and send a message
        let t = tx.clone();
        drop(tx);
        t.send(()).unwrap();

        // wait for the child thread to exit before we exit
        rx2.recv().unwrap();
    }

    #[test]
    fn send1() {
        let (tx, rx) = sync_channel::<i32>(0);
        let _t = thread::spawn(move|| { rx.recv().unwrap(); });
        assert_eq!(tx.send(1), Ok(()));
    }

    #[test]
    fn send2() {
        let (tx, rx) = sync_channel::<i32>(0);
        let _t = thread::spawn(move|| { drop(rx); });
        assert!(tx.send(1).is_err());
    }

    #[test]
    fn send3() {
        let (tx, rx) = sync_channel::<i32>(1);
        assert_eq!(tx.send(1), Ok(()));
        let _t =thread::spawn(move|| { drop(rx); });
        assert!(tx.send(1).is_err());
    }

    #[test]
    fn send4() {
        let (tx, rx) = sync_channel::<i32>(0);
        let tx2 = tx.clone();
        let (done, donerx) = channel();
        let done2 = done.clone();
        let _t = thread::spawn(move|| {
            assert!(tx.send(1).is_err());
            done.send(()).unwrap();
        });
        let _t = thread::spawn(move|| {
            assert!(tx2.send(2).is_err());
            done2.send(()).unwrap();
        });
        drop(rx);
        donerx.recv().unwrap();
        donerx.recv().unwrap();
    }

    #[test]
    fn try_send1() {
        let (tx, _rx) = sync_channel::<i32>(0);
        assert_eq!(tx.try_send(1), Err(TrySendError::Full(1)));
    }

    #[test]
    fn try_send2() {
        let (tx, _rx) = sync_channel::<i32>(1);
        assert_eq!(tx.try_send(1), Ok(()));
        assert_eq!(tx.try_send(1), Err(TrySendError::Full(1)));
    }

    #[test]
    fn try_send3() {
        let (tx, rx) = sync_channel::<i32>(1);
        assert_eq!(tx.try_send(1), Ok(()));
        drop(rx);
        assert_eq!(tx.try_send(1), Err(TrySendError::Disconnected(1)));
    }

    #[test]
    fn issue_15761() {
        fn repro() {
            let (tx1, rx1) = sync_channel::<()>(3);
            let (tx2, rx2) = sync_channel::<()>(3);

            let _t = thread::spawn(move|| {
                rx1.recv().unwrap();
                tx2.try_send(()).unwrap();
            });

            tx1.try_send(()).unwrap();
            rx2.recv().unwrap();
        }

        for _ in 0..100 {
            repro()
        }
    }
}

// https://github.com/rust-lang/rust/blob/master/src/libstd/sync/mpsc/select.rs
mod select_tests {
    use std::thread;
    use super::*;

    macro_rules! mpsc_select {
        (
            $($name:pat = $rx:ident.$meth:ident() => $code:expr),+
        ) => ({
            select! {
                $(
                    recv(($rx).inner, msg) => {
                        let $name = match msg {
                            None => Err(::std::sync::mpsc::RecvError),
                            Some(msg) => Ok(msg),
                        };
                        $code
                    }
                )+
            }
        })
    }

    #[test]
    fn smoke() {
        let (tx1, rx1) = channel::<i32>();
        let (tx2, rx2) = channel::<i32>();
        tx1.send(1).unwrap();
        mpsc_select! {
            foo = rx1.recv() => { assert_eq!(foo.unwrap(), 1); },
            _bar = rx2.recv() => { panic!() }
        }
        tx2.send(2).unwrap();
        mpsc_select! {
            _foo = rx1.recv() => { panic!() },
            bar = rx2.recv() => { assert_eq!(bar.unwrap(), 2) }
        }
        drop(tx1);
        mpsc_select! {
            foo = rx1.recv() => { assert!(foo.is_err()); },
            _bar = rx2.recv() => { panic!() }
        }
        drop(tx2);
        mpsc_select! {
            bar = rx2.recv() => { assert!(bar.is_err()); }
        }
    }

    #[test]
    fn smoke2() {
        let (_tx1, rx1) = channel::<i32>();
        let (_tx2, rx2) = channel::<i32>();
        let (_tx3, rx3) = channel::<i32>();
        let (_tx4, rx4) = channel::<i32>();
        let (tx5, rx5) = channel::<i32>();
        tx5.send(4).unwrap();
        mpsc_select! {
            _foo = rx1.recv() => { panic!("1") },
            _foo = rx2.recv() => { panic!("2") },
            _foo = rx3.recv() => { panic!("3") },
            _foo = rx4.recv() => { panic!("4") },
            foo = rx5.recv() => { assert_eq!(foo.unwrap(), 4); }
        }
    }

    #[test]
    fn closed() {
        let (_tx1, rx1) = channel::<i32>();
        let (tx2, rx2) = channel::<i32>();
        drop(tx2);

        mpsc_select! {
            _a1 = rx1.recv() => { panic!() },
            a2 = rx2.recv() => { assert!(a2.is_err()); }
        }
    }

    #[test]
    fn unblocks() {
        let (tx1, rx1) = channel::<i32>();
        let (_tx2, rx2) = channel::<i32>();
        let (tx3, rx3) = channel::<i32>();

        let _t = thread::spawn(move|| {
            for _ in 0..20 { thread::yield_now(); }
            tx1.send(1).unwrap();
            rx3.recv().unwrap();
            for _ in 0..20 { thread::yield_now(); }
        });

        mpsc_select! {
            a = rx1.recv() => { assert_eq!(a.unwrap(), 1); },
            _b = rx2.recv() => { panic!() }
        }
        tx3.send(1).unwrap();
        mpsc_select! {
            a = rx1.recv() => { assert!(a.is_err()) },
            _b = rx2.recv() => { panic!() }
        }
    }

    #[test]
    fn both_ready() {
        let (tx1, rx1) = channel::<i32>();
        let (tx2, rx2) = channel::<i32>();
        let (tx3, rx3) = channel::<()>();

        let _t = thread::spawn(move|| {
            for _ in 0..20 { thread::yield_now(); }
            tx1.send(1).unwrap();
            tx2.send(2).unwrap();
            rx3.recv().unwrap();
        });

        mpsc_select! {
            a = rx1.recv() => { assert_eq!(a.unwrap(), 1); },
            a = rx2.recv() => { assert_eq!(a.unwrap(), 2); }
        }
        mpsc_select! {
            a = rx1.recv() => { assert_eq!(a.unwrap(), 1); },
            a = rx2.recv() => { assert_eq!(a.unwrap(), 2); }
        }
        assert_eq!(rx1.try_recv(), Err(TryRecvError::Empty));
        assert_eq!(rx2.try_recv(), Err(TryRecvError::Empty));
        tx3.send(()).unwrap();
    }

    #[test]
    fn stress() {
        const AMT: i32 = 10000;
        let (tx1, rx1) = channel::<i32>();
        let (tx2, rx2) = channel::<i32>();
        let (tx3, rx3) = channel::<()>();

        let _t = thread::spawn(move|| {
            for i in 0..AMT {
                if i % 2 == 0 {
                    tx1.send(i).unwrap();
                } else {
                    tx2.send(i).unwrap();
                }
                rx3.recv().unwrap();
            }
        });

        for i in 0..AMT {
            mpsc_select! {
                i1 = rx1.recv() => { assert!(i % 2 == 0 && i == i1.unwrap()); },
                i2 = rx2.recv() => { assert!(i % 2 == 1 && i == i2.unwrap()); }
            }
            tx3.send(()).unwrap();
        }
    }

    #[allow(unused_must_use)]
    #[test]
    fn cloning() {
        let (tx1, rx1) = channel::<i32>();
        let (_tx2, rx2) = channel::<i32>();
        let (tx3, rx3) = channel::<()>();

        let _t = thread::spawn(move|| {
            rx3.recv().unwrap();
            tx1.clone();
            assert_eq!(rx3.try_recv(), Err(TryRecvError::Empty));
            tx1.send(2).unwrap();
            rx3.recv().unwrap();
        });

        tx3.send(()).unwrap();
        mpsc_select! {
            _i1 = rx1.recv() => {},
            _i2 = rx2.recv() => panic!()
        }
        tx3.send(()).unwrap();
    }

    #[allow(unused_must_use)]
    #[test]
    fn cloning2() {
        let (tx1, rx1) = channel::<i32>();
        let (_tx2, rx2) = channel::<i32>();
        let (tx3, rx3) = channel::<()>();

        let _t = thread::spawn(move|| {
            rx3.recv().unwrap();
            tx1.clone();
            assert_eq!(rx3.try_recv(), Err(TryRecvError::Empty));
            tx1.send(2).unwrap();
            rx3.recv().unwrap();
        });

        tx3.send(()).unwrap();
        mpsc_select! {
            _i1 = rx1.recv() => {},
            _i2 = rx2.recv() => panic!()
        }
        tx3.send(()).unwrap();
    }

    #[test]
    fn cloning3() {
        let (tx1, rx1) = channel::<()>();
        let (tx2, rx2) = channel::<()>();
        let (tx3, rx3) = channel::<()>();
        let _t = thread::spawn(move|| {
            mpsc_select! {
                _ = rx1.recv() => panic!(),
                _ = rx2.recv() => {}
            }
            tx3.send(()).unwrap();
        });

        for _ in 0..1000 { thread::yield_now(); }
        drop(tx1.clone());
        tx2.send(()).unwrap();
        rx3.recv().unwrap();
    }

    #[test]
    fn preflight1() {
        let (tx, rx) = channel();
        tx.send(()).unwrap();
        mpsc_select! {
            _n = rx.recv() => {}
        }
    }

    #[test]
    fn preflight2() {
        let (tx, rx) = channel();
        tx.send(()).unwrap();
        tx.send(()).unwrap();
        mpsc_select! {
            _n = rx.recv() => {}
        }
    }

    #[test]
    fn preflight3() {
        let (tx, rx) = channel();
        drop(tx.clone());
        tx.send(()).unwrap();
        mpsc_select! {
            _n = rx.recv() => {}
        }
    }

    #[test]
    fn preflight4() {
        let (tx, rx) = channel();
        tx.send(()).unwrap();
        mpsc_select! {
            _ = rx.recv() => {}
        }
    }

    #[test]
    fn preflight5() {
        let (tx, rx) = channel();
        tx.send(()).unwrap();
        tx.send(()).unwrap();
        mpsc_select! {
            _ = rx.recv() => {}
        }
    }

    #[test]
    fn preflight6() {
        let (tx, rx) = channel();
        drop(tx.clone());
        tx.send(()).unwrap();
        mpsc_select! {
            _ = rx.recv() => {}
        }
    }

    #[test]
    fn preflight7() {
        let (tx, rx) = channel::<()>();
        drop(tx);
        mpsc_select! {
            _ = rx.recv() => {}
        }
    }

    #[test]
    fn preflight8() {
        let (tx, rx) = channel();
        tx.send(()).unwrap();
        drop(tx);
        rx.recv().unwrap();
        mpsc_select! {
            _ = rx.recv() => {}
        }
    }

    #[test]
    fn preflight9() {
        let (tx, rx) = channel();
        drop(tx.clone());
        tx.send(()).unwrap();
        drop(tx);
        rx.recv().unwrap();
        mpsc_select! {
            _ = rx.recv() => {}
        }
    }

    #[test]
    fn oneshot_data_waiting() {
        let (tx1, rx1) = channel();
        let (tx2, rx2) = channel();
        let _t = thread::spawn(move|| {
            mpsc_select! {
                _n = rx1.recv() => {}
            }
            tx2.send(()).unwrap();
        });

        for _ in 0..100 { thread::yield_now() }
        tx1.send(()).unwrap();
        rx2.recv().unwrap();
    }

    #[test]
    fn stream_data_waiting() {
        let (tx1, rx1) = channel();
        let (tx2, rx2) = channel();
        tx1.send(()).unwrap();
        tx1.send(()).unwrap();
        rx1.recv().unwrap();
        rx1.recv().unwrap();
        let _t = thread::spawn(move|| {
            mpsc_select! {
                _n = rx1.recv() => {}
            }
            tx2.send(()).unwrap();
        });

        for _ in 0..100 { thread::yield_now() }
        tx1.send(()).unwrap();
        rx2.recv().unwrap();
    }

    #[test]
    fn shared_data_waiting() {
        let (tx1, rx1) = channel();
        let (tx2, rx2) = channel();
        drop(tx1.clone());
        tx1.send(()).unwrap();
        rx1.recv().unwrap();
        let _t = thread::spawn(move|| {
            mpsc_select! {
                _n = rx1.recv() => {}
            }
            tx2.send(()).unwrap();
        });

        for _ in 0..100 { thread::yield_now() }
        tx1.send(()).unwrap();
        rx2.recv().unwrap();
    }

    #[test]
    fn sync1() {
        let (tx, rx) = sync_channel::<i32>(1);
        tx.send(1).unwrap();
        mpsc_select! {
            n = rx.recv() => { assert_eq!(n.unwrap(), 1); }
        }
    }

    #[test]
    fn sync2() {
        let (tx, rx) = sync_channel::<i32>(0);
        let _t = thread::spawn(move|| {
            for _ in 0..100 { thread::yield_now() }
            tx.send(1).unwrap();
        });
        mpsc_select! {
            n = rx.recv() => { assert_eq!(n.unwrap(), 1); }
        }
    }

    #[test]
    fn sync3() {
        let (tx1, rx1) = sync_channel::<i32>(0);
        let (tx2, rx2): (Sender<i32>, Receiver<i32>) = channel();
        let _t = thread::spawn(move|| { tx1.send(1).unwrap(); });
        let _t = thread::spawn(move|| { tx2.send(2).unwrap(); });
        mpsc_select! {
            n = rx1.recv() => {
                let n = n.unwrap();
                assert_eq!(n, 1);
                assert_eq!(rx2.recv().unwrap(), 2);
            },
            n = rx2.recv() => {
                let n = n.unwrap();
                assert_eq!(n, 2);
                assert_eq!(rx1.recv().unwrap(), 1);
            }
        }
    }
}
