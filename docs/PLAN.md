# genco plan

genco is a hosting panel for a single Debian server, like Plesk, with a
maintenance agent at its core: an LLM with root access that keeps the
server alive, sets up domains and does what else is needed. The operator
talks to it in a chat window next to the panel or with `genco chat` in a
terminal.

## Model

- **Client**: who a group belongs to (name, contact). No login.
- **Group**: domains that share files, one Linux user, one systemd slice
  (CPU, RAM limits) and its databases. Usually one project, e.g. a Craft
  site with its crelte frontend.
- **Domain**: a hostname of a group with what serves it: static files,
  PHP (8.2, 8.3, 8.4, 8.5), Node or a Rust binary.
- **Database**: a MariaDB database of a group.
- **User**: an admin of the panel. All users are admins, 2FA later.

## Host layout

- Groups live in `/srv/genco/<group>/` owned by the user `g-<group>`,
  nginx reads them as a member of each group, other groups can't enter.
- nginx is the only public service. PHP runs in one `php-fpm` instance per
  group and version (socket in `/run/genco/<group>/`), Node and Rust in a
  systemd unit per app, all as the group's user, sandboxed, in the
  group's slice. Apps listen on 127.0.0.1:20000+id (most apps can't use
  a socket); an nftables rule on output lets only nginx, root and the
  group connect.
- MariaDB only on its socket, one user per group with `unix_socket` auth.
- genco writes all config from its database (`genco apply`, idempotent).
  The config view shows the generated files and warns about manual edits.
  Special cases go into a custom snippet per domain.
- No SFTP/SSH per group by default.

## Deploys

- A CI job calls `POST /api/deploy/<group>` with the group's deploy token
  (GitHub/GitLab alike, no webhook). genco fetches with a read-only
  deploy key, builds in `releases/<stamp>/` with `shared/` symlinked
  (`.env`, uploads, storage), then switches PHP, runs `craft up`, switches
  Node and restarts it. A failed build leaves the live site alone; the
  last 5 releases stay for a rollback.
- Steps come from presets (craft, crelte/node, static, rust download) or a
  `genco.toml` in the repo. Builds run one at a time, as the group's user.

## Safety

- Deleting a domain, database or group creates a pending action (Start →
  Approvals) and a mail to the admins; it only runs after an admin
  confirms it while logged in, by the mail's link or with the 2FA code.
  It dumps and moves to a trash first (30 days).
- The agent has a root shell, so the real safety net are backups the box
  can't delete.

## Backups

