// Mush8 - lib.rs
// Constantin Clerc (c22dev) - 0.1
pub mod keypage {
    pub const SIZE: u64 = 0x4000;
    pub const CTX_SLOT: u32 = 0x00;
    pub const SVC_GATE: u32 = 0x08;
    pub const CNT_MUL: u32 = 0x10;
    pub const NET: u32 = 0x18;
    pub const DISPLAY_SIZE: u32 = 0x20;
    pub const RENDER_POOL: u32 = 0x28;
    pub const RENDER_HEAP: u32 = 0x30;
    pub const ROOT_HEAP: u32 = 0x38;
    pub const OS_HEAP: u32 = 0x40;
    pub const SPLIT_DIV: u32 = 0x48;
    pub const ASPECT: u32 = 0x50;
    pub const DYNRES: u32 = 0x58;

    pub const NET_NEXTENDO: u8 = 4;
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
