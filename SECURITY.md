# Security

Please report vulnerabilities privately through GitHub: **Security →
Report a vulnerability** on this repository. Don't open a public issue.

Helpful in a report: what an attacker needs (a group's code, a token, an
admin login, nothing), what they get, and how to reproduce it.

genco's own threat model, roughly:

- a group's code (a hostile or hacked site) must not read other groups'
  data or reach root;
- the internet must not get past the panel's login, the mail services or
  the sites;
- tokens give only what their scope says;
- what sites, logs and mails contain is data for the agent, never
  instructions. This is the weakest line: the agent has a root shell.
