# dc.enderr.win

Web panel where clients manage the Discord bots assigned to them, logging in
with their Discord account — and the bot itself. 100 % Rust monorepo.

| Path | Role |
| --- | --- |
| `crates/core` (`dc-core`) | Bot registry (`bots/*/bot.toml`), per-server configuration, image rendering, panel-to-bot requests, SQLite helpers. Shared by the panel and the bot. |
| `crates/panel` (`dc-panel`) | The panel: Axum, server-rendered HTML with Maud, no JavaScript (except the verification Activity page), Docker control via `bollard`. |
| `crates/bot` (`dc-bot`) | The bot run in each container. Every message uses Discord's Components V2, with image banners and cards. |
| `bots/<slug>/` | One folder per bot, created by the admin. |

## How it works

- **The admin creates the bots.** Each bot is a folder in `bots/` and runs in
  its own Docker container, all from the same `dc-bot` image:

  ```text
  bots/<slug>/bot.toml       name, description, owners              (committed)
  bots/<slug>/compose.yaml   the bot's container                     (committed)
  bots/<slug>/.env           DISCORD_TOKEN=..., DISCORD_CLIENT_SECRET (secret, git-ignored)
  bots/<slug>/settings.json  presence, written by the panel          (git-ignored)
  bots/<slug>/guilds/        one JSON per server, written by the     (git-ignored)
                             panel and by the bot's /settings
  bots/<slug>/data/bot.db    the bot's database: XP, tickets, panels (git-ignored)
  bots/<slug>/requests/      one-off requests from the panel to the  (git-ignored)
                             running bot: tests, verifications
  ```

- **Assignment** is done by Discord user id in `owners` of `bot.toml`. Users
  only see their bots; other bots return 404. Ids in `ADMIN_IDS` (panel `.env`)
  see and manage every bot. `bot.toml` changes need no panel restart.
- **From the panel**, owners start / stop / restart the container, read its
  logs, set the bot's name, picture and presence, and configure each server
  the bot is in. The bot notices saved changes within a few seconds — no restart.
- Each server section shows a **preview** of what the bot posts (with the real
  images) and a **test button** that makes the running bot send it. The
  **Danger zone** resets a server to its defaults, optionally erasing its XP.
- The panel only touches containers labelled `win.enderr.dc.bot=<slug>`.

## The bot

| Feature | Commands | Dashboard section |
| --- | --- | --- |
| Lock / hide channels | `/lock` `/unlock` `/hide` `/unhide` | — |
| Link lock | `/lockurl` | Link lock: allowed websites, exempt roles and channels |
| Bot admins | `/ownerbot add\|remove\|list` | Admins: members and roles |
| Sanctions | `/mute` `/unmute` `/kick` `/ban` (with confirmation) `/unban` | — |
| Roles | `/role add\|remove` | Welcome: roles given on arrival |
| Levels | `/rank` (image card) `/leaderboard` (image) `/xp add\|remove\|reset` | Levels: XP rates, announcements, role rewards |
| Welcome | `/settings welcome`, `/settings preview` | Welcome: channel, message, image |
| Rules + verification | `/settings rules` | Rules: text, button / captcha image / Activity, verified role, role removed once verified |
| Tickets | `/settings tickets` | Tickets: panel, category, staff, topics, transcripts |
| Logs | `/settings logs` | Logs: deleted / edited messages, channels, members, moderation |
| Settings panel | `/settings view`: image of the configuration, a menu per module, buttons and pickers to change it; `/help` | Everything |

**Who is an admin?** Moderation and settings commands are reserved to: the
server owner, members with Discord's Administrator permission, the bot's
`owners` from `bot.toml`, and the members and roles added in the dashboard's
*Admins* section or with `/ownerbot`. Only the server owner and the bot's
owners can change that list from Discord.

The rules panel and the ticket panel are posted and kept up to date by the bot
itself: change a text or a channel on the dashboard and the message follows.

### Discord application setup

On <https://discord.com/developers/applications>, for the bot's application:

1. **Bot → Privileged Gateway Intents**: enable *Server Members Intent* and
   *Message Content Intent*. Without them the bot still starts, but welcome
   messages, XP, link lock and message logs are unavailable (it says so in its logs).
2. Invite it with the link on the panel's Overview tab, then give its role the
   permissions it needs: Manage Channels, Manage Roles, Kick, Ban, Moderate
   Members, Manage Messages. Put its role **above** the roles it must give.

Starting the bot registers its slash commands globally, replacing any commands
previously registered for that application (the Activity's "Launch" entry
point is kept).

### Verification in a Discord Activity (optional)

With the *Activity* verification mode, the Verify button opens a small window
inside Discord where the member solves the captcha. It needs a one-time setup
per bot; until it is done, members get the image captcha instead.

1. Developer Portal → the bot's application → **Activities**: enable
   Activities, then under **URL Mappings** map the root `/` to
   `dc.enderr.win/activity/<slug>` (the panel must be reachable over HTTPS).
2. **OAuth2**: add any redirect URI (for example `https://dc.enderr.win`) — it is
   never opened, but Discord requires one — and copy the **Client Secret** into
   `bots/<slug>/.env` as `DISCORD_CLIENT_SECRET=...`.
3. Restart the bot, then choose *Activity* in the dashboard's Rules section.

