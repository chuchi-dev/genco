# Operations without the panel

This runbook is for when the web UI is broken, the agent has no API credit
left, or the server is gone. Everything here works as root in a shell; you
need neither the panel nor an LLM.

1. [From a fresh server to genco](#1-from-a-fresh-server-to-genco)
2. [Running the server without the panel](#2-running-the-server-without-the-panel)
3. [Restoring one site](#3-restoring-one-site)
4. [Moving or rebuilding the whole server](#4-moving-or-rebuilding-the-whole-server)
5. [Where things are](#5-where-things-are)
6. [Notifications and the agent's model](#6-notifications-and-the-agents-model)

**Do this today, not on the day you need it:** keep a copy of
`/etc/genco/restic-password` outside the server, for example in your
password manager. Without it nobody can read the backups. Also note that
the backups are currently only on the server's own disk
(`/var/backups/genco/restic`). If the disk dies, they are gone too, so
until offsite backups exist, copy the repository away now and then:

```sh
rsync -a --delete root@server:/var/backups/genco/restic/ ~/genco-restic/
```

## 1. From a fresh server to genco

You start with a Debian 13 server, its IP, and a login for `administrator`
with a password. Root may not log in over ssh with a password.

1. **DNS first.** Point the panel's domain (e.g. `genco.example.com`) to
   the IP. Without that there is no certificate, and the panel runs on
   plain http until you run `genco apply` later.
2. **Log in and become root.**

   ```sh
   ssh administrator@IP
   su -            # the root password; or `sudo -i` if administrator may sudo
   ```

3. **Add your ssh key for root.** `genco setup` switches ssh to keys only,
   but only once a user has a key. Without one it skips this step, so you
   don't lock yourself out.

   ```sh
   mkdir -p /root/.ssh && chmod 700 /root/.ssh
   echo 'ssh-ed25519 AAAA... you@laptop' >> /root/.ssh/authorized_keys
   chmod 600 /root/.ssh/authorized_keys
   ```

   In a second terminal, check that `ssh root@IP` works before going on.
4. **Install genco.**

   ```sh
   curl -fsSL https://github.com/chuchi-dev/genco/releases/latest/download/install.sh | bash
   ```

   While the repository is private, the download doesn't work. Build the
   binary on your machine instead (`scripts/build.sh`), copy it and the
   script over, and run:

   ```sh
   scp server/target/x86_64-unknown-linux-musl/release/genco scripts/install.sh root@IP:/root/
   ssh root@IP 'GENCO_BINARY=/root/genco bash /root/install.sh'
   ```

   It asks for:
   - the panel's domain;
   - an email address for Let's Encrypt;
   - the agent's API key: Anthropic's, or (left empty) OpenRouter's; one
     of them is needed, the agent runs Claude Opus 5.5;
   - the first admin's email, name and password.

   Then `genco setup` does the rest:
   - writes an inventory of the server to `/var/lib/genco/inventory.txt`;
   - sets ssh to keys only and the firewall to ssh, http and https;
   - turns on security updates;
   - installs PHP 8.2 to 8.5, Node, MariaDB, nginx, postfix and restic;
   - sets up the nightly backup at 03:00;
   - starts `genco.service`.

   The script is safe to run again. genco updates itself (see the
   table below), the script is the fallback when genco doesn't run.
5. **Log in** at `https://<domain>`, turn on two-factor login under your
   user, and save `/etc/genco/restic-password` somewhere outside the
   server.

## 2. Running the server without the panel

The panel only writes genco's database. `genco apply` turns that database
into the server's config, and the CLI does the same:

| Task | Command |
| --- | --- |
| See all clients, groups, domains and databases | `genco tree` |
| Write all config, start services, get certificates | `genco apply` |
| Deploy a group from git | `genco deploy <group> [--ref v1.2]` |
| Deploy a release built elsewhere | `genco deploy <group> --artifact release.tar.gz --ref v1.2` |
| Build runners for `on = "runner"` steps | `genco runners list`, `genco runners add <name>` (prints the token), `genco runners remove <name>` |
| Make a box a runner | on it: `genco runner install`, `genco runner register https://<panel> <token>`, `systemctl restart genco-runner` |
| Back up now | `genco backup` |
| Add a client, group, domain or database | `genco client add`, `genco group add`, `genco domain add`, `genco database add` (see `--help`) |
| Change a domain | `genco domain set <name> --php 8.4` |
| Which sender domains apps may use (SPF) | `genco mail senders` |
| A PostgreSQL database | `genco database add <name> --group <g> --postgres`, then `genco apply` (installs PostgreSQL the first time) |
| Environment variables of an app | `genco domain env <name> --set NAME=value --secret NAME=value --unset NAME`, then `genco apply` |
| A stored password (Start → Passwords) | `genco secret list`, `genco secret show <name>` |
| Reset an admin's password | `genco user password <email>` (reads the password from stdin) |
| An admin lost their authenticator | `genco user totp-off <email>` |
| Who did what (panel, tokens, agent, terminal) | `genco activity [--limit 200]` |
| Talk to the agent in the terminal | `genco chat` (needs API credit) |
| What genco version is new | `genco update --check` |
| Update genco | `genco update --yes` (signature check, backup, rollback; log in `/var/lib/genco/updates/update.log`) |
| A test server: install own builds, never update by itself | `genco update --mode testing`, then `genco update --yes --file ./genco` (unsigned, same backup and rollback) |
| Go back to an older genco | `genco update --yes --version 0.1.2`, or `cp /usr/local/bin/genco.previous /usr/local/bin/genco && systemctl restart genco` |

Delete commands only send the admins a confirmation mail; the deletion
happens when someone opens the link.

If the panel itself is down:

```sh
systemctl status genco
journalctl -u genco -n 100
systemctl restart genco
```

The sites don't depend on genco running. nginx, php-fpm, the apps and
MariaDB are normal systemd services:
- the apps run as `genco-<group>-<domain>.service`;
- PHP runs as `genco-<group>-php<version>.service`.

`systemctl list-units 'genco-*'` shows them. A broken genco binary only
stops the panel and new changes. To get a working binary back, run the
install script again.

genco checks the server every minute and runs a health check every
morning. When the agent can't run them (no credit, no key), the admins
still get a mail "genco needs attention" with the problem (and the AI
agents' webhooks a POST, Start → AI Agents).

**If the agent has no credit:** the panel keeps working; only the agent
window answers with an error. Top up the key, or put another one into
`/etc/genco/config.toml`, either `anthropic-key = "..."` or an
`openrouter-key = "..."` as the fallback. Then run
`systemctl restart genco`.

## 3. Restoring one site

The backup is a restic repository. Each night it gets:
- per group, one snapshot of the files, tagged `group:<group>` and
  `files`;
- per database, one snapshot of a dump, tagged `group:<group>` and
  `db:<name>`;
- one snapshot of `/etc` and genco's database, tagged `system`.

Restic keeps the last 3, 7 daily, 4 weekly and 6 monthly snapshots. Tags joined with a
comma must all match.

`genco restic ...` is plain restic with the repository and the password
already set. If the genco binary is broken, set them yourself and use
`restic` instead:

```sh
export RESTIC_REPOSITORY=/var/backups/genco/restic
export RESTIC_PASSWORD_FILE=/etc/genco/restic-password
restic snapshots --tag group:shop
```

### Pick a snapshot

```sh
genco restic snapshots --tag group:shop          # files and databases of group shop
genco restic ls latest --tag group:shop,files    # what is in the newest one
```

Use `latest` or a snapshot ID (the first column) in the commands below.

### Files

Restore into a temporary directory first, never straight over the site:

```sh
genco restic restore latest --tag group:shop,files --target /tmp/restore
ls /tmp/restore/srv/genco/shop
```

Then copy what you need back. Restore a single file or folder:

```sh
cp -a /tmp/restore/srv/genco/shop/shared/web/uploads/. /srv/genco/shop/shared/web/uploads/
```

SQLite databases (`*.db`, `*.sqlite`): the live file may have been copied
in the middle of a write, so each snapshot also holds a consistent copy
in `.genco-sqlite/`, named after its path (`shared/cms/data/zipp.db` is
`.genco-sqlite/shared_cms_data_zipp.db`). Stop the app, put that copy in
place and remove the old `-wal` and `-shm` files next to it:

```sh
systemctl stop genco-shop-shop.example.ch
cp /tmp/restore/srv/genco/shop/.genco-sqlite/shared_data_app.db /srv/genco/shop/shared/data/app.db
rm -f /srv/genco/shop/shared/data/app.db-wal /srv/genco/shop/shared/data/app.db-shm
chown g-shop:g-shop /srv/genco/shop/shared/data/app.db
systemctl start genco-shop-shop.example.ch
```

Or put the whole group back as it was (this deletes files that are newer
than the snapshot):

```sh
rsync -a --delete /tmp/restore/srv/genco/shop/ /srv/genco/shop/
chown -R g-shop:g-shop /srv/genco/shop   # needed if the user's id changed (other server)
genco apply
rm -rf /tmp/restore
```

The backup leaves out `node_modules`, caches and the git mirror. Node
apps need their packages again, and a deploy brings them back:
`genco deploy shop`. Without a git repository, run `npm ci` in the app as
the group user: `runuser -u g-shop -- npm ci`.

### A database

Restore into a new database first and look at it:

```sh
mariadb -e 'CREATE DATABASE shop_restored'
genco restic dump latest --tag group:shop,db:shop /shop.sql | mariadb shop_restored
mariadb shop_restored -e 'SHOW TABLES'
```

To replace the live database (the site sees the old data from then on):

```sh
mariadb-dump shop > /root/shop-now.sql          # in case you need it back
genco restic dump latest --tag group:shop,db:shop /shop.sql | mariadb shop
mariadb -e 'DROP DATABASE shop_restored'
```

The dump drops and recreates each table it contains; tables added after
the snapshot stay.
The file name inside the snapshot is always `/<database>.sql`. For a
Craft site, run `php craft clear-caches/all` as the group user afterwards.

## 4. Moving or rebuilding the whole server

The same steps work for a server that died and for moving to a new one.
You need:
- the restic repository: from the old server, or the copy you keep;
- the restic password;
- a fresh Debian 13 server.

1. **Prepare the new server** as in [section 1](#1-from-a-fresh-server-to-genco),
   steps 2 and 3 (root and your ssh key). Don't install genco yet.
2. **Bring the repository and the password over.**

   ```sh
   mkdir -p /var/backups/genco /etc/genco && chmod 700 /var/backups/genco
   rsync -a old-server:/var/backups/genco/restic /var/backups/genco/   # or from your copy
   install -m 600 /dev/stdin /etc/genco/restic-password   # paste the password, then Ctrl-D
   apt install -y restic
   export RESTIC_REPOSITORY=/var/backups/genco/restic RESTIC_PASSWORD_FILE=/etc/genco/restic-password
   restic snapshots --tag system
   ```

3. **Restore genco's config, database and certificates.** Restore only
   these paths. The rest of the old `/etc` (network, fstab, ssh host keys)
   belongs to the old machine.

   ```sh
   restic restore latest --tag system --target /tmp/sys
   cp -a /tmp/sys/etc/genco/config.toml /etc/genco/
   cp -a /tmp/sys/etc/genco/secrets.key /etc/genco/   # the stored passwords need it
   cp -a /tmp/sys/etc/letsencrypt /etc/
   mkdir -p /var/lib/genco && chmod 700 /var/lib/genco
   cp /tmp/sys/var/lib/genco/genco-backup.db /var/lib/genco/genco.db
   ```

   If the panel's domain changes, edit `domain` in `config.toml` now.
4. **Install genco** as in section 1, step 4. The script finds the
   config, the database and the password, so it asks nothing. Setup and
   `genco apply` recreate the following, with empty sites and databases:
   - users and groups;
   - PHP pools and app units;
   - nginx;
   - databases and grants.
5. **Restore every group.** For each group in `genco tree`, restore its
   files, then each of its databases:

   ```sh
   restic restore latest --tag group:shop,files --target /tmp/restore
   rsync -a /tmp/restore/srv/genco/shop/ /srv/genco/shop/
   chown -R g-shop:g-shop /srv/genco/shop
   restic dump latest --tag group:shop,db:shop /shop.sql | mariadb shop
   ```

   Then run `genco apply` once, `genco deploy <group>` for the groups
   with Node apps, and `rm -rf /tmp/restore /tmp/sys`.
6. **Test before switching DNS.** Point the domains to the new IP in your
   own `/etc/hosts` and open the sites. Then change DNS. Certificates for
   domains that had none yet come with the next `genco apply` once the DNS
   points here. The restored ones renew as usual.
7. **Shut the old server down only after** the first nightly backup has
   run on the new one: `genco restic snapshots --tag system` shows
   today's date.

## 5. Where things are

| What | Where |
| --- | --- |
| genco's config (domain, API keys, mail relay) | `/etc/genco/config.toml` |
| genco's database (clients, groups, domains, admins) | `/var/lib/genco/genco.db` |
| Backup repository / password | `/var/backups/genco/restic`, `/etc/genco/restic-password` |
| A group's files | `/srv/genco/<group>`, linux user `g-<group>` |
| Deploy releases | `/srv/genco/<group>/releases`, `current` links to the live one |
| nginx sites | `/etc/nginx/genco/<domain>.conf`, included by `/etc/nginx/conf.d/genco.conf` |
| PHP pools / app scripts | `/etc/genco/php/`, `/etc/genco/apps/` |
| Server inventory before genco | `/var/lib/genco/inventory.txt`, old firewall `/etc/nftables.conf.before-genco` |
| Deleted via the panel (30 days) | `/var/backups/genco/trash` |
| Logs | `journalctl -u genco`, `journalctl -u genco-backup`, `/srv/genco/<group>/logs` |

## 6. Notifications and the agent's model

### Where reports go

The agent's reports are its routines (daily health check, weekly update
round, weekly security review), the minute checks' alerts and update
notices.

- **Mail** (Settings → Notifications): the reports that need attention
  go to every admin.
- **Webhooks** of AI agents (Start → AI Agents): an `https://` address
  per agent. genco POSTs JSON with the agent's key as
  `Authorization: Bearer <key>`, the reports that need attention, or all
  of them with "OK reports too". "Test" sends one with kind `test`. The
  receiver answers with any 2xx within 20 seconds; genco doesn't retry,
  the report stays in the panel either way.

```json
{
  "server": "genco.example.ch",
  "status": "attention",
  "kind": "report",
  "attention": true,
  "title": "Daily health check",
  "text": "the report in markdown",
  "url": "https://genco.example.ch/conversations/12"
}
```

`status` is `ok` or `attention`, `kind` is `report` (routines and
alerts), `update`, `proposal` (the answer to a proposal, with its id in
`proposal`) or `test`, `url` opens the conversation in the panel.
Typical receivers are another agent that looks after several servers,
or a small relay to a chat: Slack's or Teams' incoming webhooks expect
their own JSON (`{"text": ...}`), so they need a few lines in between
that turn `title` and `text` into it.

### AI agents

Start → AI Agents lists the other AI agents that look after the server.
Each has one key, made when it is added (or with New Key…), shown once
and not tied to an admin. With it as `Authorization: Bearer <key>` the
agent may:

- read the status: `GET /api/status`, `GET /api/stats`
- download a database: `GET /api/databases/<name>/dump` (a Craft one
  without its personal data)
- propose a task: `POST /api/proposals` with `{"text": "...",
  "approver": "admin@example.ch"}` answers `{"id": 3, "status":
  "pending"}`. The admin with that email gets a mail and only they may
  decide; without `approver` every admin gets it and any decides. With
  `"mailDelay": 10` the mail goes only if nobody decided within 10
  minutes (at most 1440), meanwhile the taskbar shows "1 to approve" to
  the admins who may decide. The task waits in the Proposals tab (it
  shows the approver) until it is approved or rejected, it expires after
  7 days. Approved, it becomes a conversation of the
  server agent; when the agent is done, its answer goes to the agent's
  webhook (kind `proposal`) and `GET /api/proposals/<id>` shows the
  status and `answer`.

An admin's own token that downloads every database is in Start → API
Tokens; deploy tokens are in a group's Deploy tab.

### The agent's API key and model

The agent needs an Anthropic or an OpenRouter key in
`/etc/genco/config.toml`:

```toml
anthropic-key = "sk-ant-..."     # Claude directly at Anthropic
openrouter-key = "sk-or-v1-..."  # or through OpenRouter, or as fallback
model = "anthropic/claude-opus-5.5"  # an OpenRouter model id, the default
```

With `anthropic-key`, Anthropic's models go directly to Anthropic; if
Anthropic refuses a request and there is an `openrouter-key`, it goes
through OpenRouter. Settings → Agent chooses the model: Claude Opus 5.5,
Claude Sonnet 5.5, or with `openrouter-key` also Gemini 4 Argon and
GPT-6.1 Sol. The choice applies to the next request, no restart. `model`
in the config is the default until one is chosen there and may be any
other OpenRouter id. After a change of the keys run `systemctl restart
genco`. The costs are under the server's page in the Explorer (Agent
costs).

### Showing the panel in another site

The panel refuses to be shown in a frame, so no other site can trick an
admin into clicking in it. A dashboard that shows it in a frame is
allowed with `frame-ancestors = ["https://dashboard.example.ch"]` in
`/etc/genco/config.toml` (https addresses only), then `systemctl
restart genco`. The login inside the frame is separate from the one in a
browser tab.
