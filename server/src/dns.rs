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

/// `dig +short`, without a shell; nothing for an odd name. `kind` is a
/// record type or `-x` for the reverse lookup of an address.
pub async fn dig(kind: &str, name: &str) -> Vec<String> {
	if !plain_name(name) {
		return vec![];
	}
	let Ok(out) = Command::new("dig")
		.args(["+short", "+time=3", "+tries=2", kind, name])
		.output()
		.await
	else {
		return vec![];
	};
	String::from_utf8_lossy(&out.stdout)
		.lines()
		.map(|l| l.trim().trim_end_matches('.').to_string())
		.filter(|l| !l.is_empty())
		.collect()
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
	fn txt_pieces_are_joined() {
		assert_eq!(
			txt(vec!["\"v=spf1 mx \" \"~all\"".into()]),
			["v=spf1 mx ~all"]
		);
	}
}
