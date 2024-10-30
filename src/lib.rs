mod devmem;
mod reg;

#[doc(inline)]
pub use devmem::DevMem;

#[doc(inline)]
pub use reg::{ReadOnlyReg, ReadOnlySliceReg, Reg, SliceReg};

/*
register_map! {
    unsafe map SPRI in 0x8361_0000..+0x0C {
        reg1 in 0x00..0x04 of mut u32,
        reg2 in 0x04..=0x07 of mut u32,
        reg3 in 0x08..+0x04 of mut u32,
    }

*/
