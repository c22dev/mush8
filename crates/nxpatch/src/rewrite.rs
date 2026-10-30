// Mush8 - rewrite.rs
// Constantin Clerc (c22dev) - 0.1
use crate::asm::{self, Reg, SP};
use crate::classify;
use anyhow::{Context, Result, bail, ensure};
use capstone::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

pub use nxabi::{ctx, keypage};

const CTR_EL0: u32 = 0x8444_C004;

pub struct JumpTable {
    pub module_id: &'static str,
    pub site: u64,
    pub table: u64,
    pub count: usize,
}

pub const JUMP_TABLES: &[JumpTable] = &[
    JumpTable { module_id: "ba520906609c503227871a314a94fb5f6ccbe444", site: 0x759cc, table: 0x7b7578, count: 4 },
    JumpTable { module_id: "d9de423efc1512328c3c05378c7cb03173facde3", site: 0x1ae1f0, table: 0x51a2fc, count: 27 },
    JumpTable { module_id: "d9de423efc1512328c3c05378c7cb03173facde3", site: 0x1b18ec, table: 0x51afa4, count: 27 },
];

pub struct GamePatch {
    pub module_id: &'static str,
    pub site: u64,
    pub from: &'static [u32],
    pub to: &'static [u32],
}

const MK8DX_401: &str = "158c0c7399aabd7bf83795c2ef3484c700000000";
const MK8DX_305: &str = "fe941ed5ba14be5d505698da1bbf4fe700000000";

struct Game {
    module_id: &'static str,
    keypage: u64,
    aspect_stores: &'static [u64],
}

pub struct PatchGroup {
    pub module_id: &'static str,
    pub name: &'static str,
    pub optional: bool,
    pub online: bool,
    pub sites: &'static [u64],
}

pub const PATCH_GROUPS: &[PatchGroup] = &[
    PatchGroup { module_id: MK8DX_401, name: "main menu", optional: true, online: true, sites: &[0x4d8a04, 0x4d8af8, 0x4d8b08, 0x4d8bb4, 0x4da738, 0x3fc4a8] },
    PatchGroup { module_id: MK8DX_401, name: "5-8 players row", optional: true, online: false, sites: &[0x438f24, 0x439600, 0xb52a74] },
    PatchGroup { module_id: MK8DX_401, name: "ghost menu", optional: true, online: true, sites: &[0x4a3214, 0x4a32d8, 0x4a493c] },
    PatchGroup { module_id: MK8DX_401, name: "ghost upload popup", optional: true, online: true, sites: &[0x546a80] },
    PatchGroup { module_id: MK8DX_401, name: "60 fps split screen", optional: true, online: false, sites: &[0xafecc8] },
    PatchGroup { module_id: MK8DX_401, name: "dynamic resolution", optional: true, online: false, sites: &[0x3c568c, 0x3c5e0c] },
    PatchGroup { module_id: MK8DX_401, name: "4K", optional: false, online: false, sites: &[0x87de00, 0x3c7354, 0x3d6820, 0x7e342c, 0x7e3390] },
    PatchGroup { module_id: MK8DX_305, name: "main menu", optional: true, online: true, sites: &[0x4d54bc, 0x4d55a4, 0x4d55b4, 0x4d5660, 0x4d70fc, 0x3fa344] },
    PatchGroup { module_id: MK8DX_305, name: "ghost menu", optional: true, online: true, sites: &[0x49fea8, 0x49ff6c, 0x4a15d0] },
    PatchGroup { module_id: MK8DX_305, name: "ghost upload popup", optional: true, online: true, sites: &[0x543254] },
    PatchGroup { module_id: MK8DX_305, name: "60 fps split screen", optional: true, online: false, sites: &[0xafa2a8] },
    PatchGroup { module_id: MK8DX_305, name: "4K", optional: false, online: false, sites: &[0x88a514, 0x3c51e8, 0x3d46bc, 0x7f01f0, 0x7f0154] },
];

#[derive(Clone, Copy)]
enum Nextendo {
    Original,
    Patched,
    Prefix(&'static [u32]),
}

const MOV_W1_1: u32 = 0x5280_0021;

const NEXTENDO_SITES: &[(&str, u64, Nextendo)] = &[
    (MK8DX_401, 0x4d8a04, Nextendo::Prefix(&[MOV_W1_1])),
    (MK8DX_401, 0x4da738, Nextendo::Patched),
    (MK8DX_305, 0x4d54bc, Nextendo::Prefix(&[MOV_W1_1])),
    (MK8DX_305, 0x4d70fc, Nextendo::Patched),
];

fn nextendo_at(module_id: &str, site: u64) -> Nextendo {
    NEXTENDO_SITES.iter().find(|(m, s, _)| *m == module_id && *s == site).map_or(Nextendo::Original, |(_, _, n)| *n)
}

pub fn group_of(module_id: &str, site: u64) -> Option<&'static PatchGroup> {
    PATCH_GROUPS.iter().find(|g| g.module_id == module_id && g.sites.contains(&site))
}

pub fn aspect_stores(module_id: &str) -> &'static [u64] {
    GAMES.iter().find(|g| g.module_id == module_id).map_or(&[], |g| g.aspect_stores)
}

const GAMES: &[Game] = &[
    Game { module_id: MK8DX_401, keypage: 0x1368000, aspect_stores: &[0x3ccab8, 0x3cd25c, 0x3cd318] },
    Game { module_id: MK8DX_305, keypage: 0x1364000, aspect_stores: &[0x3ca958, 0x3cb0fc, 0x3cb1b8] },
];

