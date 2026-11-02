Weekly update round. Find out what can be updated on this server, install
nothing without an admin (unattended-upgrades installs Debian's security
updates by itself, genco its patch releases at night if allowed), and
write one report an admin can act on:

- Debian: `apt-get update`, then `apt list --upgradable`; security
  updates separately (`-security` in the origin), and which of them
  unattended-upgrades should have installed but didn't
  (`/var/log/unattended-upgrades/`). What it installed this week, in one
  line.
- Restarts: does the box need a reboot (`/var/run/reboot-required`,
  `.pkgs`), do services still run old libraries (`needrestart -b`)?
- Release: is this Debian release still supported (`/etc/debian_version`,
  a newer point or major release)? PHP versions in use that are end of
  life or close to it.
- genco: `genco update --check`: what is installed, what is new, and
  whether it is a patch, minor or major release (link the release
  notes). The last update and whether it worked is in `genco activity`.
  A server in testing mode (a test server) gets its builds by hand:
  don't ask for genco releases there.
- Apps: nothing to update there yourself; mention a site's own updates
  only if you saw something notable (Craft or WordPress security notices
  in their logs or panels).

For every update an admin should install, say how: the command (`apt-get
upgrade`, `genco update --yes`) or the button (Settings → Updates), and
whether it restarts something or needs a reboot.

End with a line `STATUS: OK` if everything is current or only waits for
the automatic updates, or `STATUS: ATTENTION` if an admin should install
something (security updates that didn't come by themselves, a reboot, a
new genco minor or major release, an end of life). ATTENTION notifies
the admins.
