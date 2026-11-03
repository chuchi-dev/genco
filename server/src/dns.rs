//! DNS lookups and this server's own addresses, for the mail DNS helper,
//! the SPF check of senders, certificates and the usage stats.

use tokio::process::Command;

/// A name from the database or a DNS record, before it goes to `dig`.
pub fn plain_name(name: &str) -> bool {
	!name.is_empty()
		&& name.len() <= 253
		&& !name.starts_with(['-', '.'])
		&& name
			.chars()
			.all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c))
}

/// What the DNS says now, from the zone's own name servers: a resolver
/// keeps answers up to their TTL and a missing record for the zone's
/// negative TTL (often an hour), so a record just set would stay
/// "missing". Only an authoritative answer counts, a lame server of the
/// delegation (refuses or doesn't serve the zone) is skipped. Like `dig
/// +short`, without a shell; nothing for an odd name. `kind` is a record type or `-x` for the reverse lookup of an
/// address.
pub async fn dig(kind: &str, name: &str) -> Vec<String> {
	if !plain_name(name) {
		return vec![];
	}
	let (kind, name) = match kind {
		"-x" => match reverse_name(name) {
			Some(arpa) => ("PTR", arpa),
			None => return vec![],
		},
		k => (k, name.to_string()),
	};
	for ns in zone_servers(&name).await {
		if let Some(found) = ask_server(&ns, kind, &name).await {
			return found;
		}
	}
	// no name server answered authoritatively: what the resolver knows
	run_dig(&[kind, &name]).await.unwrap_or_default()
}

/// The records `ns` has for `name`, `None` unless it answered for the
/// zone itself.
async fn ask_server(ns: &str, kind: &str, name: &str) -> Option<Vec<String>> {
	let out = Command::new("dig")
		.args(["+noall", "+answer", "+comments", "+time=3", "+tries=2"])
		.args(["+norecurse", &format!("@{ns}"), kind, name])
		.output()
		.await
		.ok()?;
	if !out.status.success() {
		return None;
	}
	authoritative(&String::from_utf8_lossy(&out.stdout))
}

/// The record data of a `dig +noall +answer +comments` output if the
/// server is authoritative (`aa`) and found the records or knows there
/// are none (NOERROR, NXDOMAIN); `None` for REFUSED, SERVFAIL or an
/// answer from a cache.
fn authoritative(text: &str) -> Option<Vec<String>> {
	let status = text.split("status: ").nth(1)?.split(',').next()?;
	let flags = text.split(";; flags:").nth(1)?.split(';').next()?;
	if !["NOERROR", "NXDOMAIN"].contains(&status)
		|| !flags.split_whitespace().any(|f| f == "aa")
	{
		return None;
	}
	Some(
		text.lines()
			.filter(|l| !l.starts_with(';'))
			.filter_map(|l| {
				// name, TTL, class, type, then the data as `+short` shows it
				let mut rest = l.trim();
				for _ in 0..4 {
					rest = rest.split_once(char::is_whitespace)?.1.trim_start();
				}
				Some(rest.trim().trim_end_matches('.').to_string())
			})
			.filter(|l| !l.is_empty())
			.collect(),
	)
}

/// `dig +short` with `args`, `None` if it got no answer at all.
async fn run_dig(args: &[&str]) -> Option<Vec<String>> {
	let out = Command::new("dig")
		.args(["+short", "+time=3", "+tries=2"])
		.args(args)
		.output()
		.await
		.ok()?;
	let text = String::from_utf8_lossy(&out.stdout);
	// "no servers could be reached" comes on stdout too
	if !out.status.success() || text.contains(";;") {
		return None;
	}
	Some(
		text.lines()
			.map(|l| l.trim().trim_end_matches('.').to_string())
			.filter(|l| !l.is_empty())
			.collect(),
	)
}

/// The name servers of the zone `name` is in, from the resolver (their
/// names change rarely): the first of its parent names that has some.
async fn zone_servers(name: &str) -> Vec<String> {
	let mut zone = name;
	loop {
		let found: Vec<String> = run_dig(&["NS", zone])
			.await
			.unwrap_or_default()
			.into_iter()
			// a CNAME comes first, then its target's servers
			.filter(|n| plain_name(n) && n.contains('.'))
			.collect();
		if !found.is_empty() {
			return found;
		}
		match zone.split_once('.') {
			Some((_, parent)) if parent.contains('.') => zone = parent,
			_ => return vec![],
		}
	}
}

/// `7.3.0.203.in-addr.arpa` for `203.0.3.7`.
fn reverse_name(ip: &str) -> Option<String> {
	let ip: std::net::Ipv4Addr = ip.parse().ok()?;
	let o = ip.octets();
	Some(format!("{}.{}.{}.{}.in-addr.arpa", o[3], o[2], o[1], o[0]))
}

