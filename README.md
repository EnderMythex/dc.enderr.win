# dc.enderr.win

Web panel where clients manage the Discord bots assigned to them, logging in
with their Discord account — and the bot itself. 100 % Rust monorepo.

| Path | Role |
| --- | --- |
| `crates/core` (`dc-core`) | Bot registry (`bots/*/bot.toml`), per-server configuration, image rendering, panel-to-bot requests, SQLite helpers. Shared by the panel and the bot. |
| `crates/panel` (`dc-panel`) | The panel: Axum, server-rendered HTML with Maud (works without JavaScript; a small script adds the unsaved-changes bar and reconnection screen), Docker control via `bollard`. |
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
  bots/<slug>/background     picture behind the cards, from the panel (git-ignored)
  bots/<slug>/guilds/        one JSON per server, written by the     (git-ignored)
                             panel and by the bot's /settings
  bots/<slug>/data/bot.db    the bot's database: XP, tickets, panels (git-ignored)
  bots/<slug>/requests/      one-off requests from the panel to the  (git-ignored)
                             running bot: tests, verifications
  ```

- **Assignment** is done by Discord user id in `owners` of `bot.toml`. Users
  only see their bots; other bots return 404. Ids in `ADMIN_IDS` (panel `.env`)
  see and manage every bot. `bot.toml` changes need no panel restart.
- The **Commands** tab lists every slash command of the bot (its packs
  included), split between those for everyone and those for bot admins.
- **Card background** (Settings tab): a picture drawn behind every image the
  bot sends (notices, welcome, rank, leaderboard, settings panel), darkened
  by a chosen percentage, stored as `bots/<slug>/background`. The bot picks
  it up within a few seconds; the server previews use it too.
- **From the panel**, owners start / stop / restart the container, read its
  logs (and, under them, the panel activity: who changed the profile, the
  presence, a server section and what changed, tests, backups…), set the bot's name, bio, picture, banner and presence, and configure each server
  the bot is in. The bot notices saved changes within a few seconds — no restart.
- Before saving, the panel asks for a confirmation when a role pick looks
  like a typo: a staff role (moderation permissions, or a name like
  "Admin" / "Modo") given to members on arrival, as verified role, level
  reward or role-panel button; or a member role (the arrival or verified
  role, a name like "Membre") chosen as admin, ticket staff or link-exempt role.
- Each server section shows a **preview** of what the bot posts (with the real
  images) and a **test button** that makes the running bot send it. The
  **Danger zone** resets a server to its defaults (optionally erasing its XP)
  or removes the bot from the server.
- **Backup** exports a server's settings as a JSON file and imports one back
  (on the same server, another server or another bot). Only the sections in the
  file are replaced, so a file can carry a single section.
- The panel only touches containers labelled `win.enderr.dc.bot=<slug>`.

## The bot

| Feature | Commands | Dashboard section |
| --- | --- | --- |
| Lock / hide channels | `/lock` `/unlock` `/hide` `/unhide` | — |
| Link lock | `/lockurl` | Link lock: allowed websites, exempt roles and channels |
| Bot admins | `/ownerbot add\|remove\|list` | Admins: members and roles |
| Sanctions | `/mute` `/unmute` `/kick` `/ban` (with confirmation) `/unban` | — |
| Roles | `/role add\|remove`, `/role add-all\|remove-all` (every member, with confirmation) | Welcome: roles given on arrival |
| Levels | `/rank` (image card) `/leaderboard` (image) `/xp add\|remove\|reset` | Levels: XP rates, announcements, role rewards |
| Welcome | `/settings welcome`, `/settings preview` | Welcome: channel, message, image; mentions of new members in chosen channels, deleted after a few seconds |
| Rules | `/settings rules` | Rules: channel, title, text (can be turned off to keep only the verification) |
| Verification | `/settings rules` | Verification: Verify button under the rules or in a message of its own in another channel; mode (button, colors, captcha image, website link, Activity), captcha style (text or slide), verified role, role removed once verified |
| Tickets | `/settings tickets` | Tickets: panel, category, staff, topics, transcripts |
| Logs | `/settings logs` | Logs: deleted / edited messages, channels, members, moderation |
| Announcements | `/announce` (form with title and text, picture, @everyone / @here / role) · `/maintenance start\|end` | — |
| Giveaways | `/giveaway start\|end\|reroll\|cancel\|list`: Enter button with live count, optional required role, automatic draw, rerolls without previous winners | — |
| Anti-scam | — | Anti-scam: Discord invites (even disguised), the "insider" pitch, scam pictures checked by AI (Gemini), staff channel with Unban |
| Link reviews | — | Link lock: watched channels, x.com blocked per channel, timeout, the member can send the link to the staff who accepts or refuses it |
| Role panel | — | Role panel: up to 10 buttons to take or drop a role |
| AI assistant | mention the bot | AI assistant: knowledge, channels, length, cooldown (needs `GEMINI_API_KEY_<SLUG>`) |
| Twitch alerts | — | Twitch: streamers, channel, role (needs `TWITCH_CLIENT_ID_<SLUG>` and `TWITCH_CLIENT_SECRET_<SLUG>`) |
| Safety reminders | — | Safety reminders: French and English channels, interval |
| Members | `/stats` `/missing-role` | — |
| Release announcements | `/release` `/download` (misty pack) | Releases: repository, channel, role, pre-releases, download and website links |
| GitHub activity | — | GitHub activity: repositories, channel, commits / pull requests / issues / stars / forks / tags |
| Status panel | `/status` (misty pack) | Status panel: websites (up, slow, down, with history), version, downloads, players, stars |
| Minecraft server | `/smp` (misty pack) | Minecraft server: address (SRV records followed), name, description, players, panel channel |
| Settings panel | `/settings view`: image of the configuration, a menu per module, buttons and pickers to change it; `/help` | Everything |

**Who is an admin?** Moderation and settings commands are reserved to: the
server owner, members with Discord's Administrator permission, the bot's
`owners` from `bot.toml`, and the members and roles added in the dashboard's
*Admins* section or with `/ownerbot`. Only the server owner and the bot's
owners can change that list from Discord.

The rules, verification, role and ticket panels are posted and kept up to date by the bot
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

### Verification on the website

With the *Website* verification mode, the Verify button gives each member a
personal link to `/verify/<slug>` on the panel, signed with the bot's token
for that member and server and valid for 10 minutes. The page shows the
captcha chosen in the Verification section: **Text** (type the code of an image) or
**Slide** (slide a puzzle piece into its hole). Nothing to set up besides the
bot's `DASHBOARD_URL`, which tells it the panel's address.

### Verification in a Discord Activity (optional)

With the *Activity* verification mode, the Verify button opens a small window
inside Discord where the member solves the captcha. It needs a one-time setup
per bot; until it is done, members get the image captcha instead.

1. Developer Portal → the bot's application → **Activities**: enable
   Activities, then under **URL Mappings** map the root `/` to
   `dc.enderr.win/activity` (the panel must be reachable over HTTPS). The
   mapping is the same for every bot: the page recognizes the bot from the
   application id in its address. The former per-bot mapping
   (`dc.enderr.win/activity/<slug>`) still works.
2. **OAuth2**: add any redirect URI (for example `https://dc.enderr.win`) — it is
   never opened, but Discord requires one — and copy the **Client Secret** into
   `bots/<slug>/.env` as `DISCORD_CLIENT_SECRET=...`.
