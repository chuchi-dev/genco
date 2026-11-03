//! The DNS records a mail domain needs, next to what its DNS has now.

use super::*;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DnsRecord {
	/// the full name, e.g. `_dmarc.example.ch`
	pub name: String,
	/// MX, TXT, A, SRV, PTR
	pub kind: String,
	/// what to enter
	pub value: String,
	/// what the DNS answers now
	pub found: Vec<String>,
	/// ok, missing, differs or optional
	pub status: String,
	pub note: String,
}

pub(super) fn record(
	name: &str,
	kind: &str,
	value: String,
	found: Vec<String>,
	ok: bool,
	note: &str,
) -> DnsRecord {
	let status = if ok {
		"ok"
	} else if found.is_empty() {
		"missing"
	} else {
		"differs"
	};
	DnsRecord {
		name: name.into(),
		kind: kind.into(),
		value,
		found,
		status: status.into(),
		note: note.into(),
	}
}

/// The SPF records of `name`.
pub(super) async fn spf(name: &str) -> Vec<String> {
	crate::dns::dig_txt(name)
		.await
		.into_iter()
		.filter(|t| t == "v=spf1" || t.starts_with("v=spf1 "))
		.collect()
}

/// Whether an SPF record has one of `terms` (with or without `+`).
pub(super) fn spf_has(found: &[String], terms: &[String]) -> bool {
	found.iter().any(|spf| {
		spf.split_whitespace()
			.map(|p| p.strip_prefix('+').unwrap_or(p))
			.any(|p| terms.iter().any(|t| t == p))
	})
}

/// The key of a DKIM record, without the spaces DNS tools put in.
pub(super) fn dkim_key(record: &str) -> Option<String> {
	record
		.split(';')
		.find_map(|p| p.trim().strip_prefix("p="))
		.map(|k| k.replace(' ', ""))
}

/// `name` points to this server.
async fn a_record(name: &str, ips: &[String], note: &str) -> DnsRecord {
	let ip = ips.first().cloned().unwrap_or_default();
	let found = crate::dns::dig("A", name).await;
	let ok = found.iter().any(|f| ips.contains(f));
	record(name, "A", ip, found, ok, note)
}

/// The SPF record of `name`, ok when it has one of `terms`.
async fn spf_record(
	name: &str,
	value: &str,
	terms: &[String],
	note: &str,
) -> DnsRecord {
	let found = spf(name).await;
	let ok = spf_has(&found, terms);
	let note = if found.is_empty() {
		note.to_string()
	} else {
		format!(
			"{note}: keep what is in there for other senders and add {} \
			(only one SPF record per name)",
			terms[0]
		)
	};
	record(name, "TXT", value.into(), found, ok, &note)
}

/// A DKIM key at `name`, compared by the key alone.
async fn dkim_txt(name: &str, value: String, note: &str) -> DnsRecord {
	let found = crate::dns::dig_txt(name).await;
	let ok = found.iter().any(|f| dkim_key(f) == dkim_key(&value));
	record(name, "TXT", value, found, ok, note)
}

/// Any DMARC record of `domain` will do.
async fn dmarc_record(domain: &str, value: String) -> DnsRecord {
	let name = format!("_dmarc.{domain}");
	let found: Vec<String> = crate::dns::dig_txt(&name)
		.await
		.into_iter()
		.filter(|t| t.starts_with("v=DMARC1"))
		.collect();
	let ok = !found.is_empty();
	record(
		&name,
		"TXT",
		value,
		found,
		ok,
		"what receivers do with mail that fails SPF and DKIM",
	)
}

/// The reverse DNS of the server's address says `name`, the one postfix
/// greets with.
async fn ptr_record(ip: &str, name: &str) -> DnsRecord {
	let found = crate::dns::dig("-x", ip).await;
	let ok = found.iter().any(|f| f == name);
	record(
		ip,
		"PTR",
		name.into(),
		found,
		ok,
		"the reverse DNS of the server's IP, only the hoster can set it; \
		without it many receivers take mail for spam",
	)
}

/// The DKIM key of a mail domain, once apply made it.
async fn dkim_check(domain: &str) -> Option<DnsRecord> {
	let value = crate::mailfilter::dkim_record(domain).await?;
	let name = format!("{}._domainkey.{domain}", crate::mailfilter::SELECTOR);
	Some(
		dkim_txt(
			&name,
			value,
			"the key receivers check the signature of the mail with",
		)
		.await,
	)
}

/// The SPF of a send-only domain: its inbox is elsewhere, so the record
/// there stays and only gets this server added. Through a relay the
/// relay's SPF counts, genco can't tell what that is.
async fn send_only_spf(
	domain: &str,
	ip: &str,
	relay: Option<&str>,
) -> DnsRecord {
	let found = spf(domain).await;
	if let Some(relay) = relay {
		let mut r = record(
			domain,
			"TXT",
			format!("the SPF entry of {relay}"),
			found,
			false,
			&format!(
				"mail goes out through the relay {relay}: add what its \
				docs say to the SPF record, not this server's IP"
			),
		);
		r.status = "optional".into();
		return r;
	}
	let allowed = match ip.parse() {
		Ok(ip) => crate::senders::spf_allows(domain, ip).await,
		Err(_) => None,
	};
	if found.is_empty() {
		return record(
			domain,
			"TXT",
			format!("v=spf1 ip4:{ip} ~all"),
			found,
			false,
			"who may send mail as the domain: this server and whoever \
			sends for the inbox elsewhere (their include:), in one record",
		);
	}
	record(
		domain,
		"TXT",
		format!("ip4:{ip}"),
		found,
		allowed == Some(true),
		&format!(
			"who may send mail as the domain: add ip4:{ip} to the SPF \
			record that is there and keep the rest (one SPF record per name)"
		),
	)
}

