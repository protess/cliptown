use std::sync::Arc;
use std::time::SystemTime;
use uuid::Uuid;

pub trait Clock: Send + Sync { fn now_unix(&self) -> i64; }
pub trait Randomness: Send + Sync { fn next_u32(&self) -> u32; }
pub trait UuidGen: Send + Sync { fn new(&self) -> String; }

pub struct ProdClock;
impl Clock for ProdClock {
    fn now_unix(&self) -> i64 {
        SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs() as i64
    }
}
pub struct ProdRandom;
impl Randomness for ProdRandom { fn next_u32(&self) -> u32 { rand::random() } }
pub struct ProdUuid;
impl UuidGen for ProdUuid { fn new(&self) -> String { Uuid::new_v4().to_string() } }

#[derive(Clone)]
pub struct DetCtx {
    pub clock: Arc<dyn Clock>,
    pub random: Arc<dyn Randomness>,
    pub uuid: Arc<dyn UuidGen>,
}
impl DetCtx {
    pub fn prod() -> Self { Self { clock: Arc::new(ProdClock), random: Arc::new(ProdRandom), uuid: Arc::new(ProdUuid) } }
}

#[cfg(any(test, feature = "test_det"))]
pub mod testing {
    use super::*;
    use std::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering};
    pub struct FakeClock(AtomicI64);
    impl FakeClock { pub fn at(t: i64) -> Self { Self(AtomicI64::new(t)) } pub fn advance(&self, by: i64) { self.0.fetch_add(by, Ordering::SeqCst); } }
    impl Clock for FakeClock { fn now_unix(&self) -> i64 { self.0.load(Ordering::SeqCst) } }
    pub struct SeededRandom(AtomicU32);
    impl SeededRandom { pub fn seed(s: u32) -> Self { Self(AtomicU32::new(s)) } }
    impl Randomness for SeededRandom {
        fn next_u32(&self) -> u32 {
            // xorshift32
            let mut x = self.0.load(Ordering::SeqCst);
            x ^= x << 13; x ^= x >> 17; x ^= x << 5;
            self.0.store(x, Ordering::SeqCst);
            x
        }
    }
    pub struct SeededUuid(AtomicU64);
    impl SeededUuid { pub fn seed(s: u64) -> Self { Self(AtomicU64::new(s)) } }
    impl UuidGen for SeededUuid {
        fn new(&self) -> String {
            let n = self.0.fetch_add(1, Ordering::SeqCst);
            format!("00000000-0000-0000-0000-{:012x}", n)
        }
    }
    pub fn ctx(t0: i64, seed: u32) -> DetCtx {
        DetCtx {
            clock: Arc::new(FakeClock::at(t0)),
            random: Arc::new(SeededRandom::seed(seed)),
            uuid: Arc::new(SeededUuid::seed(0)),
        }
    }
}