pub const GAME_PATCHES: &[GamePatch] = &[
    GamePatch { module_id: MK8DX_401, site: 0x4d8a04, from: &[0x9400_0038], to: &[0x9400_074E] },
    GamePatch { module_id: MK8DX_401, site: 0x4d8af8, from: &[0x2A01_03F3], to: &[0x2A02_03F3] },
    GamePatch { module_id: MK8DX_401, site: 0x4d8b08, from: &[0x1200_0276], to: &[0x1200_0036] },
    GamePatch { module_id: MK8DX_401, site: 0x4d8bb4, from: &[0x1A88_0513], to: &[0xD503_201F] },
    GamePatch {
        module_id: MK8DX_401,
        site: 0x4da738,
        from: &[
            0x2A1F_03E0, 0x940F_9287, 0xB400_0640, 0x940F_8E5E, 0xB400_0600, 0xF940_AC08, 0xB400_05C8, 0xB941_150A, 0x528C_1009,
            0x0A2A_012A, 0x3500_054A, 0x7940_1108, 0x0A09_0108, 0x3400_04E8, 0x3200_07E8, 0x9100_63E0, 0xB900_1BE8, 0x940F_924D,
            0x9000_7108, 0xF947_3908, 0x08DF_FD08, 0xAA00_03F4, 0x3700_01C8, 0x9000_7100, 0xF947_3800, 0x9419_E0C9, 0x3400_0140,
            0x9000_7108, 0xF947_3D08, 0x9000_7109, 0xF947_4129, 0x9100_4108, 0xF900_0128, 0x9000_7100, 0xF947_3800, 0x9419_E0C3,
            0xF940_0288, 0x9000_7101, 0xF940_0108, 0xF947_4021, 0xAA14_03E0, 0xD63F_0100, 0x7200_001F, 0xAA13_03E0, 0x9A9F_1288,
            0xF940_1908, 0x5283_F329, 0x2A1F_03E2, 0x3869_6908, 0x7100_011F, 0x1A9F_17E1, 0x97FF_F8B8,
        ],
        to: &[
            0x1400_0034, 0xA95E_2408, 0xF940_2108, 0xF940_0D08, 0xF940_2129, 0xF940_0D29, 0xA95D_2C0A, 0xF940_214A, 0xF940_0D4A,
            0xF940_216B, 0xF940_0D6B, 0x3941_610C, 0x121F_198C, 0x3901_610C, 0xBD40_3541, 0xBD40_3562, 0x1E21_3841, 0xBD40_3500,
            0xD000_746E, 0x3940_61CE, 0x3400_016E, 0x1E2C_1002, 0x1E22_0822, 0x1E22_3800, 0xF940_190D, 0xF900_192D, 0x1E21_2802,
            0xBD00_3522, 0x3941_612C, 0x321C_018C, 0x1400_0003, 0x3941_612C, 0x121F_198C, 0x3901_612C, 0xBD00_3560, 0x1E21_3802,
            0xBD00_3542, 0x3941_616C, 0x321C_018C, 0x3901_616C, 0x3941_614C, 0x321C_018C, 0x3901_614C, 0x5280_0021, 0x5281_07A2,
            0x7100_0DDF, 0x5281_078D, 0x1A82_01A2, 0x7100_09DF, 0x5281_072D, 0x1A82_01A2, 0x17FF_F8B8,
        ],
    },
    GamePatch {
        module_id: MK8DX_401,
        site: 0x3fc4a8,
        from: &[
            0x121B_0048, 0x121A_0049, 0x7100_0C3F, 0x5400_0188, 0x5280_014A, 0x1A81_054B, 0x7100_013F, 0x1A8B_0029, 0x5100_042B,
            0x7100_003F, 0x1A8B_014A, 0x7100_011F, 0x1A8A_0121, 0x2A01_03E0, 0xD65F_03C0, 0x3400_0088, 0x3200_07E1, 0x2A01_03E0,
            0xD65F_03C0, 0x3500_0109, 0x3738_0142, 0x3640_0082, 0x7100_283F, 0x321E_03E8, 0x1A81_0501, 0x2A01_03E0, 0xD65F_03C0,
            0x2A1F_03E1, 0x2A01_03E0, 0xD65F_03C0, 0x5100_0428, 0x7100_103F, 0x5280_0149, 0x1A88_0121, 0x2A01_03E0, 0xD65F_03C0,
        ],
        to: &[
            0x9000_7B6B, 0x3940_616B, 0x7100_017F, 0x5280_006C, 0x1A9F_158B, 0x5280_014A, 0x7100_0C3F, 0x5400_0168, 0x3728_00A2,
            0x3630_0222, 0x531F_7820, 0x6B0B_003F, 0x1400_0004, 0x5100_0C20, 0x1301_7C00, 0x7100_003F, 0x1A80_0540, 0xD65F_03C0,
            0x3728_0142, 0x3730_0162, 0x3738_0182, 0x3640_00A2, 0x7100_283F, 0x5280_0088, 0x1A81_0500, 0xD65F_03C0, 0x2A01_03E0,
            0xD65F_03C0, 0x2A0B_03E0, 0xD65F_03C0, 0x2A1F_03E0, 0xD65F_03C0, 0x5100_0428, 0x7100_103F, 0x1A88_0140, 0xD65F_03C0,
        ],
    },
    GamePatch {
        module_id: MK8DX_401,
        site: 0x438f24,
        from: &[
            0xF941_F6A8, 0x5280_02E9, 0xB900_4909, 0xF941_FAA8, 0x321D_07E9, 0xB900_4909, 0xF941_FEA8, 0x5280_0329, 0xB900_4909,
            0xB000_7609, 0xF947_1529, 0x9100_4137, 0xF942_02A9, 0x5280_0348, 0xB900_4928,
        ],
        to: &[
            0xB000_7609, 0xF947_1529, 0x9100_4137, 0x5280_0069, 0xB904_0AA9, 0xB000_5588, 0x910F_2908, 0xA900_A3F7, 0xAA13_03E0,
            0x9100_23E1, 0x9411_D267, 0x52A8_64C8, 0xB900_3008, 0x52A8_8BC8, 0xB900_5008,
        ],
    },
    GamePatch { module_id: MK8DX_401, site: 0x439600, from: &[0xF000_5568, 0x9135_8508], to: &[0xB000_38C8, 0x9129_D108] },
    GamePatch {
        module_id: MK8DX_401,
        site: 0x4a3214,
        from: &[
            0x5280_0D68, 0x5281_1B40, 0xB900_4A88, 0x9410_2A5A, 0xAA00_03E1, 0xAA16_03E0, 0x9410_2A69, 0xB988_5A68, 0xB948_5E69,
            0x6B09_011F, 0x5400_00CA, 0xF944_3269, 0xF828_7934, 0xB948_5A68, 0x1100_0508, 0xB908_5A68,
        ],
        to: &[
            0xAA14_03F7, 0x1400_000F, 0xAA0D_03E9, 0xEB0C_015F, 0x5400_0080, 0xF840_8549, 0xF940_2129, 0xF940_0D29, 0xB940_352B,
            0xB900_350B, 0x3941_610B, 0x321C_016B, 0x3901_610B, 0xAA09_03E8, 0x54FF_FE81, 0x1400_0032,
        ],
    },
    GamePatch {
        module_id: MK8DX_401,
        site: 0x4a32d8,
        from: &[
            0x5280_0D88, 0x5281_1C80, 0xB900_4A88, 0x9410_2A29, 0xAA00_03E1, 0xAA16_03E0, 0x9410_2A38, 0xB988_5A68, 0xB948_5E69,
            0x6B09_011F, 0x5400_00CA, 0xF944_3269, 0xF828_7934, 0xB948_5A68, 0x1100_0508, 0xB908_5A68,
        ],
        to: &[
            0xF940_2288, 0xF940_0D08, 0x3941_6109, 0x121F_1929, 0x3901_6109, 0xF940_22ED, 0xF940_0DAD, 0x3941_61A9, 0x121F_1929,
            0x3901_61A9, 0xF944_326A, 0x9100_614C, 0xF840_8548, 0xF940_2108, 0xF940_0D08, 0x1400_058B,
        ],
    },
    GamePatch {
        module_id: MK8DX_401,
        site: 0x4a493c,
        from: &[0xB948_5A68, 0x7100_111F, 0x5400_0083, 0xF944_3268, 0xF940_0D00, 0x1400_0002, 0xAA1F_03E0, 0x2A15_03E1],
        to: &[0x1400_0013, 0xB940_8A89, 0x1200_7929, 0xB900_8A89, 0xB940_8AE9, 0x1200_7929, 0xB900_8AE9, 0x17FF_FA31],
    },
    GamePatch { module_id: MK8DX_401, site: 0xb52a74, from: &[0xD503_201F, 0xD503_201F], to: &[0x305F_6E4F, 0x0000_0032] },
    GamePatch { module_id: MK8DX_401, site: 0x546a80, from: &[0x3400_23C8], to: &[0x1400_011E] },
    GamePatch { module_id: MK8DX_401, site: 0x87de00, from: &[0xB219_0FE0, 0xF2C0_8700, 0xD65F_03C0], to: &[0xF000_5740, 0xF940_1000, 0xD65F_03C0] },
    GamePatch {
        module_id: MK8DX_401,
        site: 0x3c7354,
        from: &[0x320B_07E8, 0xF900_23E8, 0x52A0_B008, 0xF900_27E8],
        to: &[0xB000_7D08, 0xF940_1508, 0x52A0_0C09, 0xA904_23E9],
    },
    GamePatch { module_id: MK8DX_401, site: 0x3d6820, from: &[0xB940_0BE8, 0xB940_0BE8, 0x3204_03E8], to: &[0xD000_7C88, 0xB940_3108, 0xD503_201F] },
    GamePatch { module_id: MK8DX_401, site: 0x7e342c, from: &[0x52B6_0A08, 0x9100_63E0, 0xF900_0FE8], to: &[0xB000_5C28, 0xF940_1D08, 0xF900_0FE8] },
    GamePatch {
        module_id: MK8DX_401,
        site: 0x7e3390,
        from: &[
            0xD100_83FF, 0xA901_7BFD, 0x9100_43FD, 0x52B8_3800, 0x940D_C3E0, 0x9100_23E0, 0x3209_07E1, 0x940D_C351, 0xF940_07E0,
            0x3209_07E1, 0x97F6_7D50, 0xA941_7BFD, 0x9100_83FF, 0xD65F_03C0,
        ],
        to: &[
            0xA9BE_7BFD, 0x9100_03FD, 0xB000_5C20, 0xF940_2000, 0x940D_C3E0, 0x9100_43E0, 0x3209_07E1, 0x940D_C351, 0xF940_0BE0,
            0x3209_07E1, 0x97F6_7D50, 0xA8C2_7BFD, 0xD65F_03C0, 0xD503_201F,
        ],
    },
    GamePatch {
        module_id: MK8DX_401,
        site: 0xafecc8,
        from: &[
            0x3940_6008, 0x1100_0508, 0x3900_6008, 0xB940_0808, 0x3400_0308, 0xF940_0809, 0x3940_640A, 0x7100_095F, 0x5400_01A3,
            0x39D0_252B, 0x3100_057F, 0x5400_0140, 0x3940_600C, 0x1ACA_098D, 0x1B0A_B1AA, 0x6B0B_015F, 0x5400_00E0, 0xF940_012A,
            0xB400_00AA, 0x3200_03EA, 0x1400_0004, 0x2A1F_03EA, 0x1400_0002, 0x321F_03EA, 0x3910_212A, 0x9110_4129, 0xD100_0508,
            0xB5FF_FD68, 0xD65F_03C0,
        ],
        to: &[
            0x3940_600C, 0x1100_058C, 0x3900_600C, 0x3940_600C, 0xA940_A408, 0x3400_02E8, 0xD000_434E, 0x3941_21CE, 0x3940_640A,
            0x6B0E_015F, 0x5400_0163, 0x39D0_252B, 0x37F8_012B, 0x1ACA_098D, 0x1B0A_B1AA, 0x6B0B_015F, 0x5400_00E0, 0xF940_012A,
            0xB400_00AA, 0x5280_002A, 0x1400_0004, 0x2A1F_03EA, 0x1400_0002, 0x5280_004A, 0x3910_212A, 0x9110_4129, 0x5100_0508,
            0x35FF_FDA8, 0xD65F_03C0,
        ],
    },
    GamePatch {
        module_id: MK8DX_401,
        site: 0x3c568c,
        from: &[0x3200_03F4, 0x7100_111F, 0x5400_0080, 0x7100_1D1F, 0x5400_0040, 0x2A1F_03F4],
        to: &[0xF000_7D09, 0x3941_6134, 0x7100_111F, 0x7A47_1904, 0x1A9F_0294, 0xD503_201F],
    },
    GamePatch {
        module_id: MK8DX_401,
        site: 0x3c5e0c,
        from: &[0x7100_0D3F, 0x5400_0141, 0x3200_03E9, 0x3900_0109, 0xB000_7A08, 0xF946_5508, 0xF940_0108, 0xB940_8D09, 0x3200_0129, 0xB900_8D09, 0x1400_0002, 0x3900_011F],
        to: &[0x7100_0D3F, 0xF000_7D09, 0x3941_6129, 0x1A9F_0129, 0x3900_0109, 0x3400_00E9, 0xB000_7A08, 0xF946_5508, 0xF940_0108, 0xB940_8D09, 0x3200_0129, 0xB900_8D09],
    },
    GamePatch { module_id: MK8DX_305, site: 0x4d54bc, from: &[0x9400_0035], to: &[0x9400_0711] },
    GamePatch { module_id: MK8DX_305, site: 0x4d55a4, from: &[0x2A01_03F3], to: &[0x2A02_03F3] },
    GamePatch { module_id: MK8DX_305, site: 0x4d55b4, from: &[0x1200_0276], to: &[0x1200_0036] },
    GamePatch { module_id: MK8DX_305, site: 0x4d5660, from: &[0x1A88_0513], to: &[0xD503_201F] },
    GamePatch {
        module_id: MK8DX_305,
        site: 0x4d70fc,
        from: &[
            0x2A1F_03E0, 0x940F_D165, 0xB400_0640, 0x940F_CD3C, 0xB400_0600, 0xF940_AC08, 0xB400_05C8, 0xB941_150A, 0x528C_1009,
            0x0A2A_012A, 0x3500_054A, 0x7940_1108, 0x0A09_0108, 0x3400_04E8, 0x3200_07E8, 0x9100_63E0, 0xB900_1BE8, 0x940F_D12B,
            0xD000_70E8, 0xF947_3908, 0x08DF_FD08, 0xAA00_03F4, 0x3700_01C8, 0xD000_70E0, 0xF947_3800, 0x9419_DBCE, 0x3400_0140,
            0xD000_70E8, 0xF947_3D08, 0xD000_70E9, 0xF947_4129, 0x9100_4108, 0xF900_0128, 0xD000_70E0, 0xF947_3800, 0x9419_DBC8,
            0xF940_0288, 0xD000_70E1, 0xF940_0108, 0xF947_4021, 0xAA14_03E0, 0xD63F_0100, 0x7200_001F, 0xAA13_03E0, 0x9A9F_1288,
            0xF940_1908, 0x5283_9329, 0x2A1F_03E2, 0x3869_6908, 0x7100_011F, 0x1A9F_17E1, 0x97FF_F8F2,
        ],
        to: &[
            0x1400_0034, 0xA95E_2408, 0xF940_2108, 0xF940_0D08, 0xF940_2129, 0xF940_0D29, 0xA95D_2C0A, 0xF940_214A, 0xF940_0D4A,
            0xF940_216B, 0xF940_0D6B, 0x3941_610C, 0x121F_198C, 0x3901_610C, 0xBD40_3541, 0xBD40_3562, 0x1E21_3841, 0xBD40_3500,
            0xB000_746E, 0x3940_61CE, 0x3400_016E, 0x1E2C_1002, 0x1E22_0822, 0x1E22_3800, 0xF940_190D, 0xF900_192D, 0x1E21_2802,
            0xBD00_3522, 0x3941_612C, 0x321C_018C, 0x1400_0003, 0x3941_612C, 0x121F_198C, 0x3901_612C, 0xBD00_3560, 0x1E21_3802,
            0xBD00_3542, 0x3941_616C, 0x321C_018C, 0x3901_616C, 0x3941_614C, 0x321C_018C, 0x3901_614C, 0x5280_0021, 0x5281_07A2,
            0x7100_0DDF, 0x5281_078D, 0x1A82_01A2, 0x7100_09DF, 0x5281_072D, 0x1A82_01A2, 0x17FF_F8F2,
        ],
    },
    GamePatch {
        module_id: MK8DX_305,
        site: 0x3fa344,
        from: &[
            0x121B_0048, 0x121A_0049, 0x7100_0C3F, 0x5400_0188, 0x5280_014A, 0x1A81_054B, 0x7100_013F, 0x1A8B_0029, 0x5100_042B,
            0x7100_003F, 0x1A8B_014A, 0x7100_011F, 0x1A8A_0121, 0x2A01_03E0, 0xD65F_03C0, 0x3400_0088, 0x3200_07E1, 0x2A01_03E0,
            0xD65F_03C0, 0x3500_0109, 0x3738_0142, 0x3640_0082, 0x7100_283F, 0x321E_03E8, 0x1A81_0501, 0x2A01_03E0, 0xD65F_03C0,
            0x2A1F_03E1, 0x2A01_03E0, 0xD65F_03C0, 0x5100_0428, 0x7100_103F, 0x5280_0149, 0x1A88_0121, 0x2A01_03E0, 0xD65F_03C0,
        ],
        to: &[
            0xD000_7B4B, 0x3940_616B, 0x7100_017F, 0x5280_006C, 0x1A9F_158B, 0x5280_014A, 0x7100_0C3F, 0x5400_0168, 0x3728_00A2,
            0x3630_0222, 0x531F_7820, 0x6B0B_003F, 0x1400_0004, 0x5100_0C20, 0x1301_7C00, 0x7100_003F, 0x1A80_0540, 0xD65F_03C0,
            0x3728_0142, 0x3730_0162, 0x3738_0182, 0x3640_00A2, 0x7100_283F, 0x5280_0088, 0x1A81_0500, 0xD65F_03C0, 0x2A01_03E0,
            0xD65F_03C0, 0x2A0B_03E0, 0xD65F_03C0, 0x2A1F_03E0, 0xD65F_03C0, 0x5100_0428, 0x7100_103F, 0x1A88_0140, 0xD65F_03C0,
        ],
    },
    GamePatch {
        module_id: MK8DX_305,
        site: 0x49fea8,
        from: &[
            0x5280_0D68, 0x5281_1B40, 0xB900_4A88, 0x9410_68A3, 0xAA00_03E1, 0xAA16_03E0, 0x9410_68B2, 0xB988_5A68, 0xB948_5E69,
            0x6B09_011F, 0x5400_00CA, 0xF944_3269, 0xF828_7934, 0xB948_5A68, 0x1100_0508, 0xB908_5A68,
        ],
        to: &[
            0xAA14_03F7, 0x1400_000F, 0xAA0D_03E9, 0xEB0C_015F, 0x5400_0080, 0xF840_8549, 0xF940_2129, 0xF940_0D29, 0xB940_352B,
            0xB900_350B, 0x3941_610B, 0x321C_016B, 0x3901_610B, 0xAA09_03E8, 0x54FF_FE81, 0x1400_0032,
        ],
    },
    GamePatch {
        module_id: MK8DX_305,
        site: 0x49ff6c,
        from: &[
            0x5280_0D88, 0x5281_1C80, 0xB900_4A88, 0x9410_6872, 0xAA00_03E1, 0xAA16_03E0, 0x9410_6881, 0xB988_5A68, 0xB948_5E69,
            0x6B09_011F, 0x5400_00CA, 0xF944_3269, 0xF828_7934, 0xB948_5A68, 0x1100_0508, 0xB908_5A68,
        ],
        to: &[
            0xF940_2288, 0xF940_0D08, 0x3941_6109, 0x121F_1929, 0x3901_6109, 0xF940_22ED, 0xF940_0DAD, 0x3941_61A9, 0x121F_1929,
            0x3901_61A9, 0xF944_326A, 0x9100_614C, 0xF840_8548, 0xF940_2108, 0xF940_0D08, 0x1400_058B,
        ],
    },
    GamePatch {
        module_id: MK8DX_305,
        site: 0x4a15d0,
        from: &[
            0xB948_5A68, 0x7100_111F, 0x5400_0083, 0xF944_3268, 0xF940_0D00, 0x1400_0002, 0xAA1F_03E0, 0x2A15_03E1,
        ],
        to: &[
            0x1400_0013, 0xB940_8A89, 0x1200_7929, 0xB900_8A89, 0xB940_8AE9, 0x1200_7929, 0xB900_8AE9, 0x17FF_FA31,
        ],
    },
    GamePatch { module_id: MK8DX_305, site: 0x543254, from: &[0x3400_23C8], to: &[0x1400_011E] },
    GamePatch { module_id: MK8DX_305, site: 0x88a514, from: &[0xB219_0FE0, 0xF2C0_8700, 0xD65F_03C0], to: &[0xD000_56C0, 0xF940_1000, 0xD65F_03C0] },
    GamePatch {
        module_id: MK8DX_305,
        site: 0x3c51e8,
        from: &[0x320B_07E8, 0xF900_23E8, 0x52A0_B008, 0xF900_27E8],
        to: &[0xF000_7CE8, 0xF940_1508, 0x52A0_0C09, 0xA904_23E9],
    },
    GamePatch { module_id: MK8DX_305, site: 0x3d46bc, from: &[0xB940_0BE8, 0xB940_0BE8, 0x3204_03E8], to: &[0x9000_7C88, 0xB940_3108, 0xD503_201F] },
    GamePatch { module_id: MK8DX_305, site: 0x7f01f0, from: &[0x52B6_0A08, 0x9100_63E0, 0xF900_0FE8], to: &[0x9000_5BA8, 0xF940_1D08, 0xF900_0FE8] },
    GamePatch {
        module_id: MK8DX_305,
        site: 0x7f0154,
        from: &[
            0xD100_83FF, 0xA901_7BFD, 0x9100_43FD, 0x52B8_3800, 0x940D_7DF5, 0x9100_23E0, 0x3209_07E1, 0x940D_7D66, 0xF940_07E0,
            0x3209_07E1, 0x97F6_7D4E, 0xA941_7BFD, 0x9100_83FF, 0xD65F_03C0,
        ],
        to: &[
            0xA9BE_7BFD, 0x9100_03FD, 0x9000_5BA0, 0xF940_2000, 0x940D_7DF5, 0x9100_43E0, 0x3209_07E1, 0x940D_7D66, 0xF940_0BE0,
            0x3209_07E1, 0x97F6_7D4E, 0xA8C2_7BFD, 0xD65F_03C0, 0xD503_201F,
        ],
    },
    GamePatch {
        module_id: MK8DX_305,
        site: 0xafa2a8,
        from: &[
            0x3940_6008, 0x1100_0508, 0x3900_6008, 0xB940_0808, 0x3400_0308, 0xF940_0809, 0x3940_640A, 0x7100_095F, 0x5400_01A3,
            0x39D0_252B, 0x3100_057F, 0x5400_0140, 0x3940_600C, 0x1ACA_098D, 0x1B0A_B1AA, 0x6B0B_015F, 0x5400_00E0, 0xF940_012A,
            0xB400_00AA, 0x3200_03EA, 0x1400_0004, 0x2A1F_03EA, 0x1400_0002, 0x321F_03EA, 0x3910_212A, 0x9110_4129, 0xD100_0508,
            0xB5FF_FD68, 0xD65F_03C0,
        ],
        to: &[
            0x3940_600C, 0x1100_058C, 0x3900_600C, 0x3940_600C, 0xA940_A408, 0x3400_02E8, 0xD000_434E, 0x3941_21CE, 0x3940_640A,
            0x6B0E_015F, 0x5400_0163, 0x39D0_252B, 0x37F8_012B, 0x1ACA_098D, 0x1B0A_B1AA, 0x6B0B_015F, 0x5400_00E0, 0xF940_012A,
            0xB400_00AA, 0x5280_002A, 0x1400_0004, 0x2A1F_03EA, 0x1400_0002, 0x5280_004A, 0x3910_212A, 0x9110_4129, 0x5100_0508,
            0x35FF_FDA8, 0xD65F_03C0,
        ],
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    X18,
    SysReg,
    Svc,
    Exclusive,
    JumpTable,
    Online,
}

#[derive(Debug, Clone)]
pub struct Patch {
    pub kind: Kind,
    pub start: u64,
    pub end: u64,
    pub tramp: u64,
    pub tramp_len: u64,
}

pub struct Output {
    pub image: Vec<u8>,
    pub text_size: u64,
    pub ro: (u64, u64),
    pub data: (u64, u64),
    pub bss: (u64, u64),
    pub keypage: u64,
    pub tramp: (u64, u64),
    pub patches: Vec<Patch>,
}

fn align_up(v: u64, a: u64) -> u64 {
    (v + a - 1) & !(a - 1)
}

fn field_regs(w: u32) -> u32 {
    [0, 5, 10, 16].iter().fold(0, |m, &s| m | 1 << ((w >> s) & 31))
}

fn replace_reg_tokens(text: &str, from: u32, to: u32) -> String {
    let mut out = String::new();
    let mut tok = String::new();
    let flush = |tok: &mut String, out: &mut String| {
        for p in ["x", "w"] {
            if *tok == format!("{p}{from}") {
                *tok = format!("{p}{to}");
            }
        }
        out.push_str(tok);
        tok.clear();
    };
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            tok.push(c);
        } else {
            flush(&mut tok, &mut out);
            out.push(c);
        }
    }
    flush(&mut tok, &mut out);
    out
}

