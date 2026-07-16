# Bondage Club Server (Rust)

Primary implementation of the Bondage Club multiplayer backend.

## Stack

- **tokio** + **axum** + **socketioxide** (Socket.IO 4)
- **mongodb** (async driver)
- **bcrypt** (passwords uppercased before hash/verify — DB compatible)

## Run

```bash
# Requires MongoDB
export DATABASE_URL=mongodb://localhost:27017/BondageClubDatabase
cargo run --release
# listens on :4288
```

Health check: `GET /healthz`

## Docker

From repo root:

```bash
docker compose up -d --build
```

Or build the crate image directly:

```bash
cd rust && docker build -t bc-server-rs .
```

## Feature status

| Area | Status |
|------|--------|
| Account create / login / LoginQueue | Done |
| Password reset + email | Done |
| Account update / query / beep / difficulty | Done |
| Chat rooms search/create/join/leave | Done |
| Chat + game relay | Done |
| Character sync (appearance/pose/…) | Done |
| Room admin (all actions) | Done |
| Ownership (4-step trial→collar) | Done |
| Lovership (6-step dating→wedding) | Done |
| Offline release / NPC break | Done |
| CharacterUpdate (others + AllowItem) | Done |
| ItemUpdate (AllowItem + exclude source) | Done |
| Message rate limit (20/s) | Done |
| OnlineFriends (Sub/Lover/Friend + private) | Done |
| AccountUpdate NPC Lover + room sync | Done |
| AccountBeep (owner/leash/env/secret) | Done |
| Post-login strip pre-auth handlers | Done |
| Room search (Ignore/ShowLocked/Friends/240) | Done |
| Kick/Ban ServerKick/ServerBan order | Done |
| AllowItem Dom+25 vs target | Done |
| BlackList room-filtered + ItemPerm 1/2 | Done |

Account documents stay flexible BSON/JSON for Mongo compatibility with existing client DBs.