3. Restart the bot, then choose *Activity* in the dashboard's Verification section.

The page identifies the member with Discord's OAuth2 (`identify` scope) through
the Activity itself, shows the captcha, and the bot then gives the roles.

### Packs

A bot can ship extra content in `bots/<slug>/assets/`, enabled with
`packs = [...]` in its `bot.toml`. The `scanline` pack (bot Scanline) adds
`/filters`, `/tools`, `/tuto-terminal` and `/wallet-tracker`, and the AI
assistant's default knowledge (`assistant.md`); see
`bots/scanline/assets/README.md`. The presence can also show the SOL price
(Settings → Activity → SOL price).

The former Scanline bot's settings (channels, roles, welcome DM, link lock,
role panel, Twitch, reminders) are in `bots/scanline/import/scanline-settings.json`:
import it in the server's *Backup* section. Panels posted by the former bot
(Verify and role buttons) keep working.

The `misty` pack (bot Misty Bot) adds `/status`, `/release`, `/download` and
`/smp`. Releases, GitHub activity, the status panel and the Minecraft server
are sections every bot has; GitHub needs no token, but `GITHUB_TOKEN_<SLUG>`
(a token without any permission) lets the bot check every minute instead of
every 10 to 15. The former Misty bot's `data/state.json` can be imported as is
in a server's *Backup* section: its sections are converted, with Misty's
repository, website and status pages filled in.