struct Ctx<'a> {
    cs: Capstone,
    module_id: String,
    words: &'a [u32],
    image: &'a [u8],
}

impl Ctx<'_> {
    fn text(&self, w: u32, pc: u64) -> Result<String> {
        let b = w.to_le_bytes();
        let insns = self.cs.disasm_count(&b, pc, 1)?;
        let i = insns.iter().next().with_context(|| format!("undecodable word {w:#010x} at {pc:#x}"))?;
        Ok(format!("{} {}", i.mnemonic().unwrap_or(""), i.op_str().unwrap_or("")))
    }

    fn regs(&self, w: u32, pc: u64) -> Result<u32> {
        let b = w.to_le_bytes();
        let insns = self.cs.disasm_count(&b, pc, 1)?;
        let mut m = field_regs(w);
        if let Some(i) = insns.iter().next() {
            let d = self.cs.insn_detail(i)?;
            for r in d.regs_read().iter().chain(d.regs_write()) {
                if let Some(g) = self.cs.reg_name(*r).as_deref().and_then(classify::gpr_index) {
                    m |= 1 << g;
                }
            }
        }
        Ok(m)
    }

    fn subst18(&self, w: u32, pc: u64, s: Reg) -> Result<u32> {
        let want = replace_reg_tokens(&self.text(w, pc)?, 18, s);
        let fields: Vec<u32> = [0, 5, 10, 16].into_iter().filter(|&f| (w >> f) & 31 == 18).collect();
        for mask in 1..(1u32 << fields.len()) {
            let mut n = w;
            for (i, f) in fields.iter().enumerate() {
                if mask & (1 << i) != 0 {
                    n = (n & !(31 << f)) | (s << f);
                }
            }
            if self.text(n, pc).ok().as_deref() == Some(want.as_str()) {
                return Ok(n);
            }
        }
        bail!("cannot rename x18 in `{}` at {pc:#x}", self.text(w, pc)?)
    }

    fn writes_sp(&self, w: u32, pc: u64) -> Result<bool> {
        let t = self.text(w, pc)?;
        let has_sp = t.split(|c: char| !c.is_ascii_alphanumeric()).any(|x| x == "sp" || x == "wsp");
        let first_is_sp = t.split_whitespace().nth(1).is_some_and(|o| o.starts_with("sp,") || o.starts_with("wsp,"));
        Ok(has_sp && (t.contains('!') || t.contains("],") || first_is_sp))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Need {
    Plain,
    X18,
    SysReg,
}

struct Emitter {
    base: u64,
    words: Vec<u32>,
    fixups: Vec<(usize, usize)>,
    labels: Vec<Option<usize>>,
}

impl Emitter {
    fn pc(&self) -> u64 {
        self.base + self.words.len() as u64 * 4
    }
    fn emit(&mut self, w: u32) {
        self.words.push(w);
    }
    fn label(&mut self) -> usize {
        self.labels.push(None);
        self.labels.len() - 1
    }
    fn bind(&mut self, l: usize) {
        self.labels[l] = Some(self.words.len());
    }
    fn cond_to(&mut self, w: u32, l: usize) {
        self.fixups.push((self.words.len(), l));
        self.words.push(w);
    }
    fn resolve(&mut self) -> Result<()> {
        for &(at, l) in &self.fixups {
            let to = self.labels[l].context("unbound label")?;
            let from = self.base + at as u64 * 4;
            self.words[at] = asm::retarget(self.words[at], from, self.base + to as u64 * 4)?;
        }
        self.fixups.clear();
        Ok(())
    }
}

struct Scratch {
    s: Reg,
    t: Reg,
    u: Option<Reg>,
    ctx_spill: bool,
}

const SCRATCH_ORDER: [Reg; 28] = [9, 10, 11, 12, 13, 14, 15, 16, 17, 0, 1, 2, 3, 4, 5, 6, 7, 8, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28];

impl Scratch {
    fn prologue(&self, e: &mut Emitter, keypage: u64) -> Result<()> {
        let (s, t) = (self.s, self.t);
        e.emit(asm::stp(s, t, SP, -16));
        if let Some(u) = self.u {
            e.emit(asm::stur(u, SP, -24));
        }
        e.emit(asm::mrs_tpidrro(s));
        let pc = e.pc();
        e.emit(asm::adrp(t, pc, keypage)?);
        e.emit(asm::ldr(t, t, keypage::CTX_SLOT));
        e.emit(asm::ldr_reg(t, s, t));
        if self.ctx_spill {
            e.emit(asm::ldur(s, SP, -16));
            e.emit(asm::str(s, t, ctx::SPILL0));
            e.emit(asm::ldur(s, SP, -8));
            e.emit(asm::str(s, t, ctx::SPILL1));
            if self.u.is_some() {
                e.emit(asm::ldur(s, SP, -24));
                e.emit(asm::str(s, t, ctx::SPILL2));
            }
        }
        e.emit(asm::ldr(s, t, ctx::X18));
        Ok(())
    }

    fn epilogue(&self, e: &mut Emitter, store: bool, dest: u64) -> Result<()> {
        let (s, t) = (self.s, self.t);
        if store {
            e.emit(asm::str(s, t, ctx::X18));
        }
        if self.ctx_spill {
            if let Some(u) = self.u {
                e.emit(asm::ldr(u, t, ctx::SPILL2));
            }
            e.emit(asm::ldr(s, t, ctx::SPILL0));
            e.emit(asm::ldr(t, t, ctx::SPILL1));
        } else {
            e.emit(asm::ldp(s, t, SP, -16));
            if let Some(u) = self.u {
                e.emit(asm::ldur(u, SP, -24));
            }
        }
        let pc = e.pc();
        e.emit(asm::b(pc, dest)?);
        Ok(())
    }
}

pub fn rewrite(nso: &nxfs::nso::Nso) -> Result<Output> {
    rewrite_skipping(nso, &[])
}

pub fn rewrite_skipping(nso: &nxfs::nso::Nso, skip: &[u64]) -> Result<Output> {
    let text = nso.text_bytes();
    ensure!(nso.text.mem_offset == 0, "text must start at 0");
    let mut words: Vec<u32> = text.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
    let module_id = hex::encode(&nso.module_id[..20]);
    for p in GAME_PATCHES.iter().filter(|p| p.module_id == module_id && !skip.contains(&p.site)) {
        let w = &mut words[p.site as usize / 4..][..p.from.len()];
        ensure!(w == p.from, "game patch at {:#x}: code differs from the expected original", p.site);
        w.copy_from_slice(p.to);
    }
    let cx = Ctx { cs: classify::capstone()?, module_id, words: &words, image: &nso.image };

    let mod0 = u32::from_le_bytes(nso.image[4..8].try_into()?) as usize;
    let is_header = |i: usize| i < 2 || (mod0 / 4..mod0 / 4 + 7).contains(&i);
    let mut need = vec![Need::Plain; words.len()];
    for s in classify::find_sites(text)? {
        if !is_header(s.offset / 4) {
            need[s.offset / 4] = Need::X18;
        }
    }
    let mut svcs = Vec::new();
    for (i, &w) in words.iter().enumerate() {
        if is_header(i) {
            continue;
        }
        if asm::is_svc(w) {
            svcs.push(i);
        } else if asm::is_mrs(w) || asm::is_msr_reg(w) {
            match asm::sysreg(w) {
                asm::SYSREG_TPIDR_EL0 => need[i] = Need::SysReg,
                asm::SYSREG_TPIDRRO_EL0 | asm::SYSREG_CNTPCT_EL0 | asm::SYSREG_CTR_EL0 if asm::is_mrs(w) => need[i] = Need::SysReg,
                0x5807 | 0x5A10 | 0x5A20 | 0x5A21 => {}
                r => bail!("unhandled system register {r:#x} at {:#x}", i * 4),
            }
        }
    }

    let mut regions: Vec<(usize, usize, Kind)> = Vec::new();
    for i in 0..words.len() {
        if !asm::is_load_exclusive(words[i]) {
            continue;
        }
        let end = (i + 1..(i + 16).min(words.len())).find(|&j| asm::is_store_exclusive(words[j]));
        if let Some(j) = end {
            if (i..=j).any(|k| need[k] != Need::Plain) {
                regions.push((i, j + 1, Kind::Exclusive));
            }
        }
    }
    let window_starts: BTreeSet<usize> = regions.iter().map(|r| r.0).collect();
    for i in 0..words.len() {
        if need[i] != Need::Plain && !window_starts.contains(&i) {
            let kind = if need[i] == Need::SysReg { Kind::SysReg } else { Kind::X18 };
            regions.push((i, i + 1, kind));
        }
    }
    regions.sort();

    let bss_start = (nso.data.mem_offset + nso.data.size) as u64;
    let bss_end = bss_start + nso.bss_size as u64;
    let keypage = align_up(bss_end, keypage::SIZE);
    let game = GAMES.iter().find(|g| g.module_id == cx.module_id);
    if let Some(g) = game {
        ensure!(keypage == g.keypage, "game patches expect the keypage at {:#x}", g.keypage);
    }
    let tramp_base = keypage + keypage::SIZE;
    let mut e = Emitter { base: tramp_base, words: Vec::new(), fixups: Vec::new(), labels: Vec::new() };
    let mut patched = words.clone();
    let mut patches = Vec::new();

    for &(start, end, kind) in &regions {
        let tramp = e.pc();
        let kind = emit_region(&cx, &mut e, &need, start, end, kind, keypage)
            .with_context(|| format!("region at {:#x}", start * 4))?;
        e.resolve()?;
        patched[start] = asm::b(start as u64 * 4, tramp)?;
        patches.push(Patch { kind, start: start as u64 * 4, end: end as u64 * 4, tramp, tramp_len: e.pc() - tramp });
    }
    for &i in &svcs {
        let tramp = e.pc();
        let n = (words[i] >> 5) & 0xFFFF;
        e.emit(asm::stp_pre(16, 30, SP, -16));
        e.emit(asm::movz(16, n, 0));
        let pc = e.pc();
        e.emit(asm::adrp(30, pc, keypage)?);
        e.emit(asm::ldr(30, 30, keypage::SVC_GATE));
        e.emit(asm::blr(30));
        e.emit(asm::ldp_post(16, 30, SP, 16));
        let pc = e.pc();
        e.emit(asm::b(pc, (i as u64 + 1) * 4)?);
        patched[i] = asm::b(i as u64 * 4, tramp)?;
        patches.push(Patch { kind: Kind::Svc, start: i as u64 * 4, end: i as u64 * 4 + 4, tramp, tramp_len: e.pc() - tramp });
    }

    let online: Vec<&GamePatch> = GAME_PATCHES
        .iter()
        .filter(|p| p.module_id == cx.module_id && !skip.contains(&p.site) && group_of(&cx.module_id, p.site).is_some_and(|g| g.online))
        .filter(|p| !matches!(nextendo_at(&cx.module_id, p.site), Nextendo::Patched))
        .collect();
    check_online_entries(&cx, &words, &online)?;
    for p in &online {
        let i = p.site as usize / 4;
        let end = p.site + 4 * p.from.len() as u64;
        ensure!(patched[i] == words[i] && !patches.iter().any(|q| q.start < end && p.site < q.end), "online patch at {:#x} overlaps another rewrite", p.site);
        let tramp = e.pc();
        let prefix = match nextendo_at(&cx.module_id, p.site) {
            Nextendo::Prefix(words) => words,
            _ => &[],
        };
        emit_online_switch(&cx, &mut e, keypage, p.site, prefix, p.from, p.to).with_context(|| format!("online patch at {:#x}", p.site))?;
        patched[i] = asm::b(p.site, tramp)?;
        patches.push(Patch { kind: Kind::Online, start: p.site, end: p.site + 4 * p.from.len() as u64, tramp, tramp_len: e.pc() - tramp });
    }

    if let Some(g) = game {
        for &site in g.aspect_stores {
            let i = site as usize / 4;
            ensure!(patched[i] == words[i], "camera aspect store at {site:#x} overlaps another rewrite");
            let tramp = e.pc();
            emit_aspect_scale(&mut e, keypage, words[i], site + 4).with_context(|| format!("camera aspect store at {site:#x}"))?;
            patched[i] = asm::b(site, tramp)?;
        }
    }

    let tramp_end = e.pc();
    let mut image = vec![0u8; tramp_end as usize];
    image[..nso.image.len()].copy_from_slice(&nso.image);
    for (i, w) in patched.iter().enumerate() {
        image[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }
    for (i, w) in e.words.iter().enumerate() {
        let o = tramp_base as usize + i * 4;
        image[o..o + 4].copy_from_slice(&w.to_le_bytes());
    }
    Ok(Output {
        image,
        text_size: nso.text.size as u64,
        ro: (nso.ro.mem_offset as u64, nso.ro.size as u64),
        data: (nso.data.mem_offset as u64, nso.data.size as u64),
        bss: (bss_start, nso.bss_size as u64),
        keypage,
        tramp: (tramp_base, tramp_end),
        patches,
    })
}

fn emit_online_switch(cx: &Ctx, e: &mut Emitter, keypage: u64, site: u64, prefix: &[u32], from: &[u32], to: &[u32]) -> Result<()> {
    ensure!(!from.is_empty() && from.len() == to.len(), "patch blocks differ in length");
    let end = site + 4 * from.len() as u64;
    e.emit(asm::stur(16, SP, -8));
    let pc = e.pc();
    e.emit(asm::adrp(16, pc, keypage)?);
    e.emit(asm::ldrb_w(16, 16, keypage::NET));
    e.emit(asm::sub_imm_w(16, 16, keypage::NET_NEXTENDO as u32));
    let original = e.label();
    e.cond_to(asm::cbz_w(16, 0), original);
    e.emit(asm::ldur(16, SP, -8));
    let mut exits = BTreeMap::new();
    relocate(cx, e, &to[..1], site, false, &mut exits)?;
    if !asm::is_b(to[0]) {
        let pc = e.pc();
        e.emit(asm::b(pc, site + 4)?);
    }
    e.bind(original);
    e.emit(asm::ldur(16, SP, -8));
    for &w in prefix {
        ensure!(!asm::is_cond_branch(w) && !asm::is_uncond_branch(w) && !asm::is_adr(w) && !asm::is_adrp(w) && !asm::is_ldr_literal(w), "prefix word {w:#010x} depends on its address");
        e.emit(w);
    }
    relocate(cx, e, from, site, true, &mut exits)?;
    if !asm::is_b(from[from.len() - 1]) {
        let pc = e.pc();
        e.emit(asm::b(pc, end)?);
    }
    for (target, l) in exits {
        e.bind(l);
        let pc = e.pc();
        e.emit(asm::b(pc, target)?);
    }
    e.resolve()
}

fn relocate(cx: &Ctx, e: &mut Emitter, block: &[u32], base: u64, internal: bool, exits: &mut BTreeMap<u64, usize>) -> Result<()> {
    let end = base + 4 * block.len() as u64;
    let labels: Vec<usize> = block.iter().map(|_| e.label()).collect();
    let local = |t: u64| (internal && (base..end).contains(&t) && t % 4 == 0).then(|| labels[((t - base) / 4) as usize]);
    let mut jumps = Vec::new();
    for (k, &w) in block.iter().enumerate() {
        e.bind(labels[k]);
        let pc = base + 4 * k as u64;
        let text = cx.text(w, pc)?;
        let uses_x18 = text.split(|c: char| !c.is_ascii_alphanumeric()).any(|t| t == "x18" || t == "w18");
        let special = asm::is_svc(w) || asm::is_mrs(w) || asm::is_msr_reg(w) || asm::is_load_exclusive(w) || asm::is_store_exclusive(w);
        ensure!(!uses_x18 && !special, "`{text}` at {pc:#x} needs its own rewrite");
        if asm::is_adrp(w) || asm::is_adr(w) {
            let (rd, target) = (w & 31, asm::adr_target(w, pc));
            let here = e.pc();
            e.emit(asm::adrp(rd, here, target)?);
            if target & 0xFFF != 0 {
                e.emit(asm::add_imm(rd, rd, (target & 0xFFF) as u32));
            }
        } else if asm::is_ldr_literal(w) {
            ensure!((w >> 26) & 1 == 0, "SIMD literal load at {pc:#x}");
            let (rt, target) = (w & 31, asm::ldr_literal_target(w, pc));
            let here = e.pc();
            e.emit(asm::adrp(rt, here, target)?);
            e.emit(asm::add_imm(rt, rt, (target & 0xFFF) as u32));
            e.emit(asm::ldr_literal_as_base(w, rt).context("prfm literal")?);
        } else if asm::is_cond_branch(w) {
            let t = asm::cond_target(w, pc);
            let l = match local(t) {
                Some(l) => l,
                None => *exits.entry(t).or_insert_with(|| e.label()),
            };
            e.cond_to(w, l);
        } else if asm::is_b(w) || asm::is_bl(w) {
            let t = asm::branch_target(w, pc);
            match local(t) {
                Some(l) => {
                    ensure!(asm::is_b(w), "bl into its own block at {pc:#x}");
                    jumps.push((e.words.len(), l));
                    e.emit(w);
                }
                None => {
                    let here = e.pc();
                    e.emit(if asm::is_bl(w) { asm::bl(here, t)? } else { asm::b(here, t)? });
                }
            }
        } else {
            e.emit(w);
        }
    }
    for (at, l) in jumps {
        let to = e.labels[l].context("unbound label")?;
        e.words[at] = asm::b(e.base + at as u64 * 4, e.base + to as u64 * 4)?;
    }
    Ok(())
}

fn check_online_entries(cx: &Ctx, words: &[u32], online: &[&GamePatch]) -> Result<()> {
    let spans: Vec<(u64, u64)> = online.iter().map(|p| (p.site, p.site + 4 * p.from.len() as u64)).collect();
    let check = |w: u32, pc: u64, own: Option<(u64, u64)>| -> Result<()> {
        let target = if asm::is_b(w) || asm::is_bl(w) {
            asm::branch_target(w, pc)
        } else if asm::is_cond_branch(w) {
            asm::cond_target(w, pc)
        } else {
            return Ok(());
        };
        if let Some(&span) = spans.iter().find(|(s, e)| target > *s && target < *e) {
            ensure!(own == Some(span), "`{}` at {pc:#x} enters the patch at {:#x} past its start", cx.text(w, pc)?, span.0);
        }
        Ok(())
    };
    for (i, &w) in words.iter().enumerate() {
        let pc = i as u64 * 4;
        if !spans.iter().any(|(s, e)| (*s..*e).contains(&pc)) {
            check(w, pc, None)?;
        }
    }
    for (p, &span) in online.iter().zip(&spans) {
        for (k, &w) in p.from.iter().enumerate() {
            check(w, p.site + 4 * k as u64, Some(span))?;
        }
    }
    Ok(())
}

fn emit_aspect_scale(e: &mut Emitter, keypage: u64, store: u32, back: u64) -> Result<()> {
    ensure!(asm::is_str_s(store) && store & 0x1F == 0 && ![16, 17, SP].contains(&((store >> 5) & 0x1F)), "expected str s0, [xN, #imm], found {store:#010x}");
    e.emit(asm::stp_pre(16, 17, SP, -32));
    e.emit(asm::str_q(1, SP, 16));
    let pc = e.pc();
    e.emit(asm::adrp(16, pc, keypage)?);
    e.emit(asm::ldr_s(1, 16, keypage::ASPECT));
    e.emit(asm::fmov_w_s(17, 1));
    e.emit(asm::lsl_w(17, 17, 1));
    e.emit(asm::cbz_w(17, 8));
    e.emit(asm::fmul_s(0, 0, 1));
    e.emit(asm::ldr_q(1, SP, 16));
    e.emit(asm::ldp_post(16, 17, SP, 32));
    e.emit(store);
    let pc = e.pc();
    e.emit(asm::b(pc, back)?);
    Ok(())
}

fn emit_region(cx: &Ctx, e: &mut Emitter, need: &[Need], start: usize, end: usize, kind: Kind, keypage: u64) -> Result<Kind> {
    let words = cx.words;
    let pc_of = |i: usize| i as u64 * 4;

    let mut avoid = (1u32 << 18) | (1 << 29) | (1 << 30) | (1 << 31);
    let mut ctx_spill = false;
    let mut needs_u = false;
    for i in start..end {
        avoid |= cx.regs(words[i], pc_of(i))?;
        ctx_spill |= cx.writes_sp(words[i], pc_of(i))?;
        needs_u |= need[i] == Need::SysReg && asm::sysreg(words[i]) == asm::SYSREG_CNTPCT_EL0;
        ensure!(i + 1 == end || !asm::is_uncond_branch(words[i]), "branch inside a multi-instruction region");
    }
    let mut free = SCRATCH_ORDER.iter().copied().filter(|r| avoid & (1 << r) == 0);
    let sc = Scratch {
        s: free.next().context("no scratch register")?,
        t: free.next().context("no scratch register")?,
        u: if needs_u { Some(free.next().context("no scratch register")?) } else { None },
        ctx_spill,
    };

    if end - start == 1 && words[start] == 0xD61F_0240 {
        let jt = JUMP_TABLES
            .iter()
            .find(|j| j.module_id == cx.module_id && j.site == pc_of(start))
            .with_context(|| format!("br x18 at {:#x} is not a known jump table", pc_of(start)))?;
        return emit_jump_table(cx, e, &sc, jt, keypage).map(|_| Kind::JumpTable);
    }

    sc.prologue(e, keypage)?;
    let labels: Vec<usize> = (start..end).map(|_| e.label()).collect();
    let mut exits: BTreeMap<u64, usize> = BTreeMap::new();
    for i in start..end {
        e.bind(labels[i - start]);
        let (w, pc) = (words[i], pc_of(i));
        let d18 = |r: u32| if r == 18 { sc.s } else { r };
        match need[i] {
            Need::SysReg => {
                let rt = d18(w & 31);
                match asm::sysreg(w) {
                    asm::SYSREG_TPIDR_EL0 if asm::is_mrs(w) => e.emit(asm::ldr(rt, sc.t, ctx::TPIDR)),
                    asm::SYSREG_TPIDR_EL0 => e.emit(asm::str(rt, sc.t, ctx::TPIDR)),
                    asm::SYSREG_TPIDRRO_EL0 => e.emit(asm::ldr(rt, sc.t, ctx::TPIDRRO)),
                    asm::SYSREG_CTR_EL0 => {
                        e.emit(asm::movz(rt, CTR_EL0 & 0xFFFF, 0));
                        e.emit(asm::movk(rt, CTR_EL0 >> 16, 16));
                    }
                    asm::SYSREG_CNTPCT_EL0 => {
                        let u = sc.u.unwrap();
                        e.emit(asm::mrs_cntvct(rt));
                        let here = e.pc();
                        e.emit(asm::adrp(u, here, keypage)?);
                        e.emit(asm::ldr(u, u, keypage::CNT_MUL));
                        e.emit(asm::umulh(rt, rt, u));
                    }
                    r => bail!("sysreg {r:#x}"),
                }
            }
            n => {
                let renamed = if n == Need::X18 && !asm::is_adr(w) && !asm::is_adrp(w) && !asm::is_ldr_literal(w) {
                    cx.subst18(w, pc, sc.s)?
                } else {
                    w
                };
                if asm::is_adrp(w) || asm::is_adr(w) {
                    let (rd, target) = (d18(w & 31), asm::adr_target(w, pc));
                    let here = e.pc();
                    e.emit(asm::adrp(rd, here, target)?);
                    if target & 0xFFF != 0 {
                        e.emit(asm::add_imm(rd, rd, (target & 0xFFF) as u32));
                    }
                } else if asm::is_ldr_literal(w) {
                    let rt = w & 31;
                    ensure!((w >> 26) & 1 == 0, "SIMD literal load in region");
                    let rd = d18(rt);
                    let target = asm::ldr_literal_target(w, pc);
                    let here = e.pc();
                    e.emit(asm::adrp(rd, here, target)?);
                    e.emit(asm::add_imm(rd, rd, (target & 0xFFF) as u32));
                    e.emit(asm::ldr_literal_as_base((w & !31) | rd, rd).context("prfm literal")?);
                } else if asm::is_cond_branch(w) {
                    let target = asm::cond_target(w, pc);
                    let ti = target / 4;
                    let l = if (start as u64..end as u64).contains(&ti) {
                        labels[ti as usize - start]
                    } else {
                        *exits.entry(target).or_insert_with(|| e.label())
                    };
                    e.cond_to(renamed, l);
                } else {
                    ensure!(!asm::is_uncond_branch(w), "unsupported branch `{}` at {pc:#x}", cx.text(w, pc)?);
                    e.emit(renamed);
                }
            }
        }
    }
    sc.epilogue(e, true, pc_of(end))?;
    for (target, l) in exits {
        e.bind(l);
        sc.epilogue(e, true, target)?;
    }
    Ok(kind)
}

fn emit_jump_table(cx: &Ctx, e: &mut Emitter, sc: &Scratch, jt: &JumpTable, keypage: u64) -> Result<()> {
    ensure!(!sc.ctx_spill && sc.u.is_none());
    let read_i32 = |addr: u64| -> Result<i32> {
        let b = cx.image.get(addr as usize..addr as usize + 4).context("jump table outside image")?;
        Ok(i32::from_le_bytes(b.try_into()?))
    };
    let mut targets = BTreeSet::new();
    for k in 0..jt.count {
        let t = (jt.table as i64 + read_i32(jt.table + 4 * k as u64)? as i64) as u64;
        ensure!((t as usize) < cx.words.len() * 4, "jump table target {t:#x} outside text");
        targets.insert(t);
    }
    let (s, t) = (sc.s, sc.t);
    sc.prologue(e, keypage)?;
    e.emit(asm::str(s, t, ctx::X18));
    let mut pads = Vec::new();
    for &target in &targets {
        let here = e.pc();
        e.emit(asm::adrp(t, here, target)?);
        e.emit(asm::add_imm(t, t, (target & 0xFFF) as u32));
        e.emit(asm::sub_reg(t, s, t));
        let l = e.label();
        e.cond_to(0xB400_0000 | t, l);
        pads.push((l, target));
    }
    e.emit(asm::brk(0x18));
    for (l, target) in pads {
        e.bind(l);
        sc.epilogue(e, false, target)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicorn_engine::{Arch, Mode, Prot, RegisterARM64 as R, Unicorn};

    #[test]
    fn every_game_patch_has_a_group() {
        for p in GAME_PATCHES {
            assert!(group_of(p.module_id, p.site).is_some(), "game patch at {:#x} has no group", p.site);
        }
        for g in PATCH_GROUPS {
            for &s in g.sites {
                assert!(GAME_PATCHES.iter().any(|p| p.module_id == g.module_id && p.site == s), "{} lists {s:#x}, which is not a game patch", g.name);
            }
        }
    }

    #[test]
    fn aspect_scale_keeps_registers() {
        let (code, keypage, back, camera) = (0x1000u64, 0x4000u64, 0x2000u64, 0x6000u64);
        for (factor, expected) in [(1.22f32, 1.5f32 * 1.22), (0.75, 1.5 * 0.75), (0.0, 1.5), (-0.0, 1.5)] {
            let mut e = Emitter { base: code, words: Vec::new(), fixups: Vec::new(), labels: Vec::new() };
            emit_aspect_scale(&mut e, keypage, asm::str_s(0, 9, 0xB0), back).unwrap();
            let bytes: Vec<u8> = e.words.iter().flat_map(|w| w.to_le_bytes()).collect();
            let mut uc = Unicorn::new(Arch::ARM64, Mode::LITTLE_ENDIAN).unwrap();
            uc.mem_map(0, 0x10000, Prot::ALL).unwrap();
            uc.mem_write(code, &bytes).unwrap();
            uc.mem_write(keypage + keypage::ASPECT as u64, &factor.to_le_bytes()).unwrap();
            uc.reg_write(R::CPACR_EL1, 3 << 20).unwrap();
            uc.reg_write(R::SP, 0x9000).unwrap();
            uc.reg_write(R::X9, camera).unwrap();
            uc.reg_write(R::X16, 0x1616).unwrap();
            uc.reg_write(R::X17, 0x1717).unwrap();
            uc.reg_write(R::NZCV, 0x6000_0000).unwrap();
            uc.reg_write(R::S0, 1.5f32.to_bits() as u64).unwrap();
            uc.reg_write_long(R::Q1, &[0x5A; 16]).unwrap();
            uc.emu_start(code, back, 0, 64).unwrap();
            let mut stored = [0u8; 4];
            uc.mem_read(camera + 0xB0, &mut stored).unwrap();
            assert_eq!(f32::from_le_bytes(stored), expected);
            assert_eq!(uc.reg_read(R::PC).unwrap(), back);
            assert_eq!(uc.reg_read(R::SP).unwrap(), 0x9000);
            assert_eq!((uc.reg_read(R::X16).unwrap(), uc.reg_read(R::X17).unwrap()), (0x1616, 0x1717));
            assert_eq!(uc.reg_read(R::NZCV).unwrap(), 0x6000_0000);
            assert_eq!(&*uc.reg_read_long(R::Q1).unwrap(), &[0x5A; 16]);
        }
    }

    #[test]
    fn online_switch_runs_the_original_in_nextendo_mode() {
        let (site, keypage, tramp) = (0x1000u64, 0x4000u64, 0x8000u64);
        let from = [0xD280_0040, 0xB400_0041, 0x1400_0002, 0xD280_0123, 0xD280_0084];
        let to = [0xD280_0020, 0xD280_0025, 0xD503_201F, 0xD503_201F, 0xD503_201F];
        let cx = Ctx { cs: classify::capstone().unwrap(), module_id: String::new(), words: &[], image: &[] };
        let mut e = Emitter { base: tramp, words: Vec::new(), fixups: Vec::new(), labels: Vec::new() };
        emit_online_switch(&cx, &mut e, keypage, site, &[], &from, &to).unwrap();
        let end = site + 4 * from.len() as u64;
        for (net, x1, expect) in [(0u8, 0u64, (1, 0x33, 0x44, 1)), (2, 0, (1, 0x33, 0x44, 1)), (4, 0, (2, 9, 4, 0x55)), (4, 1, (2, 0x33, 4, 0x55))] {
            let mut uc = Unicorn::new(Arch::ARM64, Mode::LITTLE_ENDIAN).unwrap();
            uc.mem_map(0, 0x10000, Prot::ALL).unwrap();
            let mut code: Vec<u32> = to.to_vec();
            code[0] = asm::b(site, tramp).unwrap();
            uc.mem_write(site, &code.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>()).unwrap();
            uc.mem_write(tramp, &e.words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>()).unwrap();
            uc.mem_write(keypage + keypage::NET as u64, &[net]).unwrap();
            for (r, v) in [(R::X1, x1), (R::X3, 0x33), (R::X4, 0x44), (R::X5, 0x55), (R::X16, 0x1616), (R::SP, 0x9000), (R::NZCV, 0x6000_0000)] {
                uc.reg_write(r, v).unwrap();
            }
            uc.emu_start(site, end, 0, 64).unwrap();
            let got = (uc.reg_read(R::X0).unwrap(), uc.reg_read(R::X3).unwrap(), uc.reg_read(R::X4).unwrap(), uc.reg_read(R::X5).unwrap());
            assert_eq!(got, expect, "net {net} x1 {x1}");
            assert_eq!(uc.reg_read(R::PC).unwrap(), end);
            assert_eq!(uc.reg_read(R::X16).unwrap(), 0x1616);
            assert_eq!(uc.reg_read(R::NZCV).unwrap(), 0x6000_0000);
        }
    }

    #[test]
    fn nextendo_prefix_runs_before_the_original() {
        let (site, keypage, tramp) = (0x1000u64, 0x4000u64, 0x8000u64);
        let cx = Ctx { cs: classify::capstone().unwrap(), module_id: String::new(), words: &[], image: &[] };
        let mut e = Emitter { base: tramp, words: Vec::new(), fixups: Vec::new(), labels: Vec::new() };
        emit_online_switch(&cx, &mut e, keypage, site, &[MOV_W1_1], &[0xD280_0040], &[0xD280_0020]).unwrap();
        for (net, expect) in [(0u8, (1, 0x77)), (4, (2, 1))] {
            let mut uc = Unicorn::new(Arch::ARM64, Mode::LITTLE_ENDIAN).unwrap();
            uc.mem_map(0, 0x10000, Prot::ALL).unwrap();
            uc.mem_write(site, &asm::b(site, tramp).unwrap().to_le_bytes()).unwrap();
            uc.mem_write(tramp, &e.words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<u8>>()).unwrap();
            uc.mem_write(keypage + keypage::NET as u64, &[net]).unwrap();
            uc.reg_write(R::X1, 0x77).unwrap();
            uc.reg_write(R::SP, 0x9000).unwrap();
            uc.emu_start(site, site + 4, 0, 32).unwrap();
            assert_eq!((uc.reg_read(R::X0).unwrap(), uc.reg_read(R::X1).unwrap()), expect, "net {net}");
        }
    }

    #[test]
    fn nextendo_sites_are_online_patches() {
        for &(module_id, site, _) in NEXTENDO_SITES {
            assert!(group_of(module_id, site).is_some_and(|g| g.online), "{site:#x} is not an online patch");
        }
    }

    #[test]
    fn aspect_scale_rejects_other_stores() {
        let mut e = Emitter { base: 0x1000, words: Vec::new(), fixups: Vec::new(), labels: Vec::new() };
        assert!(emit_aspect_scale(&mut e, 0x4000, asm::str_s(1, 9, 0xB0), 0x2000).is_err());
        assert!(emit_aspect_scale(&mut e, 0x4000, asm::str_s(0, 16, 0xB0), 0x2000).is_err());
        assert!(emit_aspect_scale(&mut e, 0x4000, asm::str(0, 9, 0xB0), 0x2000).is_err());
    }
}
