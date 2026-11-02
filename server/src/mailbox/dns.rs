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

/// The records `domain` needs for mail on this server, next to what its
/// DNS has now.
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

pub(super) async fn dns_check(
	cfg: &Config,
	domain: &str,
) -> Result<Vec<DnsRecord>> {
	let host =
		host(cfg).ok_or(Error::Invalid("the panel has no domain".into()))?;
	let ips = crate::dns::own_ipv4().await;
	let ip = ips.first().cloned().unwrap_or_default();
	let mut records = vec![];

	let found = crate::dns::dig("A", &host).await;
	let ok = found.iter().any(|f| ips.contains(f));
	records.push(record(
		&host,
		"A",
		ip.clone(),
		found,
		ok,
		"the mail server itself, once for all mail domains",
	));

	let found = spf(&host).await;
	let ok = spf_has(&found, &["a".into(), format!("ip4:{ip}")]);
	records.push(record(
		&host,
		"TXT",
		"v=spf1 a ~all".into(),
		found,
		ok,
		"the mail host sends as itself (forwarded mail, bounces), once for \
		all mail domains",
	));

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

	let found = spf(domain).await;
	let ok = spf_has(
		&found,
		&["mx".into(), format!("a:{host}"), format!("ip4:{ip}")],
	);
	let note = if found.is_empty() {
		"who may send mail as the domain"
	} else {
		"who may send mail as the domain: keep what is in there for other \
		senders and add mx (only one SPF record per domain)"
	};
	records.push(record(
		domain,
		"TXT",
		"v=spf1 mx ~all".into(),
		found,
		ok,
		note,
	));

	if let Some(value) = crate::mailfilter::dkim_record(domain).await {
		let name =
			format!("{}._domainkey.{domain}", crate::mailfilter::SELECTOR);
		let found = crate::dns::dig_txt(&name).await;
		let ok = found.iter().any(|f| dkim_key(f) == dkim_key(&value));
		records.push(record(
			&name,
			"TXT",
			value,
			found,
			ok,
			"the key receivers check the signature of the mail with",
		));
	}

	let dmarc = format!("_dmarc.{domain}");
	let found: Vec<String> = crate::dns::dig_txt(&dmarc)
		.await
		.into_iter()
		.filter(|t| t.starts_with("v=DMARC1"))
		.collect();
	let ok = !found.is_empty();
	records.push(record(
		&dmarc,
		"TXT",
		format!("v=DMARC1; p=quarantine; rua=mailto:postmaster@{domain}"),
		found,
		ok,
		"what receivers do with mail that fails SPF and DKIM",
	));

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

	let found = crate::dns::dig("-x", &ip).await;
	let ok = found.iter().any(|f| f == &host);
	records.push(record(
		&ip,
		"PTR",
		host.clone(),
		found,
		ok,
		"the reverse DNS of the server's IP, only the hoster can set it; \
		without it many receivers take mail for spam",
	));

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
