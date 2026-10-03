//! The frame ring (`protocol/schemas/browser-rpc/browser-rpc.md`, "The frame ring"): one shared-memory region per
//! tab, a 4096-byte header and two BGRA slots. The engine writes ([`Region::write`]); the shell reads. [`Reader`] is
//! the shell's side as the engine's tests use it; `crates/browser/src/embedded.rs` has the shell's own.
//!
//! Linux only for now: `memfd_create`, `mmap`, and the descriptor sent with `SCM_RIGHTS` ([`send_fd`]). macOS
//! (`shm_open`) and Windows (a named file mapping) are documented in browser-rpc.md, not built.

// Shared memory and descriptor passing are system calls on raw pointers; every use is commented.
#![allow(unsafe_code)]

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicU32, Ordering};

pub const MAGIC: u32 = 0x5242_4C45;
pub const VERSION: u32 = 1;
pub const SLOTS: usize = 2;
pub const HEADER_SIZE: usize = 4096;
pub const SLOT_HEADER: usize = 64;
pub const SLOT_STRIDE: usize = 320;
pub const MAX_DIRTY: usize = 16;

pub const FREE: u32 = 0;
pub const WRITING: u32 = 1;
pub const READY: u32 = 2;
pub const READING: u32 = 3;

/// A rectangle in device pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl Rect {
    fn union(self, o: Rect) -> Rect {
        let x0 = self.x.min(o.x);
        let y0 = self.y.min(o.y);
        let x1 = (self.x + self.width).max(o.x + o.width);
        let y1 = (self.y + self.height).max(o.y + o.height);
        Rect {
            x: x0,
            y: y0,
            width: x1 - x0,
            height: y1 - y0,
        }
    }
}

/// At most [`MAX_DIRTY`] rectangles: the rest are merged into the last one.
pub fn clamp_dirty(rects: &[Rect]) -> Vec<Rect> {
    if rects.len() <= MAX_DIRTY {
        return rects.to_vec();
    }
    let mut out = rects[..MAX_DIRTY - 1].to_vec();
    let rest = rects[MAX_DIRTY - 1..]
        .iter()
        .copied()
        .reduce(Rect::union)
        .unwrap_or_default();
    out.push(rest);
    out
}

/// `CLOCK_MONOTONIC` in nanoseconds: the clock of `paintNs`, shared by both processes.
pub fn monotonic_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: clock_gettime writes the timespec it is given.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// The bytes of a slot for a frame of `width` by `height`, rounded to pages.
pub fn slot_size_for(width: u32, height: u32) -> usize {
    (width as usize * height as usize * 4).div_ceil(4096) * 4096
}

fn slot_off(i: usize) -> usize {
    SLOT_HEADER + i * SLOT_STRIDE
}

/// A mapping of a region (the whole of it, read-write: the engine's side).
struct Map {
    ptr: *mut u8,
    len: usize,
}

// SAFETY: the mapping is plain shared memory; every access goes through atomics for the states or through the
// protocol (a slot is written only by the side that moved it to writing or reading).
unsafe impl Send for Map {}
unsafe impl Sync for Map {}

