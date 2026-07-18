use parking_lot::RwLock;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use crate::protocol::MemberNumber;
use crate::state::{ChatRoom, OnlineAccount};
use crate::util::common_time;

#[derive(Debug, Clone)]
pub struct AccountCreationRecord {
    pub address: String,
    pub time: i64,
}

#[derive(Debug, Clone)]
pub struct PasswordResetEntry {
    pub account_name: String,
    pub reset_number: String,
}

/// Live members currently rendering the same Underground Prison sector. This is
/// deliberately independent from ChatRoom: the sector remains its own screen.
#[derive(Debug, Clone)]
pub struct PrisonSectorSession {
    pub host_member_number: MemberNumber,
    pub sector_index: usize,
    pub members: HashSet<MemberNumber>,
}

/// Global process state (single instance, in-memory rooms/sessions).
pub struct World {
    pub accounts_by_socket: HashMap<String, OnlineAccount>,
    pub socket_by_member: HashMap<MemberNumber, String>,
    pub socket_by_account_name: HashMap<String, String>,
    pub chat_rooms: HashMap<String, ChatRoom>,
    pub room_by_name_env: HashMap<String, String>, // "ENV:name" -> room id
    pub prison_sector_sessions: HashMap<String, PrisonSectorSession>, // "host:sector" -> live view
    pub ip_connections: HashMap<String, VecDeque<i64>>,
    pub account_creation_ip: Vec<AccountCreationRecord>,
    pub password_resets: Vec<PasswordResetEntry>,
    pub next_member_number: AtomicI64,
    pub next_password_reset_at: AtomicI64,
    pub login_pending: HashSet<String>,
    /// Approximate login queue depth (for LoginQueue event when > 16).
    pub login_queue_len: usize,
}

impl World {
    pub fn new(next_member_number: i64) -> Self {
        Self {
            accounts_by_socket: HashMap::new(),
            socket_by_member: HashMap::new(),
            socket_by_account_name: HashMap::new(),
            chat_rooms: HashMap::new(),
            room_by_name_env: HashMap::new(),
            prison_sector_sessions: HashMap::new(),
            ip_connections: HashMap::new(),
            account_creation_ip: Vec::new(),
            password_resets: Vec::new(),
            next_member_number: AtomicI64::new(next_member_number),
            next_password_reset_at: AtomicI64::new(0),
            login_pending: HashSet::new(),
            login_queue_len: 0,
        }
    }

    pub fn online_count(&self) -> usize {
        self.accounts_by_socket.len()
    }

    pub fn allocate_member_number(&self) -> MemberNumber {
        self.next_member_number.fetch_add(1, Ordering::SeqCst)
    }

    pub fn get_by_socket(&self, socket_id: &str) -> Option<&OnlineAccount> {
        self.accounts_by_socket.get(socket_id)
    }

    pub fn get_by_socket_mut(&mut self, socket_id: &str) -> Option<&mut OnlineAccount> {
        self.accounts_by_socket.get_mut(socket_id)
    }

    pub fn get_by_member(&self, member: MemberNumber) -> Option<&OnlineAccount> {
        self.socket_by_member
            .get(&member)
            .and_then(|sid| self.accounts_by_socket.get(sid))
    }

    pub fn insert_account(&mut self, account: OnlineAccount) {
        let socket_id = account.socket_id.clone();
        let member = account.member_number;
        let name = account.account_name.clone();
        self.socket_by_member.insert(member, socket_id.clone());
        self.socket_by_account_name.insert(name, socket_id.clone());
        self.accounts_by_socket.insert(socket_id, account);
    }

    /// Removes an online account and returns each sector whose membership changed.
    /// Callers emit the sector snapshots only after releasing the world lock.
    pub fn remove_account(
        &mut self,
        socket_id: &str,
    ) -> Option<(OnlineAccount, Vec<(MemberNumber, usize)>)> {
        if let Some(acc) = self.accounts_by_socket.remove(socket_id) {
            self.socket_by_member.remove(&acc.member_number);
            self.socket_by_account_name.remove(&acc.account_name);
            self.login_pending.remove(socket_id);
            let mut changed_sessions = Vec::new();
            for session in self.prison_sector_sessions.values_mut() {
                if session.members.remove(&acc.member_number) {
                    changed_sessions.push((session.host_member_number, session.sector_index));
                }
            }
            self.prison_sector_sessions
                .retain(|_, session| !session.members.is_empty());
            Some((acc, changed_sessions))
        } else {
            self.login_pending.remove(socket_id);
            None
        }
    }

    pub fn find_duplicate_login(&self, account_name: &str) -> Option<String> {
        self.socket_by_account_name.get(account_name).cloned()
    }

    pub fn room_key(env: &str, name: &str) -> String {
        format!("{}:{}", env, name.to_uppercase())
    }

    pub fn get_room_by_name(&self, env: &str, name: &str) -> Option<&ChatRoom> {
        let key = Self::room_key(env, name);
        self.room_by_name_env
            .get(&key)
            .and_then(|id| self.chat_rooms.get(id))
    }

    /// Node: room names are unique globally (case-insensitive), not per environment.
    pub fn room_name_exists_any(&self, name: &str) -> bool {
        let upper = name.to_uppercase();
        self.chat_rooms
            .values()
            .any(|r| r.name.to_uppercase() == upper)
    }

    pub fn insert_room(&mut self, room: ChatRoom) {
        let key = Self::room_key(&room.environment, &room.name);
        self.room_by_name_env.insert(key, room.id.clone());
        self.chat_rooms.insert(room.id.clone(), room);
    }

    pub fn remove_room(&mut self, room_id: &str) {
        if let Some(room) = self.chat_rooms.remove(room_id) {
            let key = Self::room_key(&room.environment, &room.name);
            self.room_by_name_env.remove(&key);
        }
    }

    pub fn remove_member_from_all_rooms(&mut self, member: MemberNumber) {
        let mut empty_rooms = Vec::new();
        for room in self.chat_rooms.values_mut() {
            room.members.retain(|&m| m != member);
            if room.members.is_empty() {
                empty_rooms.push(room.id.clone());
            }
        }
        for id in empty_rooms {
            self.remove_room(&id);
        }
        if let Some(acc) = self.get_by_member(member) {
            let sid = acc.socket_id.clone();
            if let Some(a) = self.accounts_by_socket.get_mut(&sid) {
                a.chat_room_id = None;
            }
        }
    }

    /// Node never prunes AccountCreationIP; total = all-time since process start.
    pub fn count_creations_for_ip(&self, ip: &str) -> (usize, usize) {
        let now = common_time();
        let mut total = 0;
        let mut hour = 0;
        for r in &self.account_creation_ip {
            if r.address == ip {
                total += 1;
                if r.time >= now - 3_600_000 {
                    hour += 1;
                }
            }
        }
        (total, hour)
    }
}

pub type SharedWorld = Arc<RwLock<World>>;

pub fn new_shared_world(next_member: i64) -> SharedWorld {
    Arc::new(RwLock::new(World::new(next_member)))
}
