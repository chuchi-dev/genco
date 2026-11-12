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
password manager. Without it nobody can read the backups. The backups
are on the server's own disk (`/var/backups/genco/restic`): if the disk
dies, they are gone too. Set up the remote copy (Settings → Backups,
[below](#the-remote-copy-backblaze-b2-s3)) and keep its bucket key and
repository password outside the server too. Without a remote copy,
copy the repository away now and then:

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
| Runner builds queue | a step with `on = "runner"` never builds on the server: its deploy waits in the queue until a runner takes it, oldest first; the log says the position and what the runners do. It fails after the queue timeout (Settings → Runners, 2 h; `wait = <minutes>` on the step can only shorten it), when no runner is online for 15 minutes, or at once without any runner. Cancel in the group's Deploy tab takes it out of the queue. Only detected steps (no genco.toml) build here, on a server without runners |
| Back up now | `genco backup` |
| Copy the backups off the server (Backblaze B2, S3) | Settings → Backups, or `genco backup remote set --b2 --key-id <keyID> --bucket <bucket> [--path genco/<panel>]` (the application key on stdin; see "The remote copy" below); `genco backup remote show`, `genco backup remote test`, `genco backup remote off` |
| List and restore snapshots | `genco backup snapshots [--tag group:shop] [--remote]`, `genco backup restore latest --tag group:shop,files --target /var/lib/genco/restore/shop [--remote]`; anything else with `genco restic [--remote] ...` |
| Add a client, group, domain or database | `genco client add`, `genco group add`, `genco domain add`, `genco database add` (see `--help`) |
| Change a domain | `genco domain set <name> --php 8.4` |
| Give a group more memory, CPU or tasks | `genco group set <g> --memory 2.5G --cpu 200% --tasks 1024`, then `genco apply` (see "Group limits" below) |
| Restart a Node or Rust app without downtime | `genco domain set <name> --zero-downtime true`, then `genco apply` (see below) |
| Which sender domains apps may use (SPF) | `genco mail senders` |
| Which mailboxes, forwards and send-only addresses are used | `genco mail stats [<domain>]` or the mail domain's page (see "Mail counts" below) |
| An app sends as a domain whose inbox is elsewhere | `genco mail add-domain <domain> --group <g> --send-only` (or the mail domain's page: Mail → Make Send-only…), then `genco mail send-only add noreply@<domain> --env <group>[/<domain>]`: it writes `ZIPP_SMTP_URL` with the password into the app's environment (then `genco apply`) and prints Host, Port 587 and User, the password only with `--show-password`; set the SPF (add this server's `ip4:`) and DKIM records of `genco mail dns <domain>` (see "Send-only mail" below) |
| DNS for genco's own mails (genco@<panel>, signed with selector `genco-panel`) | Settings → Mail: SPF, DKIM, DMARC and PTR with their state |
| Send genco's mails through a relay as its account | Settings → Mail → Through an SMTP relay, Sender `ie@example.ch` (From and bounce address; signed by genco only as the panel's own domain) |
| Why a mail doesn't arrive | Settings → Mail → Send Test Mail or `genco mail test <to>` waits for postfix and shows its reason (deferred, bounced, a failed relay login; the command exits non-zero then); the panel also says when outgoing port 25 seems blocked (`genco setup` checks it too) |
| A server without mail (port 25 blocked, no relay) | Settings → Mail → No mail: deletions are confirmed in the panel with the 2FA code (the password without 2FA), no invites, no new mail domains, reports only in the panel and webhooks |
| genco sends mail but hosts none | Settings → Mail → uncheck Mail hosting: no new mail domains, mailboxes, forwards or send-only addresses (panel, CLI and agent refuse them); genco's own mails (confirmations, invites, proposal links) keep going. Existing mail domains stay and keep running until deleted; without any, Dovecot, Rspamd and the submission ports stay off as always. No mail implies hosting off |
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

A domain's kind (`genco domain add|set --kind`) says what serves it:

| Kind | Serves | Options |
| --- | --- | --- |
| `static` | the files in `/srv/genco/<group>/<root>` | `--root` |
| `php` | the same with PHP-FPM (`genco-<group>-php<version>.service`) | `--root`, `--php 8.4` |
| `node`, `rust` | an app on port 20000 + domain id (`genco-<group>-<domain>.service`), nginx serves the public folder | `--root`, `--command`, `--public`, `--zero-downtime` |
| `proxy` | forwards every request to another web server | `--upstream http://127.0.0.1:8081` |
| `redirect` | sends every request (http and https, ACME challenges excepted) to another address, no files, no units | `--target https://example.ch/`, `--keep-path true` (default; the request's path and query follow the target, which then has no query), `--status 301` (default), 302, 307 or 308 |

`--www true` adds www.<domain> with its own certificate: it redirects to
the domain, a redirect's www straight to its target in one hop. Use the
redirect kind for redirects, not a `return` in `--nginx-extra` or a
domain of its own for www. Changing the kind removes what the old one
had (units, ports, a redirect's target and options) with the next
`genco apply`; the group's files stay.

`--nginx-extra` goes into the domain's https server block, and into the
port 80 one only until the domain has its certificate; the www name never
gets it. On a redirect domain that is beside the `location /` with the
redirect, e.g. a header or a `location = /robots.txt`. ACME challenges
are a `^~` location, a regex location there can't take them.

Delete commands only ask: the deletion gets a number (`A12`), waits in
Start → Approvals and every admin gets a mail with their own link; it
happens when an admin confirms it. With mail off they print a link
instead, an admin confirms it there or in Approvals with their password.

### Approvals

Start → Approvals (and the "N to approve" in the taskbar, its tooltip
lists them) shows what waits for an admin, newest first: the plans of
the AI agents' proposals (`#12`, see [AI agents](#ai-agents); those the
planner still works on are listed too) and the pending actions
(`A12`): deletions of domains, databases, groups, mail domains and
mailboxes, and the agent's changes of the remote backup. Each says who
asked (an admin, the terminal, an AI agent, or genco's agent with its
conversation, "Open Chat"), what exactly happens (for a database its
size), when it was requested and when it expires (a proposal after 7
days, an action after 24 hours). Below are the decided and expired ones
of the last 30 days. Find (Ctrl+K) finds both by number ("A12", "#12")
and text.

Confirming in the panel takes one more factor, the same for both kinds
(the session already proves the password):

- an admin with two-factor login confirms with a 2FA code (the code is
  used up, wrong codes count toward the login's limit);
- an admin without 2FA can only reject in the panel; the link in the mail
  confirms (turn on Start → Two-Factor Login to confirm in the panel);
- with mail off (Settings → Mail) nothing is mailed, so the password
  confirms, or the 2FA code when the admin has 2FA.

The mails stay the way to confirm from a phone: each admin gets their own
link, the real link comes first and the description (it may hold names
the agent chose) below a line. A deletion's link (`/confirm/…`) works for
the admin it was mailed to, logged in to the panel; a proposal's link
works without a login (see below). Links stop working once anybody
decided, or it expired.

Rejecting needs no second factor, it can't make anything run. The
reason is optional (at most 2000 characters): a proposal's goes to the
proposing agent, an action's into the conversation of genco's agent that
asked for it (as a system message, "<admin> rejected A12 (…), it won't
run. The reason: …"). The activity log has every confirmation and
rejection ("Confirm request", "Reject request" with the number).

API (an admin's session): `GET /api/approvals` lists both kinds with a
`kind` field (`proposal` with `proposal`, `action` with `action`), `POST
/api/actions/<id>/confirm` with `{"password", "code"}`, `POST
/api/actions/<id>/reject` with `{"message"}` (optional), `POST
/api/proposals/<id>/approve` with `{"plan", "password", "code"}` (`plan`
is the id of the newest plan, the one the admin read) and `POST
/api/proposals/<id>/change` with `{"plan", "note"}`.

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
send-only add|list|show|password|remove`, the domain's page in the
panel): each is its own SMTP login on 587, user the whole address, that
may send only as that address: Postfix checks the envelope sender, the
Rspamd rule `/etc/rspamd/lua.local.d/genco.lua` rejects mail whose From
header is another address (the group's `g-<group>` login keeps all of
the group's domains). No login sends with the null sender `<>` on 587
and 465. postmaster@ of every domain and genco@, abuse@ of the panel's
domain are no send-only addresses. Their password is encrypted with
`/etc/genco/secrets.key`; the CLI prints it only with `--show-password`,
`--env <group>[/<domain>] [--name ZIPP_SMTP_URL]` writes the SMTP URL
into the app's environment as a secret instead (on `add`, `show` and
`password`), the panel shows it behind Show. They live in Dovecot's passdb for SMTP logins, which has no
userdb, so IMAP and the webmail refuse them. Send-only addresses work on
receiving domains too (noreply@ without a mailbox), never on an address
that is a mailbox or forward. The DNS check of a send-only domain has no
MX: its SPF has to list this server (add `ip4:<ip>` to the record that
is there), the DKIM record `genco._domainkey`, DMARC recommended. With
mail through a relay the relay's SPF counts instead; with No mail or mail
hosting off there are no new send-only domains or addresses.

Mail counts: every minute genco reads Postfix's new lines in the journal
(from a cursor in the stats database, so a restart counts nothing twice;
the first run on a server starts at that moment) and counts per day and
address of the receiving mail domains how a mail ended: received
(delivered to a mailbox here), forwarded (sent on to another server),
rejected (refused for good while it came in: unknown address, Rspamd's
spam reject) or bounced, all by the address it was sent to (before
forwards and the catch-all); and per SMTP login (send-only addresses,
mailboxes, the group's `g-<group>`) what it sent. A mail to a forward
with two targets counts once; mail still being retried counts when it
ends. Only counts and the time of each address's last mail are kept,
never senders, subjects or message ids: the counts for 90 days, the last
mail for 400 days (of rejected addresses 90). The mail domain's page
shows the last 30 days per mailbox (received and sent), forward (the
catch-all sums up the addresses without their own) and send-only address
with the last mail, the domain's totals, and the 10 refused unknown
addresses tried most (a domain keeps 200 of them per day, the rest count
as `(others)@domain`);
`genco mail stats [<domain>]` prints the same.

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

The groups' units (apps, PHP, workers, schedules, deploy steps) run in a
sandbox as the group's user: they see only the group's files and listen
on TCP only on their own port (an app's `PORT`, a worker's `--port`),
every other TCP bind is denied (`SocketBindDeny=tcp:1-65535`). Outgoing
TCP connections (SMTP, HTTP) bind port 0 and get an ephemeral port. So
that port 0 never lands on another unit's port while it restarts,
`apply` reserves genco's ports (the panel, 20000 to 39999, the workers'
ports, MariaDB, PostgreSQL, Redis, Rspamd, postsrsd, ManageSieve) from
the ephemeral range in `/etc/sysctl.d/60-genco.conf`
(`net.ipv4.ip_local_reserved_ports`, for IPv6 too); the owning unit
still binds its port. `apply` warns when `ip_local_port_range` starts
below Debian's 32768. Apps may use UDP freely for client traffic (DNS,
NTP; resolvers like hickory that pick their own port work); inbound UDP
stays closed by the firewall. To check a sandbox by hand:

```sh
systemd-run --wait --pipe -q -p DynamicUser=yes \
  -p SocketBindAllow=tcp:20003 -p SocketBindDeny=tcp:1-65535 \
  python3 -c 'import socket as s
for t, p in [(s.SOCK_STREAM, 0), (s.SOCK_DGRAM, 40000),
             (s.SOCK_STREAM, 20003), (s.SOCK_STREAM, 20004)]:
    try: s.socket(s.AF_INET, t).bind(("127.0.0.1", p)); print(p, "ok")
    except OSError as e: print(p, e)'
# 0 ok, 40000 ok, 20003 ok, 20004 [Errno 1] Operation not permitted
```

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

Restore into a directory of its own first, never straight over the site.
`genco backup restore` makes it only for root (0700) and refuses places
where others write or could put links in its way (`/srv/genco`,
`/var/vmail`, `/var/www`, `/home`, `/tmp`, a directory not root's alone);
without `--target` it uses `/var/lib/genco/restore/<time>`:

```sh
genco backup restore latest --tag group:shop,files --target /var/lib/genco/restore/shop
ls /var/lib/genco/restore/shop/srv/genco/shop
```

Then copy what you need back. Restore a single file or folder:

```sh
cp -a /var/lib/genco/restore/shop/srv/genco/shop/shared/web/uploads/. /srv/genco/shop/shared/web/uploads/
```

SQLite databases (`*.db`, `*.sqlite`): the live file may have been copied
in the middle of a write, so each snapshot also holds a consistent copy
in `.genco-sqlite/`, named after its path (`shared/cms/data/zipp.db` is
`.genco-sqlite/shared_cms_data_zipp.db`). Stop the app, put that copy in
place and remove the old `-wal` and `-shm` files next to it:

```sh
systemctl stop genco-shop-shop.example.ch
cp /var/lib/genco/restore/shop/srv/genco/shop/.genco-sqlite/shared_data_app.db /srv/genco/shop/shared/data/app.db
rm -f /srv/genco/shop/shared/data/app.db-wal /srv/genco/shop/shared/data/app.db-shm
chown g-shop:g-shop /srv/genco/shop/shared/data/app.db
systemctl start genco-shop-shop.example.ch
```

Or put the whole group back as it was (this deletes files that are newer
than the snapshot):

```sh
rsync -a --delete /var/lib/genco/restore/shop/srv/genco/shop/ /srv/genco/shop/
chown -R g-shop:g-shop /srv/genco/shop   # needed if the user's id changed (other server)
genco apply
rm -rf /var/lib/genco/restore/shop
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

### The remote copy (Backblaze B2, S3)

After each good full backup (the nightly one or Back Up Now) genco copies
the new snapshots with `restic copy` into a second repository off the
server, then removes old ones there (by default it keeps 14 daily, 8
weekly and 12 monthly per group and kind). Everything is in the backup's
log. A failed copy doesn't fail the backup but is a problem of its own,
like no successful copy for more than 2 days.

Set it up in Settings → Backups (your 2FA code, or password without
2FA) or as root:

```sh
# Backblaze B2: a bucket and an application key for it (read, write, delete)
genco backup remote set --b2 --key-id 0051234abcd --bucket acme-backups
# then paste the applicationKey, Enter, Ctrl-D
# S3-compatible storage
genco backup remote set --s3 --endpoint https://s3.example.com --region eu-1 \
	--key-id AKIA... --bucket acme-backups [--path genco/panel] \
	[--keep-daily 14 --keep-weekly 8 --keep-monthly 12] [--limit-upload 2048]
genco backup remote test    # connects, creates the repository if there is none
```

Where the copy goes decides where all data goes. The panel asks for your
2FA code (your password without 2FA); when the agent runs `genco backup
remote set` or `off`, it only asks: it waits in Start → Approvals with a number and every
admin gets a mail to confirm it (with mail off, the agent gets a link where
an admin confirms it with their password), the same as a deletion. Root in a terminal changes it directly. Whoever does
it, every admin gets a mail (and the AI agents' webhooks a notice) when
the copy goes somewhere else or is turned off, and a turned off copy is a
problem for a week (`backup:remote-off`), so it can't stop silently.

The path in the bucket is `genco/<panel name>` by default. The key and
the repository password are stored encrypted with
`/etc/genco/secrets.key` and go to restic only in its environment; the
panel shows neither.

genco makes the remote repository's own password and shows it **once**,
when you save. When an admin confirmed the agent's request, the agent
never sees it: Settings → Backups shows "Show Password" (your 2FA code,
or password without 2FA) to the first admin who opens it, once. Keep it
outside the server with the bucket's key, in your
password manager: if the server is lost, the remote copy is useless
without it. To take over an existing remote repository (a new server, see
[section 4](#4-moving-or-rebuilding-the-whole-server)), enter its password
in "Repository password", or as the second line of stdin of `genco backup
remote set`.

The server's key may delete (restic removes old snapshots), so a
compromised server can delete the copy too. Against that, let the bucket
keep deleted files for a while: at Backblaze B2 the lifecycle setting
"Keep prior versions for this number of days", at S3 versioning with a
lifecycle rule.

Restoring from the remote copy works like from the local one, with
`--remote`:

```sh
genco backup snapshots --remote --tag group:shop
genco backup restore latest --tag group:shop,files --target /var/lib/genco/restore/shop --remote
genco restic --remote dump latest --tag group:shop,db:shop /shop.sql | mariadb shop_restored
```

The panel's snapshot lists switch between "On this server" and "Remote
copy". Needs restic 0.14 or newer (`restic copy --from-repo`): Debian 12
has 0.14, Debian 13 0.18.

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
   restic restore latest --tag group:shop,files --target /var/lib/genco/restore/shop
   rsync -a /var/lib/genco/restore/shop/srv/genco/shop/ /srv/genco/shop/
   chown -R g-shop:g-shop /srv/genco/shop
   restic dump latest --tag group:shop,db:shop /shop.sql | mariadb shop
   ```

   Then run `genco apply` once, `genco deploy <group>` for the groups
   with Node apps, and `rm -rf /var/lib/genco/restore /tmp/sys`.

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

### From the remote copy only

When the server and its disk are gone, the remote copy is enough. You
need the bucket's key (B2 keyID and applicationKey, or the S3 access and
secret key), the bucket and path, and the remote repository's password.

1. **Prepare the new server** as in section 1, steps 2 and 3.
2. **Read the remote copy with plain restic:**

   ```sh
   apt install -y restic
   export RESTIC_REPOSITORY=b2:acme-backups:genco/panel.example.com
   export B2_ACCOUNT_ID=0051234abcd
   read -rs B2_ACCOUNT_KEY && export B2_ACCOUNT_KEY       # the applicationKey
   read -rs RESTIC_PASSWORD && export RESTIC_PASSWORD     # the remote repository's password
   restic snapshots --tag system
   ```

   For S3: `RESTIC_REPOSITORY=s3:https://s3.example.com/acme-backups/genco/panel.example.com`
   with `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` (and
   `AWS_DEFAULT_REGION`).
3. **Restore genco's config, database and certificates** as in step 3
   above, with `restic restore latest --tag system --target /tmp/sys`,
   and also the old local repository's password, so the new local
   backups use the same:

   ```sh
   mkdir -p /etc/genco && cp -a /tmp/sys/etc/genco/restic-password /etc/genco/
   ```

   genco's database holds the remote settings, encrypted with the
   restored `secrets.key`: the new server copies to the same remote.
4. **Install genco** as in step 4 above, then check the remote:
   `genco backup remote show` and `genco backup remote test`. If the
   restored settings don't work (another bucket key), set them again
   with the password as the second line:
   `printf '%s\n%s\n' "$B2_ACCOUNT_KEY" "$RESTIC_PASSWORD" | genco backup remote set --b2 --key-id ... --bucket ... --path ...`.
5. **Restore every group** as in step 5 above, from the remote:

   ```sh
   genco backup restore latest --tag group:shop,files --target /var/lib/genco/restore/shop --remote
   rsync -a /var/lib/genco/restore/shop/srv/genco/shop/ /srv/genco/shop/
   chown -R g-shop:g-shop /srv/genco/shop
   genco restic --remote dump latest --tag group:shop,db:shop /shop.sql | mariadb shop
   ```

   Mail of a group's mail domains is in its own snapshot:
   `genco backup restore latest --tag group:shop,mail --target /var/lib/genco/restore/mail --remote`,
   then `rsync -a /var/lib/genco/restore/mail/var/vmail/ /var/vmail/` and `genco apply`.
6. Steps 6 and 7 above (test, DNS, the first nightly backup).

What this doesn't cover: there is no one-command restore of the whole
server, each group is restored by hand as above; what changed after the
last nightly copy is lost; usage stats (`stats.db`), `node_modules`,
caches and git mirrors were never backed up (a deploy brings them back);
nothing outside the groups, `/etc` and `/var/lib/genco` is in the backup
(software installed by hand, root's files, databases genco doesn't
manage); and the old `/etc` beyond genco's files belongs to the old
machine. The restored certificates renew as usual once DNS points here.

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
  "planning"}`. `text` says what the agent wants and why, not a long
  list of steps.

  genco's planner turns the proposal into a plan first: the same model
  as genco's agent (Settings → Agent), with one tool, a shell in a
  read-only sandbox. Root there has no capability but reading files;
  the whole file system is read-only, there is no network, no unix
  socket (so no systemd, databases, docker or postfix), no other
  process to see, and genco's secrets, `/etc/shadow`, ssh keys and
  certificate keys are hidden. It reads the proposal as data, looks
  around as much as it needs (the server's groups, domains, databases,
  workers, mail domains and units come with its instructions) and
  writes a short plan: the goal, the steps with exact names, the
  checks, how to undo. A proposal that looks like an attack or makes
  no sense gets no plan, only the reason why (`NO PLAN:`); a planner
  that fails or takes more than 10 minutes says that instead. genco
  removes invisible and control characters from the plan and refuses
  one over 4000 characters (the planner gets one more try); the panel
  and the mail show the plan exactly as genco's agent will get it, and
  warn about non-Latin letters (e.g. Cyrillic look-alikes in a name).

  The admin with the `approver` email gets the mail once the plan is
  there, without `approver` every admin does; any admin decides. With
  `"mailDelay": 10` the mail goes only if nobody decided within 10
  minutes (at most 1440), meanwhile the taskbar shows "1 to approve" to
  every admin. With mail off (Settings → Mail) no mail goes out, the
  answer has a `note` saying so and only the taskbar shows it. The
  proposal waits in Start → Approvals (it shows who got the mail):

  - **Approve** takes the admin's second factor (see
    [Approvals](#approvals)). The plan becomes a conversation of genco's
    agent: it gets the plan as the approving admin's message, never the
    proposal, and carries it out without doing more; if the plan turns
    out wrong it stops and explains. Lines like `Delete: database
    shop_old` (also `domain`, `group`, `mail domain`, `mailbox`, one
    name each) are confirmed with the plan: while genco's agent works
    on it, those deletions run without another confirmation, once each,
    still to the trash first. Any other deletion asks as usual. The
    panel and the mail list them ("Deletes without asking again").
  - **Change…** sends the plan back to the planner with the admin's
    note ("keep the old DNS for now"), no second factor: the planner
    can't do anything, and the new version needs an approval again.
    The mail goes out again; at most 10 versions.
  - **Reject…** with an optional reason, also while the planner works.

  The panel shows the agent's request only on demand (Request). When
  genco's agent is done, its answer goes to the agent's webhook (kind
  `proposal`) and `GET /api/proposals/<id>` shows the status and
  `answer`, without the plans (they may hold what the planner read on
  the server). Rejected, the webhook gets "<admin> rejected the
  proposal: <reason>" (or "<admin> rejected the proposal." without a
  reason) and `answer` is the reason; the reason is optional, at most
  2000 characters, cleaned like the text. Nobody decided within 7
  days, the status becomes `expired` and the webhook gets "The proposal
  expired after 7 days without a decision." once. An agent has at most
  10 undecided proposals, so a loop can't run up the planner's bill.

  Each admin's mail has their own link (`https://<panel>/proposal/…`)
  that decides without logging in, handy on a phone: it shows the plan
  as the panel does (with its warnings, the deletions and the request
  on demand), Approve, Change… and Reject…. Opening it changes nothing
  (mail scanners open links too). Approve needs more than the mailbox,
  the plan steers the root agent: the 2FA code with two-factor login on,
  otherwise the admin's password; wrong ones count toward the login's
  limits. Change and Reject need neither, they can't make anything run;
  after a change the link shows the new version once it is there. The
  mail has the links first and the plan below a line, so a link in the
  plan can't pass for genco's; nginx logs the panel's token links as
  `/proposal/...`. The links of a proposal stop working once anybody
  decided it, in the panel or by a link, or it expired; they work only
  on the panel's domain, and an address that tries 20 links that don't
  work waits 15 minutes. The activity log names the admin ("Approve
  proposal via the mail link"), never the link, and has every command
  of the planner (actor `planner`).

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
