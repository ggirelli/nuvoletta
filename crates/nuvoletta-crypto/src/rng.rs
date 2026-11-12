//! Randomness source. Production code only ever uses the OS CSPRNG; the
//! deterministic source exists for tests only.

use crate::error::{Error, Result};

/// Source of random bytes (crate-private: callers cannot select a non-OS RNG).
pub(crate) trait RandomSource {
    fn fill(&mut self, buf: &mut [u8]) -> Result<()>;

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut a = [0u8; N];
        self.fill(&mut a)?;
        Ok(a)
    }
}

/// The OS CSPRNG.
pub(crate) struct OsRandom;

impl RandomSource for OsRandom {
    fn fill(&mut self, buf: &mut [u8]) -> Result<()> {
        getrandom::fill(buf).map_err(|_| Error::Rng)
    }
}

/// Deterministic SHA-256 counter stream, tests only.
#[cfg(test)]
pub(crate) struct DeterministicRandom {
    seed: [u8; 32],
    counter: u64,
    buf: Vec<u8>,
}

#[cfg(test)]
impl DeterministicRandom {
    pub(crate) fn new(label: &str) -> Self {
        use sha2::{Digest, Sha256};
        Self {
            seed: Sha256::digest(label.as_bytes()).into(),
            counter: 0,
            buf: Vec::new(),
        }
    }
}

#[cfg(test)]
impl RandomSource for DeterministicRandom {
    fn fill(&mut self, out: &mut [u8]) -> Result<()> {
        use sha2::{Digest, Sha256};
        for b in out.iter_mut() {
            if self.buf.is_empty() {
                let mut h = Sha256::new();
                h.update(self.seed);
                h.update(self.counter.to_be_bytes());
                self.counter += 1;
                self.buf = h.finalize().to_vec();
                self.buf.reverse();
            }
            *b = self.buf.pop().unwrap_or(0);
        }
        Ok(())
    }
}
