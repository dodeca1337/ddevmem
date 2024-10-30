use bytemuck::{AnyBitPattern, NoUninit};
use memmap2::{MmapMut, MmapOptions};
use std::{fmt::Debug, fs::OpenOptions, hint::black_box, io::Error as IOError};

#[derive(Debug)]
pub enum Error {
    CentOpenFile(IOError),
    CentMmapFile(IOError),
}

pub struct DevMem {
    mmap: MmapMut,
    address: usize,
}

impl Debug for DevMem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_fmt(format_args!(
            "DevMem({:#X}..{:#X})",
            self.address,
            self.address + self.len()
        ))
    }
}

impl DevMem {
    pub fn new(address: usize, size: Option<usize>) -> Result<Self, Error> {
        let page_size = page_size::get();
        let size = size.unwrap_or(page_size);

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(false)
            .open("/dev/mem")
            .map_err(Error::CentOpenFile)?;

        let mmap = unsafe {
            MmapOptions::new()
                .len(size)
                .offset(address as u64)
                .map_mut(&file)
                .map_err(Error::CentMmapFile)?
        };

        Ok(Self { mmap, address })
    }

    #[inline(always)]
    pub fn address(&self) -> usize {
        self.address
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.mmap.len()
    }

    #[inline(always)]
    pub fn data(&self) -> &[u8] {
        black_box(&self.mmap)
    }

    #[inline(always)]
    pub fn data_mut(&mut self) -> &mut [u8] {
        black_box(&mut self.mmap)
    }

    #[inline(always)]
    pub fn get<T: AnyBitPattern>(&self, offset: usize) -> Option<&T> {
        if offset + std::mem::size_of::<T>() > self.len() {
            return None;
        }

        let data = &self.data()[offset..offset + std::mem::size_of::<T>()];
        Some(black_box(bytemuck::from_bytes(data)))
    }

    #[inline(always)]
    pub fn get_mut<T: NoUninit + AnyBitPattern>(&mut self, offset: usize) -> Option<&mut T> {
        if offset + std::mem::size_of::<T>() > self.len() {
            return None;
        }

        let data = &mut self.data_mut()[offset..offset + std::mem::size_of::<T>()];
        Some(black_box(bytemuck::from_bytes_mut(data)))
    }

    #[inline(always)]
    pub fn get_slice<T: AnyBitPattern>(&self, offset: usize, count: usize) -> Option<&[T]> {
        if offset + std::mem::size_of::<T>() * count > self.len() {
            return None;
        }

        let data = &self.data()[offset..offset + std::mem::size_of::<T>() * count];
        Some(black_box(bytemuck::cast_slice(data)))
    }

    #[inline(always)]
    pub fn get_slice_mut<T: NoUninit + AnyBitPattern>(
        &mut self,
        offset: usize,
        count: usize,
    ) -> Option<&mut [T]> {
        if offset + std::mem::size_of::<T>() * count > self.len() {
            return None;
        }

        let data = &mut self.data_mut()[offset..offset + std::mem::size_of::<T>() * count];
        Some(black_box(bytemuck::cast_slice_mut(data)))
    }
}
