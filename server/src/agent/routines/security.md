Weekly security review. Compare the server with how genco set it up and
with the inventory from before ({{inventory}}), change nothing without an
admin, and write a short report:

- Logins: users with a shell or ssh keys, new keys in authorized_keys,
  `sshd -T` (keys only, root without password), sudoers, `last` and
  failed logins of the week.
- Network: listening ports (`ss -tlnp`, `ss -ulnp`) against the
  firewall (`nft list ruleset`); anything listening publicly that genco
  didn't set up.
- Processes and timers: unknown long running processes, cron jobs and
  systemd timers that genco didn't write; processes of the groups' users
  that aren't their PHP or app (miners, shells).
- Sites: files changed in the last week outside of deploys in the
  groups' web roots, PHP files in upload folders, world writable files.
- Packages: anything installed that didn't come from Debian, genco's
  PHP repository or genco itself (updates are the update round's).
- genco: the admins of the panel (`genco user list`) and their API
  tokens.

End with a line `STATUS: OK` if nothing looks wrong, or
`STATUS: ATTENTION` with what an admin should look at. ATTENTION notifies
the admins.