impl Map {
    fn new(fd: RawFd, len: usize, offset: usize, prot: libc::c_int) -> io::Result<Map> {
        // SAFETY: mapping a descriptor we own with a length within its size; failure is checked.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                prot,
                libc::MAP_SHARED,
                fd,
                offset as libc::off_t,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Map {
            ptr: ptr.cast(),
            len,
        })
    }

    fn u32_at(&self, off: usize) -> u32 {
        debug_assert!(off + 4 <= self.len);
        // SAFETY: in bounds, 4-byte aligned by the layout.
        unsafe { std::ptr::read_volatile(self.ptr.add(off).cast::<u32>()) }
    }

    fn u64_at(&self, off: usize) -> u64 {
        debug_assert!(off + 8 <= self.len);
        // SAFETY: in bounds, 8-byte aligned by the layout.
        unsafe { std::ptr::read_volatile(self.ptr.add(off).cast::<u64>()) }
    }

    fn i32_at(&self, off: usize) -> i32 {
        self.u32_at(off) as i32
    }

    fn put_u32(&self, off: usize, v: u32) {
        debug_assert!(off + 4 <= self.len);
        // SAFETY: in bounds and aligned; only the owner of the slot (or of the header, at creation) writes it.
        unsafe { std::ptr::write_volatile(self.ptr.add(off).cast::<u32>(), v.to_le()) }
    }

    fn put_u64(&self, off: usize, v: u64) {
        debug_assert!(off + 8 <= self.len);
        // SAFETY: as above.
        unsafe { std::ptr::write_volatile(self.ptr.add(off).cast::<u64>(), v.to_le()) }
    }

    fn state(&self, slot: usize) -> &AtomicU32 {
        // SAFETY: the state word is 4-byte aligned, inside the header, and lives as long as the mapping; both
        // processes only touch it atomically.
        unsafe { &*self.ptr.add(slot_off(slot)).cast::<AtomicU32>() }
    }
}

impl Drop for Map {
    fn drop(&mut self) {
        // SAFETY: unmapping what new mapped.
        unsafe { libc::munmap(self.ptr.cast(), self.len) };
    }
}

/// What a slot holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotMeta {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub sequence: u64,
    pub paint_ns: u64,
    pub copy_ns: u64,
    pub dirty: Vec<Rect>,
}

fn read_meta(m: &Map, slot: usize) -> SlotMeta {
    let o = slot_off(slot);
    let n = (m.u32_at(o + 40) as usize).min(MAX_DIRTY);
    SlotMeta {
        width: m.u32_at(o + 4),
        height: m.u32_at(o + 8),
        stride: m.u32_at(o + 12),
        sequence: m.u64_at(o + 16),
        paint_ns: m.u64_at(o + 24),
        copy_ns: m.u64_at(o + 32),
        dirty: (0..n)
            .map(|i| {
                let r = o + 48 + i * 16;
                Rect {
                    x: m.i32_at(r),
                    y: m.i32_at(r + 4),
                    width: m.i32_at(r + 8),
                    height: m.i32_at(r + 12),
                }
            })
            .collect(),
    }
}

/// The engine's side of one region.
pub struct Region {
    pub id: u64,
    pub slot_size: usize,
    pub width: u32,
    pub height: u32,
    fd: OwnedFd,
    map: Map,
}

/// What [`Region::write`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Written {
    /// The frame is ready in this slot.
    Slot { slot: usize, copy_ns: u64 },
    /// No slot was free or ready (the shell holds one and the other is being written): dropped.
    Dropped,
}

