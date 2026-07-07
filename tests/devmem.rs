//! Tests for the raw `DevMem` API (emulator backend).

use ddevmem::DevMem;

fn devmem(size: usize) -> DevMem {
    unsafe { DevMem::new(0x1000_0000, Some(size)).unwrap() }
}

#[test]
fn starts_zeroed_and_reports_geometry() {
    let mem = devmem(64);
    assert_eq!(mem.address(), 0x1000_0000);
    assert_eq!(mem.len(), 64);
    assert!(!mem.is_empty());
    assert_eq!(mem.read::<u32>(0), Some(0));
    assert_eq!(format!("{mem:?}"), "DevMem(0x10000000..0x10000040)");
}

#[test]
fn default_size_is_page_size() {
    let mem = unsafe { DevMem::new(0, None).unwrap() };
    assert_eq!(mem.len(), page_size::get());
}

#[test]
fn read_write_modify_roundtrip() {
    let mem = devmem(64);

    mem.write::<u32>(0, 0xDEAD_BEEF).unwrap();
    assert_eq!(mem.read::<u32>(0), Some(0xDEAD_BEEF));

    mem.modify::<u32>(0, |v| v.rotate_left(16)).unwrap();
    assert_eq!(mem.read::<u32>(0), Some(0xBEEF_DEAD));

    mem.write::<u8>(63, 0xAB).unwrap();
    assert_eq!(mem.read::<u8>(63), Some(0xAB));
}

#[test]
fn out_of_bounds_access_is_rejected() {
    let mem = devmem(64);

    assert_eq!(mem.read::<u32>(64), None);
    assert_eq!(mem.read::<u32>(61), None); // 61 + 4 > 64
    assert_eq!(mem.write::<u32>(64, 0), None);
    assert_eq!(mem.modify::<u32>(64, |v| v), None);

    // Offsets that would overflow the bounds arithmetic.
    assert_eq!(mem.read::<u32>(usize::MAX), None);
    assert_eq!(mem.read::<u32>(usize::MAX - 3), None);
}

#[test]
fn misaligned_access_is_rejected() {
    let mem = devmem(64);

    assert_eq!(mem.read::<u32>(2), None);
    assert_eq!(mem.write::<u32>(1, 0), None);
    assert_eq!(mem.modify::<u16>(3, |v| v), None);

    // Byte access has no alignment requirement.
    assert_eq!(mem.read::<u8>(3), Some(0));
}

#[test]
fn slice_roundtrip() {
    let mem = devmem(64);

    mem.write_slice(16, &[1u32, 2, 3, 4]).unwrap();
    let mut buf = [0u32; 4];
    mem.read_slice(16, &mut buf).unwrap();
    assert_eq!(buf, [1, 2, 3, 4]);
}

#[test]
fn slice_bounds_and_alignment() {
    let mem = devmem(64);
    let mut buf = [0u32; 4];

    assert_eq!(mem.read_slice(52, &mut buf), None); // 52 + 16 > 64
    assert_eq!(mem.read_slice(50, &mut buf[..1]), None); // misaligned
    assert_eq!(mem.write_slice(52, &[0u32; 4]), None);
    assert_eq!(mem.read_slice(48, &mut buf), Some(()));
}

#[test]
fn zero_length_region() {
    let mem = devmem(0);
    assert!(mem.is_empty());
    assert_eq!(mem.read::<u8>(0), None);
    assert_eq!(mem.write::<u8>(0, 1), None);
}

#[test]
fn unchecked_access_works_within_bounds() {
    let mem = devmem(64);
    unsafe {
        mem.write_unchecked::<u32>(8, 0x1234_5678);
        assert_eq!(mem.read_unchecked::<u32>(8), 0x1234_5678);
    }
}

#[test]
fn mapping_is_page_aligned() {
    // The emulator promises the same alignment as a real /dev/mem mapping,
    // which is what makes `offset % align_of::<T>() == 0` sufficient.
    let mem = devmem(64);
    assert_eq!(mem.as_ptr() as usize % page_size::get(), 0);
}

#[test]
fn devmem_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<DevMem>();
}