The page identifies the member with Discord's OAuth2 (`identify` scope) through
the Activity itself, shows the captcha, and the bot then gives the roles.

## Adding a bot

```sh
cp -r bots/_template bots/music
cp bots/music/.env.example bots/music/.env   # put the bot token in it
# edit bots/music/bot.toml (name, owners); replace "my-bot" with "music" in compose.yaml
docker compose -f bots/music/compose.yaml up -d --build
```

## Local development

1. Create an application on <https://discord.com/developers/applications> and
   add the redirect `http://localhost:8080/auth/callback` under **OAuth2**.
2. `cp .env.example .env` and fill in `DISCORD_CLIENT_ID`,
   `DISCORD_CLIENT_SECRET` and your own id in `ADMIN_IDS`.
3. `cargo run -p dc-panel`, then open <http://localhost:8080>.
4. Run a bot without Docker: `cargo run -p dc-bot -- bots/sang`
   (the token is read from `bots/sang/.env`).

Docker Desktop is optional locally: without it, bots show as "Unknown" on the
panel and the power buttons are disabled, but configuration works.

## Production (dc.enderr.win)

```sh
# in .env: PUBLIC_URL=https://dc.enderr.win and DOCKER_GID=$(stat -c %g /var/run/docker.sock)
sudo chown -R 10001 bots          # panel and bots run as uid 10001 and write here
docker compose up -d --build                             # the panel
docker compose -f bots/sang/compose.yaml up -d --build   # each bot
```

- Register `https://dc.enderr.win/auth/callback` as redirect on Discord.
- The panel listens on `127.0.0.1:8080` over HTTP: put a TLS reverse proxy in
  front (Caddy, nginx, Cloudflare Tunnel…). With an `https://` `PUBLIC_URL`,
  cookies are `Secure` and HSTS is enabled.
- Access to the Docker socket is equivalent to root on the host. The panel
  limits itself to labelled bot containers, but keep it updated and behind HTTPS.
- Back up `bots/*/guilds/`, `bots/*/data/` and the panel's `/data` volume.

## Deploying on Coolify

`compose.coolify.yaml` runs the panel and the bots as one Docker Compose
resource. Secrets are Coolify environment variables; the panel and the bots
share a `bots` volume; each `bot.toml` comes with the image and is copied
into that volume at every start.

1. **New resource** → your Git repository → build pack **Docker Compose**,
   compose file `/compose.coolify.yaml`.
2. **Environment variables** (Coolify lists them after reading the file):

   | Variable | Value |
   | --- | --- |
   | `DISCORD_CLIENT_ID`, `DISCORD_CLIENT_SECRET` | the Discord application used to log in to the panel |
   | `PUBLIC_URL` | `https://dc.enderr.win` |
   | `ADMIN_IDS` | staff ids that manage every bot |
   | `DOCKER_GID` | output of `stat -c %g /var/run/docker.sock` on the server |
   | `DISCORD_TOKEN_SANG` | the Sang bot token (secret) |
   | `DISCORD_CLIENT_SECRET_SANG` | optional, for the Activity verification |
   | `OWNERS_SANG` | optional, extra owner ids, comma-separated |

3. **Domain**: on the `panel` service, `https://dc.enderr.win:8080` (8080 is
   the container port; Coolify's proxy serves HTTPS on 443). No domain for the bots.
4. On the Discord application of the login, add the redirect
   `https://dc.enderr.win/auth/callback` (OAuth2 → Redirects).
5. **Deploy**. The first build compiles the whole workspace in release mode:
   count a few minutes and about 2 GB of RAM on the server.

**Giving a client access to a bot**: add their Discord id to `owners` in
`bots/<slug>/bot.toml` and push (Coolify redeploys), or put it in
`OWNERS_<SLUG>` and restart the resource. They log in on the panel with
Discord and see only their bots.

**Adding a bot**: create `bots/<slug>/bot.toml`, copy the `bot-sang` service
of `compose.coolify.yaml` with the new slug, and add its
`DISCORD_TOKEN_<SLUG>` to both the panel and the new service.

The panel finds each bot's container by its `win.enderr.dc.bot` label, so the
names Coolify gives to containers do not matter.

## Security

- Discord OAuth2 login (`identify` scope only), `state` parameter checked.
- Sessions: only the SHA-256 of the token is stored; `HttpOnly`, `SameSite=Lax` cookie.
- CSRF token on every form.
- Every bot and server request is checked against `owners` / `ADMIN_IDS`; a
  server page also requires the bot to be on that server (404 otherwise).
- Everything saved from the dashboard is validated and bounded before it is written.
- Bot tokens stay in each bot's `.env` (or in `DISCORD_TOKEN_<SLUG>` variables) and are never displayed or baked into images.
- Strict CSP (`default-src 'none'`), no inline script or style. The only script
  is the one of the verification Activity (`/activity/<slug>`), which is public,
  holds no panel session, and can only result in "this Discord account passed
  the captcha of this server".
- A bot's name and picture can be changed by its owners; uploads are limited to
  2 MB images and sent straight to Discord, never stored.
- The bot never pings `@everyone` or roles from user-provided text.

## Development

```sh
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

Fonts: the bot's images use [Inter](https://github.com/rsms/inter) (SIL Open
Font License, see `crates/bot/assets/fonts/LICENSE-Inter.txt`).