- restic, daily: one snapshot per group (files + `mariadb-dump`) and one of
  the system (`/etc`, genco's database). Local repository first, then AWS
  S3 with versioning and a key without delete rights. Restores are tested.

## Install

- `curl -fsSL https://…/install.sh | bash` downloads the release binary
  (webui embedded) and runs `genco setup`: it first makes an inventory of
  the server (OS, users, open ports, ssh, running services, updates) and
  shows it, then hardens it (ssh keys only, nftables, security updates)
  and installs nginx, MariaDB, PHP, Node, restic.

## Milestones

### 1. Skeleton
- [x] axum server, SQLite, admins with sessions, SSE events
- [x] clients, groups, domains, databases (data only), services, config
      files and database sizes read from the system
- [x] maintenance agent: Opus 5.5 via Anthropic, root shell and file tools,
      conversations that survive a restart
- [x] `genco chat` in the terminal, works without the server
- [x] Win95 webui: desktop, taskbar, explorer, agent window
- [x] running on a test VPS (`scripts/install.sh`)

### 2. Host layer
- [x] server inventory and hardening in `genco setup`, which also installs
      nginx, MariaDB, PHP 8.2–8.5 (sury), Node 24, composer, certbot
- [x] `genco apply`: users, directories, nginx, php-fpm, units, slices,
      app port firewall, MariaDB users and databases, certificates
- [x] services and config view with drift warnings, Apply in the panel
- [x] test sites: PHP + DB, Node, Rust, static on subdomains of the test VPS;
      break-out tests between two groups (files, ports, processes, config)

### 3. Deploys
- [x] a deploy key per group, API tokens with scopes (`deploy:<group>`)
      for CI, `POST /api/deploy`
- [x] releases with shared paths, build steps as the group's user, the
      switch, after-switch steps, app restarts, the last 5 releases kept
- [x] `genco.toml` or detected steps (composer, Craft, npm, one level deep)
- [x] Deploy tab: repository, key, deploys with live log, rollback, tokens
      with a GitLab CI example; `genco deploy` in the terminal
- [x] tested with a private GitHub repo (Craft + Node): deploy by token,
      failed build leaves the site alone, rollback

### 4. Backups
- [x] restic, local repository (/var/backups/genco/restic), nightly timer:
      per group its files and a dump per database, the system (/etc,
      genco's database); kept the last 3, 7 daily, 4 weekly, 6 monthly per kind
- [x] backup runs with log on the server page, snapshots per group,
      `genco backup`, `genco restic ...` for restores
- [x] restore test: files, a database into a new one, genco's database
- [x] offsite: a remote copy to Backblaze B2 or S3-compatible storage
      (`restic copy` after each full backup, own retention, Settings →
      Backups and `genco backup remote`), restores with `--remote`
- [ ] offsite: a key that can't delete (bucket versioning instead of
      restic's prune on the server)

### 5. Safety and polish
- [x] postfix on localhost for genco and the sites (PHP mail() via
      msmtp), direct or through a relay set in Settings, test mail
- [x] deleting a domain, database or group mails every admin a link, it
      runs when one confirms it logged in; dump or archive to the trash
      (30 days); `genco ... delete` for the agent
- [x] TOTP 2FA per admin
- [x] onboarding: on the first start the agent looks at the server and
      writes a checklist with pointers (mail and DNS, backup password, 2FA)
- [x] install.sh asks for what it needs and downloads the release, a
      GitHub workflow builds it on a tag (the repository is private yet)
- [x] break-out test between groups (milestone 2)

### 6. Files and databases
- [x] file manager per group (Files tab): browse, edit text files like
      .env, upload (drag and drop, 2 GB), download files and folders as
      zip, new folder, rename, delete to .trash (30 days); every
      operation runs as the group's user; Monaco, keyboard control,
      folder uploads, copy, extract .zip/.tar(.gz) for big uploads
- [x] database download (Database page, or a token with the `databases`
      scope for the whole server): a Craft database (detected by its
      tables) without personal data via chuchi-dev/craft-dbstrip, others
      complete
- [x] tokens belong to the admin who created them and go with them;
      Start → API Tokens lists all with their owner

### 7. Code review follow-up (2026-11-01)
- [x] Rust bugs: apply lock across processes, deploy/backup output that
      isn't UTF-8 hangs, aborted uploads, stuck running rows after a
      restart, restart after a failed after-switch step, swallowed errors
- [x] Svelte bugs: editor recreated on ctrl+s, Escape closes every
      dialog, events dead after log off and on, double log loads,
      swallowed errors
- [x] duplicated helpers (commands, job logs, tokens, stamps; log views,
      form saving, prompt/confirm as Win95 dialogs, colors), long
      components split (Files, Deploys, Agent, Group)

### 8. Routines and alerts (operator 2026-11-01, after 7)
- [x] checks without the LLM every minute (`checks.rs`): failed units,
      sites not answering or 500+ (3 minutes), disk 90%, load over 15
      minutes above 1.5 × cores, memory under 10% (5 minutes),
      certificates under 14 days, last backup failed or older than a day,
      failed deploys; a new problem starts an alert conversation, the
      same problem again only after 4 hours
- [x] agent routines (`routines.rs`, server's local time): daily health
      check 07:00 incl. pending updates, weekly security review Monday
      07:30 against the inventory; Run menu in the agent window
- [x] routines and alerts end with `STATUS: OK|ATTENTION`; a failed run
      or no API key counts as ATTENTION; turning to ATTENTION mails the
      admins; tray icon in the taskbar, Resolved button

### 8b. Activity log and invites (operator 2026-11-01)
- [x] activity log of every change: API middleware (admin or token,
      action, target, names from the body, outcome), logins incl. failed
      ones, `genco` commands (agent, systemd or terminal), the agent's
      tool calls, alerts and routines; Start → Activity Log, `genco
      activity`, kept a year
- [x] admins are invited by mail and set their own password (link valid
      3 days, "Send Link" again doubles as password reset)
- [x] or added with a password (Start → Users → Add…, confirmed with the
      admin's own password), they can log in right away (operator
      2026-11-03)
- [x] an admin changes their own email and name (Start → Change
      Email…, with the password), the old address gets a mail about it

### 8c. Usage stats (operator 2026-11-01, instead of Grafana)
- [x] a sample every minute into /var/lib/genco/stats.db (no backup):
      server CPU, memory, disk, network; group CPU and memory of its
      slice, disk of files and databases (hourly du), requests, errors
      and traffic of its domains; domain requests, 5xx and bytes sent
      from its nginx log (without the server's own addresses), CPU and
      memory of an app; minutes for 2 days, hours for 400 days
- [x] charts on the server page, a Usage tab of a group with a table per
      domain, and on the domain page; 1h, 24h, 7d, 30d
- [x] top pages of a domain (operator 2026-11-03): `GET
      /api/domains/{id}/top?hours=` reads the newest 50 MB of its access
      log (and `.1`) on request: the 50 most requested paths without the
      query (requests, 4xx, 5xx, bytes) and the top 404s, no user
      agents, and the top addresses (requests, 4xx, 5xx, their most
      requested path); lists under the domain's charts; admins only, no
      tokens: paths are text anyone can log, IPs personal data

### 10. Mail hosting (operator 2026-11-01)
- [x] mail domains in a group with mailboxes (Dovecot 2.4, IMAPS 993,
      maildir in /var/vmail as user vmail, bcrypt, quota) and forwards
      (`*` catch-all); Postfix receives on 25, sends for mailboxes on
      587/465 after a login and only as the mailbox; one host
      `mail.<panel domain>` with its certificate; Dovecot's config is
      genco's alone (no PAM logins), it runs only with mail domains
- [x] DNS helper (A, MX, SPF, DMARC, SRV, PTR: wanted, found, status) in
      the panel and `genco mail dns`; the agent explains what to change
- [x] mail in the group backups, deletions by mail with an archive
- [x] webmail: SnappyMail on the mail host (operator: try it, Roundcube
      if it doesn't convince); `genco setup` installs or updates it,
      checked against its release key in the repo; own user and PHP-FPM
      in the groups' sandbox, data in /var/lib/genco-webmail, one domain
      setting for every login (this server's IMAPS and SMTP with login),
      its admin panel off
- [x] spam filter and DKIM: Rspamd with Redis as milter (15 rejects, 6
      goes to Junk by a global Sieve rule, no greylisting), a DKIM key
      per mail domain in /etc/genco/dkim signs mail of mailboxes and of
      the sites, its record in the DNS helper; postsrsd rewrites the
      sender of forwarded mail (SRS) except for the server's own domains
- [x] send-only mail (operator 2026-11-03): a mail domain whose inbox is
      elsewhere receives nothing here but is signed; send-only addresses
      are an app's own SMTP login that sends only as itself (envelope
      and From header, no IMAP), password encrypted with secrets.key;
      app settings in the panel, `genco mail send-only --env` writes
      `ZIPP_SMTP_URL` into the app's environment without printing it
- [x] mail per address (operator 2026-11-04): Postfix's journal is
      counted every minute per day and address (received, forwarded,
      rejected, bounced, sent with a login), only counts and the last
      mail, 90 days in the stats database; the mail domain's page and
      `genco mail stats` show which mailboxes and forwards are used
- [ ] later: learn spam/ham from moves into and out of Junk (imapsieve),
      Sieve filters and vacation replies in the webmail (ManageSieve),
      mailbox owners change their password themselves

### 10b. Password store (operator 2026-11-01)
- [x] passwords for the admins (Start → Passwords, `genco secret`):
      name, group, user, URL, note; the value encrypted (ChaCha20-
      Poly1305) with /etc/genco/secrets.key, shown or copied only on
      request and logged; the agent stores the logins it creates there
      instead of files or the chat

### 10c. Agent costs (operator 2026-11-01)
- [x] every llm request with its tokens and price (Anthropic list prices
      in costs.rs, cache reads and writes apart), per day on the server
      page split into conversations, routines and alerts, with a table

### 11. Builds, artifacts, Postgres, moving servers in place (operator 2026-11-01)
- [x] PHP settings per group (upload size, post_max_size, memory_limit,
      max_execution_time, max_input_vars), nginx's body limit and
      timeout follow; `genco group set` (also repo/branch) and
      `genco group deploy-key`; genco.toml documented for the agent
- [x] artifact deploys: `PUT /deploy/artifact` with a deploy token or
      `genco deploy --artifact`, unpacked as the group's user, no build
      steps; before every switch an `ldd` check of the release's binaries
- [x] `GET /deploy/target` and `genco runners list`: OS, release,
      glibc, arch
- [x] README: Debian 13 is the tested and recommended base
- [x] `genco runner` (operator: instead of the ssh builder, which is
      gone): the same binary on a build box connects out to its genco
      servers (token per runner), long-polls for jobs, builds
      `on = "runner"` steps in podman or docker with an image for the
      server's Debian (`rust:1-{codename}`), sends the log and the
      artifacts back; caches per server, group and folder. Builds wait
      in a queue until a runner takes them, never on the server
      (operator, after three deploys at once on gs2: the third fell
      back after 5 minutes and had no cargo); queue timeout, early
      failure without an online runner, cancel. Settings → Runners, `genco runners`, `genco runner
      install|register|run`. a Rust app built cold in 2m51s on a
      build box (9m20s on the server alone), warm deploy 22 s
- [ ] on-demand machines: genco starts a cloud machine with a cache disk
      that runs `genco runner --once` and stops it after (Fly Machines or
      AWS, per second; Hetzner bills per hour); CI templates (GitHub
      Actions, GitLab) upload artifacts
- [ ] from deploying a Rust app: [x] environment variables and
      secrets per app domain (written to an env file the unit loads);
      [x] app units start only once a release exists (they crash-looped
      and alerted before the first deploy); [x] a time limit per build
      step; [x] Cargo detected without genco.toml (build caches outside
      the release, the binary copied into it); [x] SQLite files backed up with
      `sqlite3 .backup` like the MariaDB dumps; [x] a hint which MAIL_FROM
      domains this server may send for (genco mail senders); a Rust toolchain only through
      a runner (genco runner), not rustup on the server
- [x] PostgreSQL databases like MariaDB ones (isolation, backups, dumps,
      imports), installed only when the first one is created
- [ ] moving a Docker based server in place (the domains point to its
      IP): save every database, volume and compose file first, then
      replace the old setup with genco on the same box; genco's
      agent can do the inventory and the move with an approved plan

### 11b. Updates and notifications (operator 2026-11-01)
- [x] genco updates itself from signed GitHub releases (ssh-keygen
      signatures, key in CI as GENCO_RELEASE_KEY, public half in
      `server/src/update/allowed_signers` and install.sh): `genco update
      [--check] [--yes] [--version]`, Settings → Updates; backup of binary
      and database, `genco setup`, restart, health check, rollback
- [x] nightly check at 04:00, patch releases install themselves unless
      turned off per server
- [x] weekly update round (Monday 07:15): Debian updates, reboots, end of
      life, genco releases in one report
- [x] notifications: mail to the admins and/or a webhook (JSON POST with
      a bearer token), OK reports optionally too; noah takes them at
      `/api/observe/<server>/genco`

### 11c. Group limits (operator 2026-11-03)
- [x] memory, CPU and tasks of a group's slice as settings (panel
      General tab, `genco group set --memory --cpu --tasks`), applied to
      the running slice without restarts
- [x] overcommit (operator 2026-11-12): all groups capped together at
      the RAM without a reserve (genco.slice), shared services with a
      low OOM score, alerts on memory pressure and OOM kills instead of
      the sum of limits, the group's usage and 24 h peak beside its limit
- [x] drop-ins that override genco's units reported by apply, the minute
      check and the config view; `genco group adopt-limits`
- [x] ImageMagick limits for groups with PHP, from the group's memory

### 11d. Approvals (operator 2026-11-07)
- [x] Start → Approvals: proposals (#12) and pending actions (A12) in one
      list with who asked, what happens, expiry; 30 days of history;
      the taskbar and Find cover both
- [x] confirming in the panel with password and 2FA code for both kinds,
      without 2FA only rejecting (mail off: the password)
- [x] rejecting an action with a reason, back to the agent's
      conversation; a personal mail link per admin for deletions too

### 9. Access for other agents (operator 2026-11-01, later)
- [x] a `read` token scope: `GET /api/status` (server, groups with their
      services and last deploy, backups, problems, reports that need
      attention) and `GET /api/stats`, without the LLM; for the dashboard
      of all servers
- [x] Start → AI Agents (operator 2026-11-02): one key per agent, not
      an admin's (read, databases, propose), its webhook gets the reports
      (the notification webhook moved there) and the answers to its
      proposals; an admin's own token only downloads databases
- [x] Tasks from another agent are untrusted: they wait as a
  proposal until an admin approves them (mail to all admins, Proposals
  tab), then the agent gets them. Routine things go through scoped tokens without the LLM (deploy,
  database download). A cleaning LLM may flag suspicious requests in the
  mail, it is no protection on its own.
- [x] ~~A small model rewrites a proposal into a short formatted version
  (operator 2026-11-03)~~, replaced by plans (operator 2026-11-12): a
  planner in a read-only sandbox turns a proposal into a short plan, the
  admin approves it (2FA), asks for a change (a new version) or rejects
  it; genco's agent gets only the approved plan, never the proposal; the
  plan's `Delete:` lines are confirmed with it, once each
- [x] A rejection can have a reason for the proposing agent, an expired
  proposal tells it once

### 11e. Quarantine and signs of a hack (operator 2026-11-12)
- [x] quarantine a group (panel, CLI, genco's agent): services stopped,
      sites 503, SMTP login and deploys off; lifted by an admin or the
      agent (when asked, or after it cleaned the cause)
- [x] signs of a compromised group in the minute check (CPU, programs
      from /tmp or deleted, shells under PHP-FPM, mining pool ports, mail
      bursts, PHP files in uploads): they only wake the agent

### Later
- Automatic bans without the llm (operator 2026-11-12, postponed):
  rate limits per site, genco bans an IP that floods or scans for a
  short time, longer when it comes back; admins' addresses protected.
- A check of the sites from outside (operator 2026-11-12, postponed):
  not generally, maybe from noah or an external service.
- GitHub deploys tested, malware scanning (Imunify or CrowdSec,
  ClamAV).
- A dashboard on another device or domain that shows the status of all
  servers, over the API with a token that may only read.

## Backlog

Small things found on the way, not part of a milestone yet.

From the review round of 2026-11-01, done in wave 2 (2026-11-02):
- [x] downloads with single-use tickets instead of the session token
- [x] two-factor on/off asks for the password, an accepted invite resets it
- [x] deploys: a lock per group, one waiting deploy per group (the newest
  ref wins), steps have their time limits
- [x] one loop per conversation over all processes (`genco chat`)
- [x] DKIM for the sites: a group's SMTP login (g-<group>, 587) may send
  as its mail domains and is signed; mail() over localhost:25 stays
  unsigned
- [x] read tokens get keys instead of free text in /status
- [x] keyboard in list views (lib/listKeys: arrows, Home/End, Enter)
- [x] an invite page survives a reload
- [x] unknown tokens answer "Unknown or deleted token"
- [x] a deleted domain's own folder goes to the group's trash
- [x] /etc/nginx/genco is 0750 root

Still open:
- `www-data` is a member of every group, so nginx could read `shared/`
  (.env) too. Narrower (ACLs on the web roots only) breaks uploads in
  `shared/` behind links (Craft's web/assets): needs a model per shared
  folder first.
- An app restart (deploy, rollback) answers 502 for a few seconds
  unless the domain has "Restart without downtime" (opt-in since 0.6.0,
  two slots; a Node site on a move showed it); Apply Changes still
  restarts in place.
- Drop-down menus (Start, Run, conversations) could share one component.
- A group's PostgreSQL role sees the names of the other databases
  (pg_database), not their content. Kept (2026-11-03): hiding them means
  revoking pg_database from the roles, which breaks pg_dump, Craft's
  backups and many drivers.
- A release whose app doesn't start stays `current` (with and without
  "Restart without downtime"): the old process keeps serving what it
  loaded, but a restart or reboot of the app would start the broken
  release; the deploy fails and says so. Kept on purpose (operator,
  2026-11-03): no automatic rollback, the admin rolls back in the Deploy
  tab or deploys a fix.
- Rspamd + Redis only when needed (operator, 2026-11-03): genco setup
  installs and starts them unconditionally; on a 3.8 GB server without
  mail domains rspamd took 5 processes and ~600 MB. Start them (and the
  milter settings) with the first mail domain, stop them without one.
  Done (2026-11-03), postsrsd too.
- postfix inet_interfaces: setup and every apply set loopback-only
  without mail domains; Docker containers sending through
  host.docker.internal:25 (172.17.0.1) lose their mail, and a manual
  change is reset by the next apply. Allow the docker bridge (listen on
  it, mynetworks) while containers need it, or keep a manual setting.
  Done (2026-11-03): docker0's address and the bridges' networks.
- DNS checks: dns::dig takes the first name server's answer with
  +norecurse, also an empty one from a lame server (a delegated server
  that doesn't serve the zone): a domain got "no DNS record" and no
  certificate. Treat empty/REFUSED/non-authoritative (no aa flag)
  answers as no answer and try the next server.
- `genco panel rename` that moves the mail host leaves the certificate
  of mail.<old name> behind (`certbot delete --cert-name` by hand), and
  `forget-old` can't tell whether runners still use the old name (the
  old name's access log shows /api/ calls).
- Moving a site with a database: an app's first start after `genco
  database add` runs its migrations on the empty database and restoring
  the dump afterwards collides. Documented (import the data before the
  first deploy, OPERATIONS.md and the agent's instructions); maybe a
  check later.
- Automatic IP blocking (operator, 2026-11-03): not yet. First watch real
  traffic (Top addresses) once sites have moved; then probably the
  agent's health check suggests blocks with a reason instead of fixed
  rules. Port 22 is never blocked over HTTP traffic; SSH brute force
  (keys only) isn't blocked either for now.
- Chat attachments in `/var/lib/genco/uploads` are never deleted: decide
  on a cleanup (e.g. after 90 days, or with their conversation; operator
  not sure yet, 2026-11-02).
