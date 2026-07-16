# Bondage Club Server (Rust)

Primary implementation of the Bondage Club multiplayer backend.

## Stack

- **tokio** + **axum** + **socketioxide** (Socket.IO 4)
- **MongoDB** (default) or **SQLite** (optional via `.env`)
- **bcrypt** (passwords uppercased before hash/verify — DB compatible)

## Run

```bash
# Default: MongoDB
export DATABASE_URL=mongodb://localhost:27017/BondageClubDatabase
cargo run --release
# listens on :4288
```

### SQLite (no MongoDB)

```bash
# Option A: scheme in DATABASE_URL
export DATABASE_URL=sqlite:./data/bondage.db

# Option B: explicit backend
export DB_BACKEND=sqlite
export DATABASE_URL=./data/bondage.db

cargo run --release
```

Copy `.env.example` to `.env` and edit as needed.

Health check: `GET /healthz`

## Database config

| Variable | Default | Notes |
|----------|---------|-------|
| `DB_BACKEND` | (auto) | `mongodb` or `sqlite`; if unset, inferred from `DATABASE_URL` |
| `DATABASE_URL` | `mongodb://localhost:27017/BondageClubDatabase` | Mongo URI, or `sqlite:./path.db` / bare `.db` path |
| `DATABASE_NAME` | `BondageClubDatabase` | Mongo only |
| `ACCOUNT_COLLECTION` | `Accounts` | Mongo only |

SQLite stores accounts as key columns + a full JSON document blob for client-compatible flexible fields.

## Docker

From repo root:

```bash
docker compose up -d --build
```

Docker Compose still starts MongoDB by default. For SQLite-only, set `DB_BACKEND=sqlite` and `DATABASE_URL` in `.env` and you can skip the `db` service.

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
| Optional SQLite backend | Done |

Account documents stay flexible JSON for compatibility with existing client DBs.
