// MK8DX 4.0.1 projection getters. Shared by the loader and signed iOS builder.
pub const MODULE_ID: &str = "158c0c7399aabd7bf83795c2ef3484c700000000000000000000000000000000";
pub const GETTERS: [(u64, [u32; 4], &str); 3] = [
    (
        0x61b8a4,
        [0xa9be4ff4, 0xa9017bfd, 0x910043fd, 0xaa0003f3],
        "_mk8_projection_get",
    ),
    (
        0x61b91c,
        [0xf81d0ff5, 0xa9014ff4, 0xa9027bfd, 0x910083fd],
        "_mk8_projection_device_get",
    ),
    (
        0x592808,
        [0xfc1b0fe8, 0xf90007f9, 0xa9015ff8, 0xa90257f6],
        "_mk8_pane_calculate",
    ),
];
pub const STUB_OFFSET: u64 = 160; // Missing-avatar guards occupy bytes 0..140.
pub const STUB_SIZE: u64 = 20;
