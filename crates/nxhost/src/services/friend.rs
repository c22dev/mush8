// Mush8 - friend.rs
// Constantin Clerc (c22dev) - 0.1
use crate::ipc::{Act, Request, Response, Service, Stub, Table, new_event, write_buffer};
use crate::services::bsd::{bytes, in_buf, out_buf};
use crate::services::nexweb::{self, Friend};
use std::sync::Arc;
use std::time::Duration;

const NOTIFICATION_QUEUE_EMPTY: u32 = (15 << 9) | 121;
const FRIEND_SIZE: usize = 0x200;
const PROFILE_SIZE: usize = 0x100;
const PRESENCE: usize = 0x40;
const APP_FIELD: usize = 0xC0;
const FRIEND_UID_HIGH: u64 = 0x1100_0000_0000_0000;
const STATUS_OFFLINE: u32 = 0;
const STATUS_ONLINE_PLAY: u32 = 2;
const FIRST_LOAD_WAIT: Duration = Duration::from_millis(1500);

pub fn open() -> Arc<dyn Service> {
    Arc::new(Table {
        name: "friend",
        cmds: &[
            (0, Act::Obj(friend_service)),
            (1, Act::Obj(notification_service)),
        ],
    })
}

fn friend_service() -> Arc<dyn Service> {
    Arc::new(FriendService)
}