/// The records `domain` needs for mail on this server, next to what its
/// DNS has now. A send-only domain has no MX here, only what sending
/// needs; `relay`: mail goes out through that host.
pub(super) async fn dns_check(
	cfg: &Config,
	domain: &str,
	send_only: bool,
	relay: Option<&str>,
) -> Result<Vec<DnsRecord>> {
	let host =
		host(cfg).ok_or(Error::Invalid("the panel has no domain".into()))?;
	let ips = crate::dns::own_ipv4().await;
	let ip = ips.first().cloned().unwrap_or_default();
	let mut records = vec![
		a_record(
			&host,
			&ips,
			"the mail server itself, once for all mail domains",
		)
		.await,
		spf_record(
			&host,
			"v=spf1 a ~all",
			&["a".into(), format!("ip4:{ip}")],
			"the mail host sends as itself (forwarded mail, bounces), once \
			for all mail domains",
		)
		.await,
	];

	if send_only {
		records.push(send_only_spf(domain, &ip, relay).await);
		records.extend(dkim_check(domain).await);
		records.push(
			dmarc_record(
				domain,
				format!("v=DMARC1; p=none; rua=mailto:postmaster@{domain}"),
			)
			.await,
		);
		records.push(ptr_record(&ip, &host).await);
		return Ok(records);
	}

	let found = crate::dns::dig("MX", domain).await;
	let ok = found.iter().any(|f| f.ends_with(&host));
	records.push(record(
		domain,
		"MX",
		format!("10 {host}."),
		found,
		ok,
		"where other servers deliver mail for the domain",
	));
	records.push(
		spf_record(
			domain,
			"v=spf1 mx ~all",
			&["mx".into(), format!("a:{host}"), format!("ip4:{ip}")],
			"who may send mail as the domain",
		)
		.await,
	);
	records.extend(dkim_check(domain).await);
	records.push(
		dmarc_record(
			domain,
			format!("v=DMARC1; p=quarantine; rua=mailto:postmaster@{domain}"),
		)
		.await,
	);

	for (service, port, note) in [
		(
			"_imaps._tcp",
			993,
			"lets mail programs find IMAP by themselves",
		),
		(
			"_submissions._tcp",
			465,
			"lets mail programs find SMTP by themselves",
		),
	] {
		let name = format!("{service}.{domain}");
		let found = crate::dns::dig("SRV", &name).await;
		let ok = found.iter().any(|f| f.ends_with(&host));
		let mut r = record(
			&name,
			"SRV",
			format!("0 1 {port} {host}."),
			found,
			ok,
			note,
		);
		if !ok && r.found.is_empty() {
			r.status = "optional".into();
		}
		records.push(r);
	}

	records.push(ptr_record(&ip, &host).await);
	Ok(records)
}

/// The check of a mail domain by name, in its mode and the mail settings'
/// (a relay sends for it).
pub(super) async fn domain_dns(
	cfg: &Config,
	db: &Db,
	name: &str,
) -> Result<Vec<DnsRecord>> {
	let send_only: bool = db.conn().query_row(
		"SELECT send_only FROM mail_domains WHERE name = ?1",
		[name.trim().to_lowercase()],
		|r| r.get(0),
	)?;
	let settings = crate::mail::settings(db)?;
	let relay = (settings.mode == "relay").then_some(settings.relay_host);
	dns_check(cfg, name, send_only, relay.as_deref()).await
}

/// What genco's own mails need, sent as genco@<panel>: SPF, DKIM and
/// DMARC of the panel's domain and the PTR of the server's address.
pub async fn panel_dns(
	cfg: &Config,
	groups: &[Group],
) -> Result<Vec<DnsRecord>> {
	let panel = cfg
		.domain
		.clone()
		.ok_or(Error::Invalid("the panel has no domain".into()))?;
	let ips = crate::dns::own_ipv4().await;
	let ip = ips.first().cloned().unwrap_or_default();
	let mut records = vec![
		a_record(
			&panel,
			&ips,
			"the panel, genco sends its mails as genco@ this name",
		)
		.await,
		spf_record(
			&panel,
			"v=spf1 a ~all",
			&["a".into(), format!("ip4:{ip}")],
			"this server may send as the panel's name",
		)
		.await,
	];
	if let Some(value) = crate::mail::panel_dkim_record().await {
		let name =
			format!("{}._domainkey.{panel}", crate::mail::PANEL_SELECTOR);
		records.push(
			dkim_txt(&name, value, "the key genco signs its mails with").await,
		);
	}
	records.push(dmarc_record(&panel, "v=DMARC1; p=quarantine".into()).await);

	// postfix greets with this name, the PTR should say the same
	let helo = if enabled(groups) {
		host(cfg).unwrap_or_else(|| panel.clone())
	} else {
		panel.clone()
	};
	records.push(ptr_record(&ip, &helo).await);
	Ok(records)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn spf_and_dkim_records_are_compared_by_their_terms() {
		let found = vec!["v=spf1 include:_spf.x.ch +mx ~all".to_string()];
		assert!(spf_has(&found, &["mx".into()]));
		assert!(!spf_has(&found, &["a".into(), "ip4:1.2.3.4".into()]));
		assert_eq!(
			dkim_key("v=DKIM1; k=rsa; p=AB CD"),
			dkim_key("v=DKIM1;p=ABCD")
		);
		assert_eq!(dkim_key("v=DKIM1"), None);
	}
}
