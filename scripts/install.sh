#!/bin/bash
# Installs or updates genco on a Debian 13 server, safe to run again:
#
#   curl -fsSL https://github.com/chuchi-dev/genco/releases/latest/download/install.sh | bash
#
# It asks for what it needs, or takes it from the environment:
#   GENCO_DOMAIN  the panel's domain, it has to point to this server
#   GENCO_EMAIL   for Let's Encrypt
#   GENCO_ANTHROPIC_KEY or GENCO_OPENROUTER_KEY  the agent's key (one of
#                 them; the other empty), Claude Opus 5.5
#   GENCO_BINARY  a local genco binary instead of the latest release
set -euo pipefail

RELEASE=https://github.com/chuchi-dev/genco/releases/latest/download

# piped into bash, the questions read from the terminal
ask() {
	local var=$1 prompt=$2 secret=${3:-}
	# set, even empty (GENCO_ANTHROPIC_KEY= goes on to OpenRouter)
	[ -n "${!var+x}" ] && return
	# without a terminal /dev/tty exists but doesn't open
	{ true </dev/tty; } 2>/dev/null || { echo "set $var" >&2; exit 1; }
	if [ -n "$secret" ]; then
		read -rsp "$prompt: " "$var" </dev/tty
		echo
	else
		read -rp "$prompt: " "$var" </dev/tty
	fi
}

# the release checked against its signature in a directory of our own,
# only then it goes into place
binary() {
	if [ -n "${GENCO_BINARY:-}" ]; then
		install -m 755 "$GENCO_BINARY" /usr/local/bin/genco.new
		mv /usr/local/bin/genco.new /usr/local/bin/genco
		return
	fi
	command -v ssh-keygen >/dev/null || {
		echo "==> installing ssh-keygen to check the release"
		apt-get -o DPkg::Lock::Timeout=300 install -y -qq openssh-client >/dev/null
	}
	echo "==> downloading genco"
	tmp=$(mktemp -d)
	trap 'rm -rf "$tmp"' EXIT
	curl -fsSL -o "$tmp/genco" "$RELEASE/genco-x86_64-linux"
	curl -fsSL -o "$tmp/genco.sig" "$RELEASE/genco-x86_64-linux.sig"
	# the same key as server/src/update/allowed_signers
	echo 'release@genco namespaces="genco-release" ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAILUztNrddVcIx0sY84R+p9eROyBS4/5iizoU+RQd6qIb' >"$tmp/signers"
	ssh-keygen -Y verify -f "$tmp/signers" -I release@genco -n genco-release \
		-s "$tmp/genco.sig" <"$tmp/genco" >/dev/null ||
		{ echo "the release's signature doesn't match, nothing changed" >&2; exit 1; }
	install -m 755 "$tmp/genco" /usr/local/bin/genco.new
	mv /usr/local/bin/genco.new /usr/local/bin/genco
}

# all of it in a function: a download cut off in the middle runs nothing
main() {
	[ "$(id -u)" = 0 ] || { echo "run as root" >&2; exit 1; }
	binary

	mkdir -p /etc/genco /var/lib/genco
	chmod 755 /etc/genco
	chmod 700 /var/lib/genco
	[ -f /etc/genco/config.toml ] || install -m 600 /dev/null /etc/genco/config.toml

	if ! grep -q '^domain' /etc/genco/config.toml; then
		ask GENCO_DOMAIN "Domain of the panel (pointing to this server)"
		ask GENCO_EMAIL "Your email (for Let's Encrypt)"
	fi
	# the agent (Claude Opus 5.5) needs one of the two keys
	if ! grep -qE '^(anthropic|openrouter)-key' /etc/genco/config.toml; then
		echo "==> the agent's key: Anthropic's, or else OpenRouter's"
		ask GENCO_ANTHROPIC_KEY "Anthropic API key (empty for OpenRouter)" secret
		key=anthropic-key value=$GENCO_ANTHROPIC_KEY
		if [ -z "$value" ]; then
			ask GENCO_OPENROUTER_KEY "OpenRouter API key" secret
			key=openrouter-key value=$GENCO_OPENROUTER_KEY
		fi
		[ -n "$value" ] ||
			{ echo "the agent needs an Anthropic or an OpenRouter key" >&2; exit 1; }
		# it goes into TOML between quotes
		[[ "$value" =~ ^[A-Za-z0-9_-]+$ ]] ||
			{ echo "that doesn't look like an API key" >&2; exit 1; }
		echo "$key = \"$value\"" >>/etc/genco/config.toml
	fi

	genco setup ${GENCO_DOMAIN:+--domain "$GENCO_DOMAIN"} \
		${GENCO_EMAIL:+--email "$GENCO_EMAIL"}

	users=$(genco user list)
	if [ -z "$users" ]; then
		echo "==> the first admin"
		ask GENCO_ADMIN_EMAIL "Email"
		ask GENCO_ADMIN_NAME "Name"
		ask GENCO_ADMIN_PASSWORD "Password (10 characters or more)" secret
		genco user add "$GENCO_ADMIN_EMAIL" --name "$GENCO_ADMIN_NAME" \
			<<<"$GENCO_ADMIN_PASSWORD"
	fi

	# the agent welcomes the admins when it starts the first time
	systemctl restart genco
	domain=$(grep '^domain' /etc/genco/config.toml | cut -d'"' -f2)
	echo
	echo "genco runs on https://$domain"
	echo "Keep a copy of /etc/genco/restic-password, the backups need it."
}

main "$@"
