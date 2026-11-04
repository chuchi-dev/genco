# Operations without the panel

This runbook is for when the web UI is broken, the agent has no API credit
left, or the server is gone. Everything here works as root in a shell; you
need neither the panel nor an LLM.

1. [From a fresh server to genco](#1-from-a-fresh-server-to-genco)
2. [Running the server without the panel](#2-running-the-server-without-the-panel)
3. [Restoring one site](#3-restoring-one-site)
4. [Moving or rebuilding the whole server](#4-moving-or-rebuilding-the-whole-server)
5. [Renaming the panel](#5-renaming-the-panel)
6. [Where things are](#6-where-things-are)
7. [Notifications and the agent's model](#7-notifications-and-the-agents-model)

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
| Deploy from CI and follow it | a deploy token (group → Deploy tab): `POST /api/deploy {group, ref}` gives the id, `GET /api/deploys/<id>` its `status` and `commitId`, `GET /api/deploys/<id>/log?offset=<n>` the log from byte n on (header `x-log-offset` says where to go on); the Deploy tab has a GitLab job that does all this. The log lands in the CI job's log, readable by everyone who sees the job. |
| Build runners for `on = "runner"` steps | `genco runners list`, `genco runners add <name>` (prints the token), `genco runners remove <name>` |
| Make a box a runner | on it: `genco runner install`, `genco runner register https://<panel> <token>`, `systemctl restart genco-runner` |
| Update a runner | on it: `genco runner update` (a daily timer of `genco runner install` does it; Settings → Runners warns when a runner runs another version) |
| Back up now | `genco backup` |
| Add a client, group, domain or database | `genco client add`, `genco group add`, `genco domain add`, `genco database add` (see `--help`) |
| Change a domain | `genco domain set <name> --php 8.4` |
| Give a group more memory, CPU or tasks | `genco group set <g> --memory 2.5G --cpu 200% --tasks 1024`, then `genco apply` (see "Group limits" below) |
| Restart a Node or Rust app without downtime | `genco domain set <name> --zero-downtime true`, then `genco apply` (see below) |
| Which sender domains apps may use (SPF) | `genco mail senders` |
| An app sends as a domain whose inbox is elsewhere | `genco mail add-domain <domain> --group <g> --send-only` (or the mail domain's page: Mail → Make Send-only…), then `genco mail sender add noreply@<domain>`: it prints Host, Port 587, User, Password and `ZIPP_SMTP_URL=`; set the SPF (add this server's `ip4:`) and DKIM records of `genco mail dns <domain>` (see "Send-only mail" below) |
| DNS for genco's own mails (genco@<panel>, signed with selector `genco-panel`) | Settings → Mail: SPF, DKIM, DMARC and PTR with their state |
| Send genco's mails through a relay as its account | Settings → Mail → Through an SMTP relay, Sender `ie@example.ch` (From and bounce address; signed by genco only as the panel's own domain) |
| Why a mail doesn't arrive | Settings → Mail → Send Test Mail waits for postfix and shows its reason (deferred, bounced, a failed relay login); it also says when outgoing port 25 seems blocked (`genco setup` checks it too) |
| A server without mail (port 25 blocked, no relay) | Settings → Mail → No mail: deletions are confirmed in the panel with password and 2FA code, no invites, no new mail domains, reports only in the panel and webhooks |
| A PostgreSQL database | `genco database add <name> --group <g> --postgres`, then `genco apply` (installs PostgreSQL the first time) |
| Environment variables of an app | `genco domain env <name> --set NAME=value --secret NAME=value --unset NAME`, then `genco apply` |
| A stored password (Start → Passwords) | `genco secret list`, `genco secret show <name>` |
| Block an address or range from the sites and mail (never ssh) | `genco firewall block 203.0.113.7 [--for 24h] [--note "..."]`, `genco firewall unblock <ip|cidr>`, `genco firewall list`; Settings → Firewall (see below) |
| Reset an admin's password | `genco user password <email>` (reads the password from stdin) |
| An admin lost their authenticator | `genco user totp-off <email>` |
| Who did what (panel, tokens, agent, terminal) | `genco activity [--limit 200]` |
| Talk to the agent in the terminal | `genco chat` (needs API credit) |
| What genco version is new | `genco update --check` |
| Update genco | `genco update --yes` (signature check, backup, rollback; log in `/var/lib/genco/updates/update.log`) |
| A test server: install own builds, never update by itself | `genco update --mode testing`, then `genco update --yes --file ./genco` (unsigned, same backup and rollback) |
| Go back to an older genco | `genco update --yes --version 0.1.2`, or `cp /usr/local/bin/genco.previous /usr/local/bin/genco && systemctl restart genco` |

Delete commands only send the admins a confirmation mail; the deletion
happens when someone opens the link. With mail off they print the link
instead, an admin confirms it there with their password.

`genco apply` never takes over a database it didn't create: when a
PostgreSQL or MariaDB database of a genco database's name exists already
and isn't the group's (another role owns it, the group has no grant on
it), apply leaves it alone, sets up everything else and ends with an
error naming it. Rename one of the two, or hand the database over by
hand (PostgreSQL: `ALTER DATABASE <name> OWNER TO "g-<group>"`;
MariaDB: grant `g-<group>`@localhost all privileges on it, with `_` in
the name written as `\_`) and run `genco apply` again. Deleting a database
in genco drops it and, in MariaDB, the group's grant on its name.

nginx reloads gracefully; when its master keeps the old config after a
reload (no new workers within 10 s), apply restarts nginx and says so,
which cuts the open connections once.

Send-only mail: a mail domain whose inbox is elsewhere (its MX points to
another provider) is send-only. genco receives nothing for it: it is not
in Postfix's `virtual_mailbox_domains`, so mail from this server to it
goes out to its real MX, and it takes no mailboxes or forwards (a domain
becomes send-only only without them). It gets a DKIM key and is signed
like the others. Apps send as it with send-only addresses (`genco mail
sender add|list|show|password|remove`, the domain's page in the panel):
each is its own SMTP login on 587, user the whole address, that may send
only as that address (the group's `g-<group>` login keeps all of the
group's domains). Their password is encrypted with
`/etc/genco/secrets.key`, the panel and `genco mail sender show` show it
again. They live in Dovecot's passdb for SMTP logins, which has no
userdb, so IMAP and the webmail refuse them. Send-only addresses work on
receiving domains too (noreply@ without a mailbox), never on an address
that is a mailbox or forward. The DNS check of a send-only domain has no
MX: its SPF has to list this server (add `ip4:<ip>` to the record that
is there), the DKIM record `genco._domainkey`, DMARC recommended. With
mail through a relay the relay's SPF counts instead; with No mail there
are no new send-only domains or addresses.

Mail services follow the mail domains: without one, `genco apply` keeps
Dovecot, Rspamd, Redis and postsrsd stopped and disabled (installed, they
start with the first mail domain) and Postfix listens only on localhost
and, while Docker runs, on docker0's address, so containers still send
through `host.docker.internal:25`. The Docker bridges' networks may relay
in either case (`mynetworks`). Postfix starts after Docker (and starts
it) where Docker is installed. Docker counts as installed only with its
`docker.service` unit and `dockerd`, and not with a masked unit: bridges
left after a purge without a reboot don't count, and Podman's (`podman0`,
`cni-podman0`) never do.
Without Docker, apply sets Postfix back to `loopback-only`, drops the
bridge networks from `mynetworks`, removes the drop-in that orders it
after Docker and restarts it. Postfix refuses to start when an address of
`inet_interfaces` is gone: after removing Docker, run `genco apply` before
the next reboot.

The blocklist (Settings → Firewall, `genco firewall`) drops IPv4 and
IPv6 addresses or ranges on ports 80, 443, 25, 465, 587, 993 and the
workers' public ports, never on ssh. A change loads at once (genco's
nftables table in `/etc/genco/nftables.conf`, which `/etc/nftables.conf`
includes at boot); an entry with an end (`--for 24h`, `7d`) is removed
by the minute check. Ranges broader than /16 (IPv4) or /32 (IPv6),
loopback, the server's own addresses and those admins logged in from in
the last 30 days are refused. Locked out of the panel by a block: ssh
in and run `genco firewall unblock <address>`.

If the panel itself is down:

```sh
systemctl status genco
journalctl -u genco -n 100
systemctl restart genco
```

The sites don't depend on genco running. nginx, php-fpm, the apps and
MariaDB are normal systemd services:
- the apps run as `genco-<group>-<domain>.service`;
- PHP runs as `genco-<group>-php<version>.service`; an app starts after
  its group's PHP units and restarts 3 s after it exits.

- an app with "Restart without downtime" has a second unit
  `genco-<group>-<domain>_b.service` on port 30000 + domain id; the
  two take turns, only the live one is enabled.

Restart without downtime (a domain option for Node and Rust apps, off
by default): a deploy or rollback starts the new release in the slot
that isn't live, waits up to 60 s until it accepts connections on its
port, points nginx at it (`nginx -t`, reload, a restart if the reload
doesn't take) and then stops the old process; when nginx doesn't take the
new config the old process stays live. If the new one never listens it
is stopped, the old one stays live and the deploy fails;
`current` points to the new release all the same (no automatic
rollback), so a restart or reboot of the app would start it: roll back in the Deploy tab or deploy a fix. Apply Changes (a new environment variable)
still restarts the live process in place. It is opt-in because old and
new process run at the same time for a few seconds: migrations must
work with the old code for that moment, background jobs inside the app
may run twice and SQLite apps may see a short lock. Switching it off
goes back to slot A with the next `genco apply`.

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

### Group limits

Every group runs in its own systemd slice, `genco-<group>.slice`: all its
processes (PHP, apps, workers, schedules, deploys) share its limits. The
defaults are 1.5 GB memory (`MemoryMax`, slowing down from 1 GB,
`MemoryHigh`), 150% CPU (one and a half cores) and 512 tasks. Change them
on the group's General tab (Limits) or with

```sh
genco group set shop --memory 2.5G --cpu 200% --tasks 1024
genco group set shop --memory-high 2G   # the soft limit, else 2/3 of --memory
genco group set shop --memory default   # back to the default
genco apply
```

Sizes take `G` or `M` (a plain number is MB). The memory limit is 128 MB
to the server's RAM, CPU 10% to 100% per core, tasks 64 to 65536. Above
`MemoryHigh` the kernel slows the group down and takes back memory, at
`MemoryMax` it kills a process of the group (the OOM killer, in `journalctl
-k`). `genco apply` writes the slice and sets the new limits on the running
slice (`systemctl set-property --runtime`, systemd doesn't apply a changed
slice file before a unit starts in it), also on a stopped one, whose old
runtime values would win over the file; nothing restarts. `systemctl show
genco-<group>.slice -p MemoryMax` or
`/sys/fs/cgroup/genco.slice/genco-<group>.slice/memory.max` show what holds
now; `genco tree` has each group's `limits` (`null` is the default, its
`defaults` what an empty one means).

When the groups' memory limits add up to more than the server's RAM,
saving warns ("Group memory limits add up to 6.5 GB, the server has 3.9
GB and no swap"). It is no error: groups rarely all use their limit at
once, but if they do, the server runs out of memory before one of them
hits its own. The minute check reports it as a problem only once a group
has a memory limit of its own (many groups on the defaults add up on a
small server, that is normal), and whenever a single group's limit is
more than 90% of the RAM.

**Drop-ins.** Files in `/etc/systemd/system/genco-*.slice.d/` or
`genco-*.service.d/` override what genco writes, so the panel's limits
would no longer be what holds; so do the prefix folders `genco-.slice.d/`
and `genco-.service.d/` (every group) and `/etc/systemd/system.control/`,
where `systemctl set-property` without `--runtime` writes. genco's own
are in `/run/systemd/system.control/`. genco never writes or removes them by
itself: `genco apply` lists them as warnings, the minute check reports each
as a problem, and the group's Configuration tab shows them. To move the
limits of a slice drop-in (`MemoryHigh`, `MemoryMax`, `CPUQuota`,
`TasksMax`) into the group's settings:

```sh
genco group adopt-limits shop
```

It reads the drop-ins of `genco-shop.slice.d` (in `/etc/systemd/system`
and `system.control`) in systemd's order, moves the files to
`/var/backups/genco/trash`, stores the limits (nothing when a file can't
be moved), reloads systemd and runs `genco apply`. A drop-in with other settings (or `infinity`, a
percentage) is refused; move that one by hand.

**ImageMagick.** PHP's Imagick keeps a photo's pixels in memory: a Craft
transform of a large original takes 500 to 700 MB and can push a group
over its limit. For groups with PHP genco sets `MAGICK_MEMORY_LIMIT` (a
quarter of the group's `MemoryHigh`, at most 256 MiB), `MAGICK_MAP_LIMIT`
(twice that) and `MAGICK_THREAD_LIMIT=1` in the environment of its
php-fpm, workers and schedules; above them ImageMagick uses its disk cache
in `/tmp` (the unit's own), slower but within the limit. ImageMagick reads them
when PHP starts, so a site's `.env` can't change them (it is loaded too
late); only `Imagick::setResourceLimit()` in the site's code does, and
`/etc/ImageMagick-*/policy.xml` can only lower them. Resizing originals on
upload keeps transforms fast.

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

   If the panel's domain changes, rename it once genco runs, see
   [section 5](#5-renaming-the-panel).
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

   Import a database before the first deploy or start of its app: an
   app that starts on the empty database runs its migrations there, and
   the dump then collides with the tables it created. The same holds for
   a site moving in from elsewhere: `genco database add`, then `genco
   database import <name> <file>` (or the Databases tab), then the
   deploy. If an app already migrated the empty MariaDB database, drop
   it and let `genco apply` create it again (PostgreSQL imports drop
   what was there already):

   ```sh
   mariadb -e 'DROP DATABASE shop'
   genco apply
   genco database import shop /root/shop.sql.gz
   ```
6. **Test before switching DNS.** Point the domains to the new IP in your
   own `/etc/hosts` and open the sites. Then change DNS. Certificates for
   domains that had none yet come with the next `genco apply` once the DNS
   points here. The restored ones renew as usual.
7. **Shut the old server down only after** the first nightly backup has
   run on the new one: `genco restic snapshots --tag system` shows
   today's date.

## 5. Renaming the panel

Never edit the name by hand, one command changes everything that comes
from it:

```sh
genco panel rename panel.example.ch
```

1. It checks the new name: a valid domain, not a domain, mail domain or
   the mail host of this server, and its A or AAAA record points here
   (`--force` renames anyway, the certificate comes with a later
   `genco apply`).
2. It sets `domain` in `/etc/genco/config.toml` (the other lines stay)
   and writes `/etc/mailname`.
3. `genco apply` writes the panel's site, gets its certificate and sets
   postfix's `myhostname`.
4. The old name keeps answering: every page redirects (308) to the new
   name, only `/api/` is still served, because runners, CI jobs and git
   webhooks don't follow redirects.
5. It prints the DNS records the new name needs (A, SPF, DKIM
   `genco-panel._domainkey.<new name>`, DMARC, PTR) and which records of
   the old name can go later.
6. Runners keep the URL in `/etc/genco/runner.toml` on their own box; it
   prints the command for them:
   `sed -i 's|https://<old>|https://<new>|' /etc/genco/runner.toml &&
   systemctl restart genco-runner`.
7. genco restarts a few seconds later and answers as the new name.

With mail domains the mail host `mail.<panel>` moves along unless
`mail-host` is set in the config. That changes what every mail program
uses for IMAP and SMTP, the webmail's address, and the MX and SRV records
of every mail domain; the new mail host must point here first, the
command prints the records. `--keep-mail-host` keeps the old one instead
(it writes `mail-host = "mail.<old name>"`).

Once runners, CI jobs and webhooks use the new name, the old one goes
with its certificate:

```sh
genco panel forget-old old.example.ch
```



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
| Logs | `journalctl -u genco`, `journalctl -u genco-backup`, `/srv/genco/<group>/logs`, per domain `/var/log/nginx/genco/<domain>.access.log` and `.error.log`; logrotate keeps 14 days (`/etc/logrotate.d/genco`, `genco-<group>`) |

## 7. Notifications and the agent's model

### Where reports go

The agent's reports are its routines (daily health check, weekly update
round, weekly security review), the minute checks' alerts and update
notices.

- **Mail** (Settings → Notifications): the reports that need attention
  go to every admin, not with mail off in Settings → Mail.
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
alerts), `update`, `proposal` (the answer to a proposal, its rejection
or expiry, with its id in `proposal`) or `test`, `url` opens the conversation in the panel.
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
  "pending"}`. The admin with that email gets the mail, without
  `approver` every admin does; any admin decides. With
  `"mailDelay": 10` the mail goes only if nobody decided within 10
  minutes (at most 1440), meanwhile the taskbar shows "1 to approve" to
  every admin. With mail off (Settings → Mail) no mail goes out, the
  answer has a `note` saying so and only the taskbar shows it. The task
  waits in the Proposals tab (it shows who got
  the mail) until it is approved or rejected, it expires after
  7 days. Approved, it becomes a conversation of the
  server agent; when the agent is done, its answer goes to the agent's
  webhook (kind `proposal`) and `GET /api/proposals/<id>` shows the
  status and `answer`. Rejected, the webhook gets "<admin> rejected
  the proposal: <reason>" (or "<admin> rejected the proposal." without
  a reason) and `answer` is the reason; the admin's reason is optional,
  at most 2000 characters, cleaned like the text. Approving takes an
  optional comment the same way ("yes, but keep the old DNS for now"):
  genco's agent gets it after the proposal as the admin's change and
  follows it where the two differ, and the answer to the webhook
  starts with "<admin> approved it with a comment: <comment>", the
  proposal's `comment` holds it. Nobody decided within 7 days, the
  status becomes `expired` and the webhook gets "The proposal expired
  after 7 days without a decision." once.

  `text` says what the agent wants and why, not a long list of steps:
  genco's agent works out the details itself. Before the admins see
  it, a small, cheap model (Gemini Flash with an `openrouter-key`,
  else Claude Haiku) rewrites it into a short Markdown version (a
  title line, then what and why, the steps at a high level, what
  changes, risks), so the request may take up to 30 seconds. The admin
  approves that version and genco's agent gets it; the panel shows the
  original too. A rewrite is dropped when it has a word with a digit,
  dot, slash, @ or colon the original doesn't have as a whole word,
  `code` that isn't in the original as it is, or none of the
  original's negations and warnings ("not", "don't", "only", "keep",
  "risk", "nicht", ...); then, and without a key, on an error or a
  timeout, the original goes as it is and the panel says why ("Not
  rewritten: timeout"). An agent has at most 10 undecided proposals,
  those still being rewritten count too, so parallel requests can't
  run up the model's bill. genco removes invisible characters
  (zero-width, bidi controls) and control characters from the text
  and turns other whitespace into spaces; the panel says how many were
  removed and warns (the mail too) when the original or the rewrite has
  non-Latin letters (e.g. Cyrillic look-alikes in a name or address).

  Each admin's mail has their own link (`https://<panel>/proposal/…`)
  that decides without logging in, handy on a phone: it shows the
  proposal as the panel does (with its warnings and the original),
  Approve and Reject… (with the optional reason). Opening it changes
  nothing (mail scanners open links too). Approve needs more than the
  mailbox, the approval and its comment steer the root agent: the 2FA
  code with two-factor login on, otherwise the admin's password; wrong
  ones count toward the login's limits. Reject needs neither, it can't
  make anything run. The mail has the links first and the proposal's
  text below a line, so a link in the text can't pass for genco's;
  nginx logs the panel's token links as `/proposal/...`. The links of a proposal stop working once anybody
  decided it, in the panel or by a link, or it expired; they work only
  on the panel's domain, and an address that tries 20 links that don't
  work waits 15 minutes. The activity log names the admin ("Approve
  proposal via the mail link"), never the link.

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
browser tab. A listed site (or a bug in it) can make an admin click in
the framed panel, so list only one host you trust.