impl Region {
    /// A new region for frames up to `width` by `height` device pixels.
    pub fn create(id: u64, width: u32, height: u32) -> io::Result<Region> {
        let slot_size = slot_size_for(width.max(1), height.max(1));
        let len = HEADER_SIZE + SLOTS * slot_size;
        // SAFETY: a NUL-terminated name; the result is checked.
        let fd = unsafe { libc::memfd_create(c"eludite-frames".as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: memfd_create returned a new descriptor we own.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        // SAFETY: sizing our own descriptor.
        if unsafe { libc::ftruncate(fd.as_raw_fd(), len as libc::off_t) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let map = Map::new(fd.as_raw_fd(), len, 0, libc::PROT_READ | libc::PROT_WRITE)?;
        map.put_u32(0, MAGIC);
        map.put_u32(4, VERSION);
        map.put_u32(8, SLOTS as u32);
        map.put_u32(12, HEADER_SIZE as u32);
        map.put_u64(16, slot_size as u64);
        map.put_u32(24, width);
        map.put_u32(28, height);
        map.put_u64(32, id);
        Ok(Region {
            id,
            slot_size,
            width,
            height,
            fd,
            map,
        })
    }

    pub fn size(&self) -> usize {
        HEADER_SIZE + SLOTS * self.slot_size
    }

    pub fn fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    /// Whether a frame of this size fits a slot.
    pub fn fits(&self, width: u32, height: u32) -> bool {
        width as usize * height as usize * 4 <= self.slot_size
    }

    fn claim(&self) -> Option<usize> {
        let seq = |s: usize| self.map.u64_at(slot_off(s) + 16);
        let mut order: Vec<usize> = (0..SLOTS).collect();
        order.sort_by_key(|&s| seq(s));
        // A free slot first (the older one), else the older unread frame.
        for want in [FREE, READY] {
            for &s in &order {
                if self
                    .map
                    .state(s)
                    .compare_exchange(want, WRITING, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
                {
                    return Some(s);
                }
            }
        }
        None
    }

    /// Copy a frame (`height` rows of `width * 4` bytes, BGRA) into a slot, mark it ready.
    pub fn write(
        &self,
        pixels: &[u8],
        width: u32,
        height: u32,
        sequence: u64,
        paint_ns: u64,
        dirty: &[Rect],
    ) -> Written {
        let bytes = width as usize * height as usize * 4;
        assert!(pixels.len() >= bytes && self.fits(width, height));
        let Some(slot) = self.claim() else {
            return Written::Dropped;
        };
        let t0 = monotonic_ns();
        // SAFETY: the slot's pixels are inside the mapping (fits), and this side owns the slot (writing).
        unsafe {
            std::ptr::copy_nonoverlapping(
                pixels.as_ptr(),
                self.map.ptr.add(HEADER_SIZE + slot * self.slot_size),
                bytes,
            );
        }
        let copy_ns = monotonic_ns().saturating_sub(t0);
        let o = slot_off(slot);
        let dirty = clamp_dirty(dirty);
        self.map.put_u32(o + 4, width);
        self.map.put_u32(o + 8, height);
        self.map.put_u32(o + 12, width * 4);
        self.map.put_u64(o + 16, sequence);
        self.map.put_u64(o + 24, paint_ns);
        self.map.put_u64(o + 32, copy_ns);
        self.map.put_u32(o + 40, dirty.len() as u32);
        for (i, r) in dirty.iter().enumerate() {
            let p = o + 48 + i * 16;
            self.map.put_u32(p, r.x as u32);
            self.map.put_u32(p + 4, r.y as u32);
            self.map.put_u32(p + 8, r.width as u32);
            self.map.put_u32(p + 12, r.height as u32);
        }
        self.map.state(slot).store(READY, Ordering::Release);
        Written::Slot { slot, copy_ns }
    }
}

/// The shell's side, as the engine's tests use it: the header page read-write (states), the slots read-only.
pub struct Reader {
    header: Map,
    slots: Map,
    pub id: u64,
    pub slot_size: usize,
    pub width: u32,
    pub height: u32,
}

impl Reader {
    pub fn open(fd: RawFd) -> io::Result<Reader> {
        let header = Map::new(fd, HEADER_SIZE, 0, libc::PROT_READ | libc::PROT_WRITE)?;
        if header.u32_at(0) != MAGIC || header.u32_at(4) != VERSION {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "not an eludite frame region (magic or version)",
            ));
        }
        let slot_size = header.u64_at(16) as usize;
        let slots = Map::new(fd, SLOTS * slot_size, HEADER_SIZE, libc::PROT_READ)?;
        Ok(Reader {
            id: header.u64_at(32),
            width: header.u32_at(24),
            height: header.u32_at(28),
            slot_size,
            header,
            slots,
        })
    }

    pub fn state(&self, slot: usize) -> u32 {
        self.header.state(slot).load(Ordering::Acquire)
    }

    pub fn meta(&self, slot: usize) -> SlotMeta {
        read_meta(&self.header, slot)
    }

    /// Consume the newest ready frame: `f` sees its metadata and pixels; the slot is then free.
    pub fn consume<R>(&self, f: impl FnOnce(&SlotMeta, &[u8]) -> R) -> Option<R> {
        self.take(&[READY], true, f)
    }

    /// Read the newest frame, ready or already consumed, and leave its state as it was.
    pub fn peek<R>(&self, f: impl FnOnce(&SlotMeta, &[u8]) -> R) -> Option<R> {
        self.take(&[READY, FREE], false, f)
    }

    fn take<R>(
        &self,
        states: &[u32],
        consume: bool,
        f: impl FnOnce(&SlotMeta, &[u8]) -> R,
    ) -> Option<R> {
        let mut order: Vec<usize> = (0..SLOTS).collect();
        order.sort_by_key(|&s| std::cmp::Reverse(read_meta(&self.header, s).sequence));
        for s in order {
            for &from in states {
                if self
                    .header
                    .state(s)
                    .compare_exchange(from, READING, Ordering::Acquire, Ordering::Relaxed)
                    .is_err()
                {
                    continue;
                }
                let meta = read_meta(&self.header, s);
                let len = meta.stride as usize * meta.height as usize;
                if meta.sequence == 0 || len > self.slot_size {
                    self.header.state(s).store(from, Ordering::Release);
                    break;
                }
                // SAFETY: the slot is inside the read-only mapping and held in reading, so the engine does not
                // write it while the slice lives.
                let pixels = unsafe {
                    std::slice::from_raw_parts(self.slots.ptr.add(s * self.slot_size), len)
                };
                let r = f(&meta, pixels);
                let back = if consume { FREE } else { from };
                self.header.state(s).store(back, Ordering::Release);
                return Some(r);
            }
        }
        None
    }
}

/// Send `fd` with an 8-byte payload (the region id) over a Unix socket (`SCM_RIGHTS`).
pub fn send_fd(sock: RawFd, fd: RawFd, id: u64) -> io::Result<()> {
    let payload = id.to_le_bytes();
    let mut iov = libc::iovec {
        iov_base: payload.as_ptr() as *mut libc::c_void,
        iov_len: payload.len(),
    };
    // SAFETY: CMSG_SPACE is a pure size computation.
    let space = unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) } as usize;
    let mut control = vec![0u8; space];
    // SAFETY: msghdr is plain data; every pointer set below outlives the sendmsg call.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = space as _;
    // SAFETY: the control buffer has room for one header carrying one descriptor (CMSG_SPACE above).
    unsafe {
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        (*cmsg).cmsg_level = libc::SOL_SOCKET;
        (*cmsg).cmsg_type = libc::SCM_RIGHTS;
        (*cmsg).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as _;
        std::ptr::write_unaligned(libc::CMSG_DATA(cmsg).cast::<RawFd>(), fd);
    }
    // SAFETY: a valid msghdr; the result is checked.
    let n = unsafe { libc::sendmsg(sock, &msg, libc::MSG_NOSIGNAL) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Receive one descriptor and its 8-byte payload sent by [`send_fd`].
pub fn recv_fd(sock: RawFd) -> io::Result<(u64, OwnedFd)> {
    let mut payload = [0u8; 8];
    let mut iov = libc::iovec {
        iov_base: payload.as_mut_ptr().cast(),
        iov_len: payload.len(),
    };
    // SAFETY: a size computation.
    let space = unsafe { libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) } as usize;
    let mut control = vec![0u8; space];
    // SAFETY: plain data, pointers outlive the call.
    let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
    msg.msg_iov = &mut iov;
    msg.msg_iovlen = 1;
    msg.msg_control = control.as_mut_ptr().cast();
    msg.msg_controllen = space as _;
    // SAFETY: a valid msghdr; the result is checked.
    let n = unsafe { libc::recvmsg(sock, &mut msg, libc::MSG_CMSG_CLOEXEC) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    if n == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the frame socket closed",
        ));
    }
    // SAFETY: walking the control messages recvmsg filled in.
    let fd = unsafe {
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        if cmsg.is_null()
            || (*cmsg).cmsg_level != libc::SOL_SOCKET
            || (*cmsg).cmsg_type != libc::SCM_RIGHTS
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "no descriptor in the frame socket message",
            ));
        }
        std::ptr::read_unaligned(libc::CMSG_DATA(cmsg).cast::<RawFd>())
    };
    // SAFETY: SCM_RIGHTS gave this process a new descriptor it now owns.
    Ok((u64::from_le_bytes(payload), unsafe {
        OwnedFd::from_raw_fd(fd)
    }))
}

