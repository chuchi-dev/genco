//! Which domains this server may send mail for: an app needs a sender
//! (MAIL_FROM, a Craft system email) whose SPF record lets this server's
//! IP send, else receivers file its mails as spam or refuse them. The
//! check follows ip4, a, mx, include and redirect like a receiver does,
//! a few levels deep.

use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::Serialize;

use crate::config::Config;
use crate::db::Db;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sender {
	pub domain: String,
	/// "allowed", "not allowed", "no SPF"
	pub status: String,
	pub spf: Option<String>,
	pub note: String,
}

use crate::dns::dig;

async fn spf_record(domain: &str) -> Option<String> {
	crate::dns::dig_txt(domain)
		.await
		.into_iter()
		.find(|t| t == "v=spf1" || t.starts_with("v=spf1 "))
}

/// `ip` in `a.b.c.d` or `a.b.c.d/n`.
fn in_net(ip: Ipv4Addr, net: &str) -> bool {
	crate::firewall::Net::parse(net).is_ok_and(|n| n.contains(ip.into()))
}

async fn resolves_to(host: &str, ip: Ipv4Addr) -> bool {
	dig("A", host).await.iter().any(|a| a == &ip.to_string())
}

/// Whether the SPF of `domain` lets `ip` send: Some(true) or Some(false),
/// None without a record.
/// Lookups one check may make, like RFC 7208 (10 mechanisms that query
/// DNS): whoever runs a hosted domain's DNS can't make it run for hours.
const LOOKUPS: usize = 10;

fn allows<'a>(
	domain: &'a str,
	ip: Ipv4Addr,
	depth: u8,
	budget: &'a AtomicUsize,
) -> futures::future::BoxFuture<'a, Option<bool>> {
	Box::pin(async move {
		if depth > 5 {
			return Some(false);
		}
		let spf = spf_record(domain).await?;
		for term in spf.split_whitespace().skip(1) {
			let (qualifier, mech) = match term.chars().next() {
				Some(q @ ('+' | '-' | '~' | '?')) => (q, &term[1..]),
				_ => ('+', term),
			};
			let (name, arg) = match mech.split_once([':', '=']) {
				Some((n, a)) => (n.to_ascii_lowercase(), Some(a)),
				None => (mech.to_ascii_lowercase(), None),
			};
			let queries =
				matches!(name.as_str(), "a" | "mx" | "include" | "redirect");
			if queries && budget.fetch_add(1, Ordering::Relaxed) >= LOOKUPS {
				// a receiver gives up (permerror) as well
				return Some(false);
			}
			let hit = match (name.as_str(), arg) {
				("ip4", Some(net)) => in_net(ip, net),
				("a", host) => resolves_to(host.unwrap_or(domain), ip).await,
				("mx", host) => {
					let mut hit = false;
					let mxs = dig("MX", host.unwrap_or(domain)).await;
					for mx in mxs.iter().take(LOOKUPS) {
						let host = mx.split_whitespace().last().unwrap_or("");
						if resolves_to(host, ip).await {
							hit = true;
							break;
						}
					}
					hit
				}
				("include", Some(other)) => {
					allows(other, ip, depth + 1, budget).await == Some(true)
				}
				("redirect", Some(other)) => {
					return allows(other, ip, depth + 1, budget).await;
				}
				("all", None) => true,
				_ => false,
			};
			if hit {
				return Some(qualifier == '+');
			}
		}
		Some(false)
	})
}

/// Whether the SPF of `domain` lets `ip` send, None without a record.
pub async fn spf_allows(domain: &str, ip: Ipv4Addr) -> Option<bool> {
	allows(domain, ip, 0, &AtomicUsize::new(0)).await
}

/// The domains an app here might send as: the panel's, the mail domains,
/// every site domain and its parent (`shop.example.ch`, `example.ch`).
fn candidates(cfg: &Config, db: &Db) -> Vec<String> {
	let mut names = vec![];
	if let Some(d) = &cfg.domain {
		names.push(d.clone());
	}
	let groups = crate::hosting::groups(&db.conn()).unwrap_or_default();
	for g in &groups {
		names.extend(g.mail_domains.iter().map(|m| m.name.clone()));
		for d in &g.domains {
			let name = d.name.trim_start_matches("www.").to_string();
			if name.matches('.').count() >= 2
				&& let Some((_, parent)) = name.split_once('.')
			{
				names.push(parent.to_string());
			}
			names.push(name);
		}
	}
	names.sort();
	names.dedup();
	names
}

async fn check(cfg: &Config, db: &Db) -> (Option<Ipv4Addr>, Vec<Sender>) {
	let ip = crate::dns::own_ipv4()
		.await
		.iter()
		.find_map(|a| a.parse::<Ipv4Addr>().ok());
	let relay = crate::mail::settings(db)
		.map(|s| s.mode == "relay")
		.unwrap_or(false);
	let Some(ip) = ip else {
		return (None, vec![]);
	};
	let mail_domains: Vec<String> = crate::hosting::groups(&db.conn())
		.unwrap_or_default()
		.iter()
		.flat_map(|g| g.mail_domains.iter().map(|m| m.name.clone()))
		.collect();
	let mut out = vec![];
	for domain in candidates(cfg, db) {
		let spf = spf_record(&domain).await;
		let allowed = spf_allows(&domain, ip).await;
		let (status, mut note) = match allowed {
			Some(true) => ("allowed", "SPF lets this server send".to_string()),
			Some(false) => (
				"not allowed",
				format!("its SPF doesn't list {ip}, add ip4:{ip} (or mx/a)"),
			),
			None => ("no SPF", "receivers may take it as spam".to_string()),
		};
		if mail_domains.contains(&domain) {
			note.push_str(
				"; a mail domain here, DKIM signed when sent with a login",
			);
		}
		if relay {
			note.push_str("; mail goes through the relay, its SPF counts");
		}
		out.push(Sender {
			domain,
			status: status.into(),
			spf,
			note,
		});
	}
	// what an app can use first
	out.sort_by_key(|s| s.status != "allowed");
	(Some(ip), out)
}

/// `genco mail senders`
pub async fn cli(cfg: &Config, db: &Db) -> String {
	let (ip, senders) = check(cfg, db).await;
	let Some(ip) = ip else {
		return "this server has no IPv4 address".into();
	};
	let mut out = format!(
		"Apps send through postfix here (localhost:25, no login) from {ip}.\n\
		Use a sender at an allowed domain:\n"
	);
	for s in senders {
		out.push_str(&format!("{:12} {}  {}\n", s.status, s.domain, s.note));
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn networks() {
		let ip: Ipv4Addr = "203.0.113.7".parse().unwrap();
		assert!(in_net(ip, "203.0.113.7"));
		assert!(in_net(ip, "203.0.113.0/24"));
		assert!(!in_net(ip, "203.0.114.0/24"));
		assert!(in_net(ip, "0.0.0.0/0"));
		assert!(!in_net(ip, "203.0.113.0/40"));
	}
}
