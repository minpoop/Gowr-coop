//! Reading and writing the game's memory without crashing it.
//!
//! On Windows, [`ProcessMemory`] goes through `ReadProcessMemory` on our own process,
//! so a wrong pointer returns `None` instead of taking the game down.

use coop_protocol::{Quat, Vec3};
use std::collections::BTreeMap;

pub trait Memory {
    fn read_into(&self, addr: usize, buf: &mut [u8]) -> bool;
    fn write_from(&self, addr: usize, data: &[u8]) -> bool;

    fn read_u32(&self, addr: usize) -> Option<u32> {
        let mut b = [0; 4];
        self.read_into(addr, &mut b).then(|| u32::from_le_bytes(b))
    }
    fn read_u64(&self, addr: usize) -> Option<u64> {
        let mut b = [0; 8];
        self.read_into(addr, &mut b).then(|| u64::from_le_bytes(b))
    }
    fn read_f32(&self, addr: usize) -> Option<f32> {
        self.read_u32(addr).map(f32::from_bits).filter(|v| v.is_finite())
    }
    fn read_vec3(&self, addr: usize) -> Option<Vec3> {
        Some(Vec3::new(self.read_f32(addr)?, self.read_f32(addr + 4)?, self.read_f32(addr + 8)?))
    }
    fn read_quat(&self, addr: usize) -> Option<Quat> {
        Some(Quat {
            x: self.read_f32(addr)?,
            y: self.read_f32(addr + 4)?,
            z: self.read_f32(addr + 8)?,
            w: self.read_f32(addr + 12)?,
        })
    }
    fn read_ptr(&self, addr: usize) -> Option<usize> {
        self.read_u64(addr).map(|p| p as usize).filter(|&p| p >= 0x10000)
    }
    fn write_f32(&self, addr: usize, v: f32) -> bool {
        self.write_from(addr, &v.to_le_bytes())
    }
    fn write_vec3(&self, addr: usize, v: Vec3) -> bool {
        let mut b = [0u8; 12];
        b[..4].copy_from_slice(&v.x.to_le_bytes());
        b[4..8].copy_from_slice(&v.y.to_le_bytes());
        b[8..].copy_from_slice(&v.z.to_le_bytes());
        self.write_from(addr, &b)
    }
    fn write_quat(&self, addr: usize, q: Quat) -> bool {
        let mut b = [0u8; 16];
        for (i, v) in [q.x, q.y, q.z, q.w].iter().enumerate() {
            b[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        self.write_from(addr, &b)
    }

    /// Follow a Cheat Engine-style pointer chain.
    ///
    /// `base` holds a pointer. Each offset but the last is added and dereferenced again;
    /// the last is just added. `[0x10, 0x28]` means `*(*base + 0x10) + 0x28`.
    /// With no offsets, `base` itself is the address.
    fn follow(&self, base: usize, chain: &[u64]) -> Option<usize> {
        let Some((last, rest)) = chain.split_last() else { return Some(base) };
        let mut addr = self.read_ptr(base)?;
        for off in rest {
            addr = self.read_ptr(addr.checked_add(*off as usize)?)?;
        }
        addr.checked_add(*last as usize)
    }
}

/// Memory for tests: a few byte regions at chosen addresses.
#[derive(Default)]
pub struct FakeMemory {
    regions: std::cell::RefCell<BTreeMap<usize, Vec<u8>>>,
}

impl FakeMemory {
    pub fn map(&self, at: usize, bytes: Vec<u8>) {
        self.regions.borrow_mut().insert(at, bytes);
    }

    fn with_region<R>(&self, addr: usize, len: usize, f: impl FnOnce(&mut [u8]) -> R) -> Option<R> {
        let mut regions = self.regions.borrow_mut();
        let (&start, bytes) = regions.range_mut(..=addr).next_back()?;
        let off = addr - start;
        let slice = bytes.get_mut(off..off.checked_add(len)?)?;
        Some(f(slice))
    }
}

impl Memory for FakeMemory {
    fn read_into(&self, addr: usize, buf: &mut [u8]) -> bool {
        self.with_region(addr, buf.len(), |s| buf.copy_from_slice(s)).is_some()
    }
    fn write_from(&self, addr: usize, data: &[u8]) -> bool {
        self.with_region(addr, data.len(), |s| s.copy_from_slice(data)).is_some()
    }
}

#[cfg(windows)]
pub use win::ProcessMemory;

#[cfg(windows)]
mod win {
    use super::Memory;
    use windows_sys::Win32::System::Diagnostics::Debug::{ReadProcessMemory, WriteProcessMemory};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    /// This process's memory, read and written in a way that fails instead of crashing.
    pub struct ProcessMemory;

    impl Memory for ProcessMemory {
        fn read_into(&self, addr: usize, buf: &mut [u8]) -> bool {
            let mut done = 0usize;
            // SAFETY: ReadProcessMemory validates the source range and fails on bad pages.
            let ok = unsafe {
                ReadProcessMemory(GetCurrentProcess(), addr as _, buf.as_mut_ptr() as _, buf.len(), &mut done)
            };
            ok != 0 && done == buf.len()
        }
        fn write_from(&self, addr: usize, data: &[u8]) -> bool {
            let mut done = 0usize;
            // SAFETY: as above; the target range is validated by the OS.
            let ok = unsafe {
                WriteProcessMemory(GetCurrentProcess(), addr as _, data.as_ptr() as _, data.len(), &mut done)
            };
            ok != 0 && done == data.len()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_chain() {
        let m = FakeMemory::default();
        let mut stat = vec![0u8; 8];
        stat.copy_from_slice(&0x2_0000u64.to_le_bytes());
        m.map(0x1_0000, stat); // static pointer -> object A
        let mut a = vec![0u8; 0x20];
        a[0x10..0x18].copy_from_slice(&0x3_0000u64.to_le_bytes());
        m.map(0x2_0000, a); // A+0x10 -> object B
        m.map(0x3_0000, vec![0u8; 0x40]);
        assert_eq!(m.follow(0x1_0000, &[0x10, 0x28]), Some(0x3_0028));
        assert_eq!(m.follow(0x1_0000, &[]), Some(0x1_0000));
        assert_eq!(m.follow(0x9_0000, &[0x10]), None);
    }

    #[test]
    fn reads_fail_instead_of_crashing() {
        let m = FakeMemory::default();
        m.map(0x1_0000, vec![0; 4]);
        assert!(m.read_u64(0x1_0000).is_none(), "runs past the region");
        assert!(m.read_f32(0x5).is_none());
        m.write_f32(0x1_0000, f32::NAN);
        assert!(m.read_f32(0x1_0000).is_none(), "NaN is treated as garbage");
    }
}