/// TXT records come quoted and maybe in pieces.
pub fn txt(found: Vec<String>) -> Vec<String> {
	found
		.iter()
		.map(|t| {
			t.split("\" \"")
				.collect::<String>()
				.trim_matches('"')
				.to_string()
		})
		.collect()
}

/// The TXT records of `name`, joined and unquoted.
pub async fn dig_txt(name: &str) -> Vec<String> {
	txt(dig("TXT", name).await)
}

/// The domain's addresses as its name servers say now, not as a resolver
/// still has them: a domain just moved here gets its certificate.
pub async fn resolve(domain: &str) -> Vec<String> {
	let mut ips = dig("A", domain).await;
	ips.extend(dig("AAAA", domain).await);
	// a CNAME comes first, keep the addresses
	ips.retain(|ip| ip.parse::<std::net::IpAddr>().is_ok());
	ips
}

/// This server's addresses (`hostname -I`), IPv4 first as listed.
pub async fn own_ips() -> Vec<String> {
	let Ok(out) = Command::new("hostname").arg("-I").output().await else {
		return vec![];
	};
	String::from_utf8_lossy(&out.stdout)
		.split_whitespace()
		.map(str::to_string)
		.collect()
}

/// This server's IPv4 addresses.
pub async fn own_ipv4() -> Vec<String> {
	own_ips()
		.await
		.into_iter()
		.filter(|ip| !ip.contains(':'))
		.collect()
}

/// This server's full name (`hostname -f`).
pub async fn fqdn() -> String {
	let out = Command::new("hostname").arg("-f").output().await;
	out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
		.unwrap_or_default()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn names_from_dns_stay_plain() {
		assert!(plain_name("_spf.google.com"));
		assert!(plain_name("203.0.113.7"));
		assert!(!plain_name("x;rm -rf /"));
		assert!(!plain_name("-x"));
	}

	#[test]
	fn reverse_names() {
		assert_eq!(
			reverse_name("203.0.3.7").as_deref(),
			Some("7.3.0.203.in-addr.arpa")
		);
		assert_eq!(reverse_name("2001:db8::1"), None);
	}

	const HEAD: &str =
		";; Got answer:\n;; ->>HEADER<<- opcode: QUERY, status: ";

	#[test]
	fn only_authoritative_answers_count() {
		let found = format!(
			"{HEAD}NOERROR, id: 1\n;; flags: qr aa; QUERY: 1, ANSWER: 2\n\n\
			;; ANSWER SECTION:\n\
			www.example.ch.\t300\tIN\tCNAME\texample.ch.\n\
			example.ch.\t300\tIN\tA\t203.0.113.7\n\n"
		);
		assert_eq!(
			authoritative(&found).unwrap(),
			["example.ch", "203.0.113.7"]
		);
		let quoted = format!(
			"{HEAD}NOERROR, id: 1\n;; flags: qr aa; QUERY: 1, ANSWER: 1\n\
			;; ANSWER SECTION:\n\
			example.ch.\t300\tIN\tTXT\t\"v=spf1  mx \" \"~all\"\n"
		);
		assert_eq!(txt(authoritative(&quoted).unwrap()), ["v=spf1  mx ~all"]);
		// an authoritative "none": no such name, or no such record yet
		let none = format!(
			"{HEAD}NXDOMAIN, id: 1\n;; flags: qr aa; QUERY: 1, ANSWER: 0\n"
		);
		assert_eq!(authoritative(&none), Some(vec![]));
		let empty = format!(
			"{HEAD}NOERROR, id: 1\n;; flags: qr aa; QUERY: 1, ANSWER: 0\n\n\
			;; OPT PSEUDOSECTION:\n; EDNS: version: 0, flags:; udp: 4096\n"
		);
		assert_eq!(authoritative(&empty), Some(vec![]));
		// a lame server of the delegation
		let refused = format!(
			"{HEAD}REFUSED, id: 1\n;; flags: qr; QUERY: 1, ANSWER: 0\n\
			; EDE: 20 (Not Authoritative)\n"
		);
		assert_eq!(authoritative(&refused), None);
		let servfail =
			format!("{HEAD}SERVFAIL, id: 1\n;; flags: qr aa; QUERY: 1\n");
		assert_eq!(authoritative(&servfail), None);
		let cached = format!(
			"{HEAD}NOERROR, id: 1\n;; flags: qr rd ra; QUERY: 1, ANSWER: 1\n\
			;; ANSWER SECTION:\nexample.ch.\t300\tIN\tA\t203.0.113.7\n"
		);
		assert_eq!(authoritative(&cached), None);
		assert_eq!(authoritative(";; no servers could be reached\n"), None);
	}

	#[test]
	fn txt_pieces_are_joined() {
		assert_eq!(
			txt(vec!["\"v=spf1 mx \" \"~all\"".into()]),
			["v=spf1 mx ~all"]
		);
	}
}