fn notification_service() -> Arc<dyn Service> {
    Arc::new(Table {
        name: "friend::INotificationService",
        cmds: &[
            (0, Act::Event(false)),
            (1, Act::Ok),
            (2, Act::Err(NOTIFICATION_QUEUE_EMPTY)),
        ],
    })
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Filter {
    online: bool,
    favorite: bool,
    same_app: bool,
}

impl Filter {
    pub fn read(raw: &[u8]) -> Filter {
        let at = |i: usize| raw.get(i).copied().unwrap_or(0);
        let status = u32::from_le_bytes([at(0), at(1), at(2), at(3)]);
        Filter { online: (1..=3).contains(&status), favorite: at(4) != 0, same_app: at(5) != 0 }
    }
    pub fn keeps(&self, f: &Friend) -> bool {
        (!self.online || f.online()) && (!self.favorite || f.favorite) && (!self.same_app || f.in_mk8())
    }
}

pub fn friend_record(id: u64, user: Option<&[u8]>, f: &Friend) -> [u8; FRIEND_SIZE] {
    let mut r = [0u8; FRIEND_SIZE];
    let mut friend_uid = [0u8; 16];
    friend_uid[..8].copy_from_slice(&FRIEND_UID_HIGH.to_le_bytes());
    friend_uid[8..].copy_from_slice(&id.to_le_bytes());
    match user.filter(|u| u.len() >= 16) {
        Some(u) => r[..16].copy_from_slice(&u[..16]),
        None => r[..16].copy_from_slice(&friend_uid),
    }
    r[0x10..0x18].copy_from_slice(&id.to_le_bytes());
    let name = truncated(&f.name, 32);
    r[0x18..0x18 + name.len()].copy_from_slice(name);
    r[PRESENCE..PRESENCE + 16].copy_from_slice(&friend_uid);
    r[PRESENCE + 0x10..PRESENCE + 0x18].copy_from_slice(&i64::MAX.to_le_bytes());
    let status = if f.online() { STATUS_ONLINE_PLAY } else { STATUS_OFFLINE };
    r[PRESENCE + 0x18..PRESENCE + 0x1C].copy_from_slice(&status.to_le_bytes());
    r[PRESENCE + 0x1C] = f.in_mk8() as u8;
    if f.in_mk8() {
        let n = f.app_field.len().min(APP_FIELD);
        r[PRESENCE + 0x20..PRESENCE + 0x20 + n].copy_from_slice(&f.app_field[..n]);
    }
    r[0x120] = f.favorite as u8;
    r[0x128] = 1;
    r
}

pub fn profile_record(id: u64, f: &Friend) -> [u8; PROFILE_SIZE] {
    let mut r = [0u8; PROFILE_SIZE];
    r[..8].copy_from_slice(&id.to_le_bytes());
    let name = truncated(&f.name, 32);
    r[8..8 + name.len()].copy_from_slice(name);
    let url = format!("https://cdn-image-e0d67c509fb203858ebcb2fe3f88c2aa.baas.nintendo.com/1/pid_{0}?pid={0}", f.pid);
    let url = &url.as_bytes()[..url.len().min(0x9F)];
    r[0x30..0x30 + url.len()].copy_from_slice(url);
    r[0xD0] = 1;
    r
}

fn truncated(s: &str, max: usize) -> &[u8] {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s.as_bytes()[..end]
}

fn ids(req: &Request) -> Vec<u64> {
    bytes(in_buf(req, 0)).chunks_exact(8).map(|c| u64::from_le_bytes(c.try_into().unwrap())).collect()
}

fn friends() -> Vec<Friend> {
    crate::kernel::blocking(|| nexweb::friends(FIRST_LOAD_WAIT))
}

fn listed(req: &Request) -> Vec<Friend> {
    let offset = (req.u32(0) as i32).max(0) as usize;
    let filter = Filter::read(req.data.get(0x18..0x28).unwrap_or(&[]));
    friends().into_iter().filter(|f| filter.keeps(f)).skip(offset).collect()
}

fn write_records<const N: usize>(req: &Request, records: impl Iterator<Item = [u8; N]>) -> usize {
    let Some(b) = out_buf(req, 0) else { return 0 };
    let room = b.size as usize / N;
    let out: Vec<u8> = records.take(room).flatten().collect();
    write_buffer(&b, &out);
    out.len() / N
}

pub struct FriendService;

impl Service for FriendService {
    fn name(&self) -> String {
        "friend::IFriendService".into()
    }
    fn call(&self, req: &Request, rsp: &mut Response) -> u32 {
        match req.cmd {
            0 => rsp.copy_handles.push(new_event(false)),
            1 | 10121 | 10421 | 10600 | 10601 => {}
            10100 => {
                let list = listed(req);
                let n = out_buf(req, 0).map_or(0, |b| {
                    let out: Vec<u8> = list.iter().take(b.size as usize / 8).flat_map(|f| f.pid.to_le_bytes()).collect();
                    write_buffer(&b, &out) / 8
                });
                rsp.push_u32(n as u32);
            }
            10101 => {
                let list = listed(req);
                let n = write_records(req, list.iter().map(|f| friend_record(f.pid, None, f)));
                rsp.push_u32(n as u32);
            }
            10102 => {
                let known = friends();
                let user = req.data.get(..16);
                let missing = Friend::default();
                write_records(req, ids(req).into_iter().map(|id| match known.iter().find(|f| f.pid == id) {
                    Some(f) => friend_record(id, user, f),
                    None => {
                        let mut r = friend_record(id, user, &missing);
                        r[0x128] = 0;
                        r
                    }
                }));
            }
            10110 => rsp.push_u32(0),
            10120 | 10420 => rsp.data.push(1),
            10400 => rsp.push_u32(0),
            10500 => {
                let known = friends();
                write_records(req, ids(req).into_iter().map(|id| match known.iter().find(|f| f.pid == id) {
                    Some(f) => profile_record(id, f),
                    None => [0u8; PROFILE_SIZE],
                }));
            }
            10610 => {
                let p = bytes(in_buf(req, 0));
                if p.len() >= 0x20 {
                    nexweb::set_presence(&p[0x20..p.len().min(0x20 + APP_FIELD)]);
                }
            }
            _ => return Stub(self.name()).call(req, rsp),
        }
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn friend(status: u32, app_id: u64) -> Friend {
        Friend { pid: 0x1234_5678, name: "Toadette".into(), status, app_id, app_field: vec![9; 0x100], favorite: false }
    }

    #[test]
    fn friend_record_matches_the_sdk_layout() {
        let r = friend_record(0x1234_5678, None, &friend(1, nexweb::MK8));
        assert_eq!(&r[..8], &FRIEND_UID_HIGH.to_le_bytes());
        assert_eq!(&r[8..16], &0x1234_5678u64.to_le_bytes());
        assert_eq!(&r[0x10..0x18], &0x1234_5678u64.to_le_bytes());
        assert_eq!(&r[0x18..0x21], b"Toadette\0");
        assert_eq!(&r[0x40..0x50], &r[..16]);
        assert_eq!(r[0x58], 2);
        assert_eq!(r[0x5C], 1);
        assert!(r[0x60..0x120].iter().all(|&b| b == 9));
        assert_eq!(r[0x120], 0);
        assert_eq!(r[0x128], 1);
    }

    #[test]
    fn friend_record_takes_the_asking_user() {
        let user = [7u8; 16];
        let r = friend_record(5, Some(&user), &friend(0, 0));
        assert_eq!(&r[..16], &user);
        assert_eq!(&r[0x40..0x48], &FRIEND_UID_HIGH.to_le_bytes());
        assert_eq!(&r[0x48..0x50], &5u64.to_le_bytes());
        assert_eq!(r[0x58], 0);
        assert_eq!(r[0x5C], 0);
        assert!(r[0x60..0x120].iter().all(|&b| b == 0));
    }

    #[test]
    fn filter_reads_sized_friend_filter() {
        let mut raw = [0u8; 16];
        assert_eq!(Filter::read(&raw), Filter::default());
        raw[0] = 3;
        raw[5] = 1;
        let f = Filter::read(&raw);
        assert!(f.keeps(&friend(2, nexweb::MK8)));
        assert!(!f.keeps(&friend(2, 0x0100_F8F0_000A_2000)));
        assert!(!f.keeps(&friend(0, nexweb::MK8)));
    }

    #[test]
    fn profile_record_has_name_and_picture() {
        let r = profile_record(77, &Friend { pid: 77, name: "Daisy".into(), ..Default::default() });
        assert_eq!(&r[..8], &77u64.to_le_bytes());
        assert_eq!(&r[8..13], b"Daisy");
        assert!(r[0x30..0xD0].starts_with(b"https://cdn-image-"));
        assert_eq!(r[0xD0], 1);
    }

    #[test]
    fn names_are_cut_on_a_char_boundary() {
        assert_eq!(truncated(&"é".repeat(20), 32).len(), 32);
        assert_eq!(truncated(&format!("a{}", "é".repeat(20)), 32).len(), 31);
    }
}