/// A `SOCK_SEQPACKET` Unix socket pair, both ends close-on-exec.
pub fn socket_pair() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [0 as RawFd; 2];
    // SAFETY: socketpair writes two descriptors into the array; the result is checked.
    let r = unsafe {
        libc::socketpair(
            libc::AF_UNIX,
            libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
            0,
            fds.as_mut_ptr(),
        )
    };
    if r != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: two new descriptors this process owns.
    Ok(unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: u32, h: u32, b: u8) -> Vec<u8> {
        vec![b; (w * h * 4) as usize]
    }

    #[test]
    fn writes_and_reads_through_a_passed_descriptor() {
        let region = Region::create(7, 40, 30).unwrap();
        let (a, b) = socket_pair().unwrap();
        send_fd(a.as_raw_fd(), region.fd(), region.id).unwrap();
        let (id, fd) = recv_fd(b.as_raw_fd()).unwrap();
        assert_eq!(id, 7);
        let reader = Reader::open(fd.as_raw_fd()).unwrap();
        assert_eq!((reader.width, reader.height, reader.id), (40, 30, 7));
        assert_eq!(reader.slot_size, 8192);
        assert!(reader.consume(|_, _| ()).is_none(), "nothing written yet");

        let dirty = [Rect {
            x: 1,
            y: 2,
            width: 3,
            height: 4,
        }];
        assert!(matches!(
            region.write(&frame(40, 30, 9), 40, 30, 1, 100, &dirty),
            Written::Slot { slot: 0, .. }
        ));
        let (meta, px) = reader.consume(|m, p| (m.clone(), p.to_vec())).unwrap();
        assert_eq!(meta.sequence, 1);
        assert_eq!(meta.paint_ns, 100);
        assert_eq!(meta.dirty, dirty);
        assert_eq!((meta.width, meta.height, meta.stride), (40, 30, 160));
        assert!(px.iter().all(|&b| b == 9));
        assert_eq!(reader.state(0), FREE);
        // Consumed: nothing new until the next frame, but a peek still sees it.
        assert!(reader.consume(|_, _| ()).is_none());
        assert_eq!(reader.peek(|m, _| m.sequence), Some(1));
        assert_eq!(reader.state(0), FREE, "a peek restores the state");
    }

    #[test]
    fn the_engine_overwrites_the_older_unread_frame_and_never_a_slot_being_read() {
        let region = Region::create(1, 4, 4).unwrap();
        let reader = Reader::open(region.fd()).unwrap();
        for seq in 1..=3u64 {
            region.write(&frame(4, 4, seq as u8), 4, 4, seq, 0, &[]);
        }
        // Frames 2 and 3 are ready; 1 was overwritten.
        assert_eq!(reader.consume(|m, p| (m.sequence, p[0])), Some((3, 3)));
        // While the shell reads slot of frame 2, both other attempts must avoid it.
        reader.take(&[READY], true, |m, _| {
            assert_eq!(m.sequence, 2);
            assert!(matches!(
                region.write(&frame(4, 4, 4), 4, 4, 4, 0, &[]),
                Written::Slot { .. }
            ));
            // The only other slot now holds frame 4 (ready); the one being read is not touched.
            assert!(matches!(
                region.write(&frame(4, 4, 5), 4, 4, 5, 0, &[]),
                Written::Slot { .. }
            ));
        });
        assert_eq!(reader.consume(|m, p| (m.sequence, p[0])), Some((5, 5)));
    }

    #[test]
    fn many_dirty_rectangles_are_merged() {
        let rects: Vec<Rect> = (0..20)
            .map(|i| Rect {
                x: i,
                y: 0,
                width: 1,
                height: 1,
            })
            .collect();
        let d = clamp_dirty(&rects);
        assert_eq!(d.len(), MAX_DIRTY);
        assert_eq!(
            d[15],
            Rect {
                x: 15,
                y: 0,
                width: 5,
                height: 1
            }
        );
    }
}
