//! `slam.exe` reserves 8 MiB for its main thread (ADR-0144): with the linker's default of
//! 1 MiB the shell overflows the stack while it starts (#369).
#![cfg(windows)]

/// The stack reserve from the PE optional header of a 64-bit image.
fn stack_reserve(image: &[u8]) -> u64 {
    let read = |at: usize, len: usize| &image[at..at + len];
    assert_eq!(read(0, 2), b"MZ", "not a PE image");
    let pe = u32::from_le_bytes(read(0x3c, 4).try_into().unwrap()) as usize;
    assert_eq!(read(pe, 4), b"PE\0\0", "no PE signature");
    // The optional header follows the 4-byte signature and the 20-byte file header.
    let optional = pe + 24;
    assert_eq!(read(optional, 2), [0x0b, 0x02], "not PE32+");
    u64::from_le_bytes(read(optional + 72, 8).try_into().unwrap())
}

#[test]
fn main_thread_reserves_8_mib() {
    let image = std::fs::read(env!("CARGO_BIN_EXE_slam")).expect("slam.exe");
    assert_eq!(stack_reserve(&image), 8 * 1024 * 1024);
}
