// Mush8 - lib.rs
// Constantin Clerc (c22dev) - 0.1
pub mod keypage {
    pub const SIZE: u64 = 0x4000;
    pub const CTX_SLOT: u32 = 0x00;
    pub const SVC_GATE: u32 = 0x08;
    pub const CNT_MUL: u32 = 0x10;
    pub const NET: u32 = 0x18;
    pub const ASPECT_SCALE: u32 = 0x20;
}

pub mod ctx {
    pub const X18: u32 = 0x00;
    pub const TPIDR: u32 = 0x08;
    pub const TPIDRRO: u32 = 0x10;
    pub const SPILL0: u32 = 0x18;
    pub const SPILL1: u32 = 0x20;
    pub const SPILL2: u32 = 0x28;
    pub const GATE_STACK: u32 = 0x30;
}

pub const GUEST_TICK_HZ: u64 = 19_200_000;