A bot folder may hold a `background.png`: it becomes the picture behind the
bot's cards until another one is chosen in Settings (Misty Bot ships the night
landscape of the Misty website).

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
   | `DISCORD_TOKEN_ELYZIUM`, `DISCORD_CLIENT_SECRET_ELYZIUM`, `OWNERS_ELYZIUM` | the same for ElyZium Bot |
   | `LEGAL_NAME`, `LEGAL_ADDRESS`, `LEGAL_EMAIL` | publisher shown on the legal pages (`/legal`, `/privacy`, `/terms`) |

3. **Domain**: on the `panel` service, `https://dc.enderr.win:8080` (8080 is
   the container port; Coolify's proxy serves HTTPS on 443). No domain for the bots.
4. On the Discord application of the login, add the redirect
   `https://dc.enderr.win/auth/callback` (OAuth2 → Redirects).
5. **Deploy**. The first build compiles the whole workspace in release mode:
   count about 3 minutes and 2 GB of RAM on the server. Later deploys reuse
   Docker's build cache (about 1.1 GB, capped at 2.5 GB) and only compile the
   code that changed: a few seconds of compilation. A `docker builder prune`
   (Coolify's disk cleanup can run one) empties that cache; the next build
   then takes the 3 minutes again.

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

- **Rate limits (panel)**, per IP: forms, login and image previews 20 at once
  then one every 3 s; the verification page 300 at once then 5 per second
  (Discord's proxy may put every Activity user behind one address); other pages
  120 then 2 per second. Over the limit: `429` with `Retry-After`. Behind a
  proxy the client is the last `X-Forwarded-For` entry; set
  `TRUSTED_PROXY_HOPS=2` if Cloudflare (or another proxy) sits in front of
  Coolify's Traefik.
- **Rate limits (bots)**: 8 commands, clicks or forms per user every 10
  seconds; XP has its own cooldown, captchas 5 attempts.
- **Bot admins cannot exceed their Discord rights**: unless they are Discord
  administrators, the server owner or a bot owner, they can only give or take
  roles below their highest role and without moderation permissions (`/role`,
  automatic roles, verified role), only sanction members below them, and need
  Discord's "Mention @everyone" permission to mention everyone. Moderation
  roles are never given to every member at once.
- No SQL is built from user input (every value is a bound parameter), HTML is
  escaped by the templates, and the scripts never write HTML.
- Discord OAuth2 login (`identify` scope only), `state` parameter checked.
- Sessions: only the SHA-256 of the token is stored; `HttpOnly`, `SameSite=Lax` cookie.
- CSRF token on every form.
- Every bot and server request is checked against `owners` / `ADMIN_IDS`; a
  server page also requires the bot to be on that server (404 otherwise).
- Everything saved from the dashboard is validated and bounded before it is written.
- Bot tokens stay in each bot's `.env` (or in `DISCORD_TOKEN_<SLUG>` variables) and are never displayed or baked into images.
- Strict CSP (`default-src 'none'`), scripts from the panel only, no inline
  script or style. Every page works without JavaScript; `assets/app.js` adds the
  unsaved-changes bar and the "Recovering connection" screen, and a service
  worker (`/sw.js`) shows that screen instead of the proxy's error page while
  the panel restarts. Answers from the panel carry an `x-dc-panel` header so
  the two can be told apart.
- The verification Activity (`/activity/<slug>`) is public, holds no panel
  session, and can only result in "this Discord account passed the captcha of
  this server".
- A bot's name, bio, picture and banner can be changed by its owners; uploads are limited to
  images (2 MB, banner 4 MB) and sent straight to Discord, never stored.
- The bot never pings `@everyone` or roles from user-provided text.

## Development

```sh
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test
```

Fonts: the bot's images use [Inter](https://github.com/rsms/inter) (SIL Open
Font License, see `crates/bot/assets/fonts/LICENSE-Inter.txt`).
