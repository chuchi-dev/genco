mod actions;
mod activity;
mod agent;
mod appenv;
mod apply;
mod approvals;
mod attachments;
mod backup;
mod chat;
mod checks;
mod cli;
mod commands;
mod config;
mod conversations;
mod costs;
mod db;
mod deploy;
mod dns;
mod dumps;
mod engines;
mod error;
mod events;
mod files;
mod firewall;
mod host;
mod hosting;
mod imports;
mod job;
mod limits;
mod llm;
mod mail;
mod mailbox;
mod mailfilter;
mod memory;
mod notify;
mod panel;
mod peers;
mod pg;
mod procs;
mod proposals;
mod quarantine;
mod render;
mod routines;
mod runner;
mod runners;
mod secrets;
mod senders;
mod serve;
mod setup;
mod signals;
mod stats;
mod status;
mod tokens;
mod update;
mod users;
mod util;
mod webmail;
mod webui;

use std::io::IsTerminal as _;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::FromRef;
use clap::Parser;

use crate::agent::Runner;
use crate::config::Config;
use crate::db::Db;
use crate::events::Events;
use commands::*;
use serve::serve;

#[derive(Debug, Parser)]
#[command(version, about = "Hosting panel with a maintenance agent")]
struct Args {
	#[clap(subcommand)]
	cmd: Cmd,
	#[clap(long, default_value = "/etc/genco/config.toml")]
	config: PathBuf,
	#[clap(long, default_value = "genco=info,tower_http=info,warn")]
	tracing: String,
}

#[derive(Debug, Parser)]
pub(crate) enum Cmd {
	/// Run the panel and the agent
	Serve,
	/// Talk to the agent in the terminal, works without the server
	Chat {
		/// continue this conversation
		#[clap(long)]
		resume: Option<i64>,
	},
	/// Look at the server, harden it and install what genco needs, then
	/// apply. Safe to run again.
	Setup {
		/// the panel's domain
		#[clap(long)]
		domain: Option<String>,
		/// for Let's Encrypt
		#[clap(long)]
		email: Option<String>,
	},
	/// Write the config of all groups and domains, start their services,
	/// create databases and get certificates
	Apply,
	/// Deploy a group from its git repository, prints the log
	Deploy {
		group: String,
		/// branch, tag or commit, the group's branch by default
		#[clap(long = "ref")]
		git_ref: Option<String>,
		/// a release built elsewhere (a tarball of its files) instead of
		/// the repository; --ref names it
		#[clap(long)]
		artifact: Option<PathBuf>,
	},
	/// Back up every group (files and databases) and the server's config;
	/// list and restore snapshots, the copy off the server
	Backup {
		#[clap(subcommand)]
		cmd: Option<backup::BackupCmd>,
	},
	/// Run restic with genco's repository and password, e.g.
	/// `genco restic snapshots --tag group:shop`; `--remote` first for
	/// the copy off the server
	Restic {
		/// the remote repository (`genco backup remote`)
		#[clap(long)]
		remote: bool,
		#[clap(trailing_var_arg = true, allow_hyphen_values = true)]
		args: Vec<String>,
	},
	/// Print clients, groups, domains and databases as JSON
	Tree,
	/// Print genco's documentation: `readme` (what it does, install) or
	/// `operations` (runbook, notifications, the agent's key and model)
	Docs { topic: Option<String> },
	/// Manage clients
	#[clap(subcommand)]
	Client(cli::ClientCmd),
	/// Manage groups
	#[clap(subcommand)]
	Group(cli::GroupCmd),
	/// Manage domains
	#[clap(subcommand)]
	Domain(cli::DomainCmd),
	/// Manage databases
	#[clap(subcommand)]
	Database(cli::DatabaseCmd),
	/// Commands of a group that run all the time (a queue, a server on
	/// its own port)
	#[clap(subcommand)]
	Worker(cli::WorkerCmd),
	/// Commands of a group that run at times, like cron
	#[clap(subcommand)]
	Schedule(cli::ScheduleCmd),
	/// Manage the admins of the panel
	#[clap(subcommand)]
	User(UserCmd),
	/// Mail domains, mailboxes, forwards and their DNS
	#[clap(subcommand)]
	Mail(mailbox::MailCmd),
	/// Passwords the admins see in the panel (Start → Passwords)
	#[clap(subcommand)]
	Secret(secrets::SecretCmd),
	/// Block addresses from the sites, mail and workers (never ssh)
	#[clap(subcommand)]
	Firewall(firewall::FirewallCmd),
	/// How the RAM is split between the groups and the system; change the
	/// reserve for the system
	Memory {
		#[clap(subcommand)]
		cmd: Option<memory::MemoryCmd>,
	},
	/// The routines and alerts that still need attention: list them,
	/// resolve one that is taken care of
	#[clap(subcommand)]
	Attention(conversations::AttentionCmd),
	/// Print the activity log, newest last
	Activity {
		#[clap(long, default_value_t = 50)]
		limit: i64,
	},
	/// The panel's own name: rename it, forget an old one
	#[clap(subcommand)]
	Panel(panel::PanelCmd),
	/// Build runners for steps with `on = "runner"` (this server's side)
	#[clap(subcommand)]
	Runners(RunnersCmd),
	/// Be a build runner on this box, see `genco runner install`
	#[clap(subcommand)]
	Runner(runner::RunnerCmd),
	/// Install the newest release of genco (checked against its
	/// signature, back to the old one if it doesn't come up)
	Update {
		/// only tell what is installed and what is new
		#[clap(long)]
		check: bool,
		/// install it, without this it only checks
		#[clap(long)]
		yes: bool,
		/// this release instead of the newest, also an older one
		#[clap(long)]
		version: Option<String>,
		/// a local build instead of a release (unsigned, for test servers)
		#[clap(long, conflicts_with = "version")]
		file: Option<PathBuf>,
		/// release, or testing: the server never updates itself and gets
		/// its builds with --file
		#[clap(long, value_parser = ["release", "testing"])]
		mode: Option<String>,
	},
}

impl Cmd {
	/// Servers, interactive and system-changing jobs, which write to
	/// sockets and child processes.
	fn long_running(&self) -> bool {
		matches!(
			self,
			Cmd::Serve
				| Cmd::Chat { .. }
				| Cmd::Runner(_)
				| Cmd::Setup { .. }
				| Cmd::Apply | Cmd::Deploy { .. }
				| Cmd::Backup { .. }
				| Cmd::Memory { cmd: Some(_) }
				| Cmd::Restic { .. }
				| Cmd::Update { .. }
				| Cmd::Panel(_)
		)
	}

	/// Whether it may change something, for the activity log; reading and
	/// long-running commands stay out (an update logs itself).
	fn changes(&self) -> bool {
		!matches!(
			self,
			Cmd::Serve
				| Cmd::Chat { .. }
				| Cmd::Tree | Cmd::Docs { .. }
				| Cmd::User(UserCmd::List)
				| Cmd::Worker(cli::WorkerCmd::List)
				| Cmd::Schedule(cli::ScheduleCmd::List)
				| Cmd::Mail(
					mailbox::MailCmd::List
						| mailbox::MailCmd::Dns { .. }
						| mailbox::MailCmd::Senders
						| mailbox::MailCmd::Stats { .. }
						| mailbox::MailCmd::SendOnly(
							mailbox::SendOnlyCmd::List
						)
				) | Cmd::Secret(secrets::SecretCmd::List)
				| Cmd::Firewall(firewall::FirewallCmd::List)
				| Cmd::Activity { .. }
				| Cmd::Attention(conversations::AttentionCmd::List)
				| Cmd::Memory { cmd: None }
				| Cmd::Update { .. }
				| Cmd::Runners(RunnersCmd::List)
				| Cmd::Group(cli::GroupCmd::DeployKey { .. })
				| Cmd::Backup {
					cmd: Some(
						backup::BackupCmd::Snapshots { .. }
							| backup::BackupCmd::Remote(
								backup::remote::RemoteCmd::Show
							)
					)
				}
		)
	}
}

#[derive(Debug, Parser)]
enum RunnersCmd {
	/// The runners, whether they are online, what builds have to match
	List,
	/// A new runner, prints the token for `genco runner register`
	Add { name: String },
	/// Remove a runner, its token stops working
	Remove { name: String },
}

#[derive(Debug, Parser)]
enum UserCmd {
	/// Add an admin, the password is read from stdin
	Add {
		email: String,
		#[clap(long)]
		name: String,
	},
	/// Set the password of an admin, read from stdin
	Password { email: String },
	/// Turn off the two-factor login of an admin who lost the authenticator
	TotpOff { email: String },
	/// List the admins
	List,
}

#[derive(Clone, FromRef)]
pub struct AppState {
	cfg: Arc<Config>,
	db: Db,
	events: Events,
	runner: Runner,
	stats: stats::StatsDb,
}

#[tokio::main]
async fn main() {
	let args = Args::parse();
	// a command piped into `head` ends quietly instead of panicking on
	// the closed stdout; long jobs keep getting EPIPE as an error, their
	// sockets and child pipes must not kill them
	if !args.cmd.long_running() {
		unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
	}

	// plain text when the agent runs a command
	tracing_subscriber::fmt()
		.with_env_filter(args.tracing.as_str())
		.with_ansi(std::io::stderr().is_terminal())
		.init();

	if let Err(e) = run(args).await {
		eprintln!("error: {e}");
		std::process::exit(1);
	}
}

async fn run(args: Args) -> Result<(), String> {
	if let Cmd::Setup { domain, email } = args.cmd {
		setup::main(&args.config, domain, email).await?;
		let cfg = Config::load(&args.config)?;
		log_cli(&cfg);
		return apply_cmd(cfg).await;
	}
	// a build box has no genco config
	if let Cmd::Runner(cmd) = args.cmd {
		return runner::main(cmd).await;
	}
	if let Cmd::Docs { topic } = &args.cmd {
		print!("{}", commands::docs(topic.as_deref())?);
		return Ok(());
	}
	let cfg = Config::load(&args.config)?;
	if args.cmd.changes() {
		log_cli(&cfg);
	}

	// deleting waits for an admin's confirmation by mail
	if cli::is_deletion(&args.cmd) {
		let db = Db::open(&cfg.db_path())?;
		let (kind, id) =
			cli::deletion(&db.conn(), &args.cmd).map_err(|e| e.to_string())?;
		let who = std::env::var("GENCO_AGENT")
			.map(|_| "genco's agent")
			.unwrap_or("the terminal");
		let req = actions::request(&cfg, &db, kind, id, who)
			.await
			.map_err(|e| e.to_string())?;
		println!("{}", req.message);
		return Ok(());
	}

	match args.cmd {
		Cmd::Setup { .. } => unreachable!(),
		Cmd::Apply => apply_cmd(cfg).await,
		Cmd::Serve => serve(cfg).await,
		Cmd::Chat { resume } => chat::main(cfg, resume).await,
		Cmd::Deploy {
			group,
			git_ref,
			artifact,
		} => deploy_cmd(cfg, group, git_ref, artifact).await,
		Cmd::Runners(cmd) => runners_cmd(&cfg, cmd).await,
		Cmd::Panel(cmd) => panel::main(&args.config, cfg, cmd).await,
		Cmd::Runner(_) => unreachable!(),
		Cmd::Backup { cmd: None } => backup_cmd(cfg).await,
		Cmd::Backup { cmd: Some(cmd) } => {
			let db = Db::open(&cfg.db_path())?;
			let actor = activity::cli_actor();
			let msg = backup::cli(&cfg, &db, cmd, read_password, actor)
				.await
				.map_err(|e| e.to_string())?;
			println!("{msg}");
			Ok(())
		}
		Cmd::Restic { remote, args } => {
			let mut cmd = if remote {
				let db = Db::open(&cfg.db_path())?;
				backup::remote::required(&db)
					.map_err(|e| e.to_string())?
					.restic()
			} else {
				backup::restic()
			};
			let status = cmd
				.args(args)
				.stdin(std::process::Stdio::inherit())
				.status()
				.await
				.map_err(|e| format!("restic: {e}"))?;
			std::process::exit(status.code().unwrap_or(1));
		}
		Cmd::Tree => {
			let db = Db::open(&cfg.db_path())?;
			let tree = hosting::tree(&db.conn()).map_err(|e| e.to_string())?;
			println!("{}", serde_json::to_string_pretty(&tree).unwrap());
			Ok(())
		}
		Cmd::Docs { .. } => unreachable!("read before the config"),
		Cmd::User(cmd) => user(cfg, cmd),
		Cmd::Memory { cmd: None } => {
			let db = Db::open(&cfg.db_path())?;
			let s = memory::status(&db.conn()).map_err(|e| e.to_string())?;
			println!("{}", s.text());
			Ok(())
		}
		Cmd::Memory {
			cmd: Some(memory::MemoryCmd::Reserve { size }),
		} => {
			let reserve =
				limits::parse_size(&size).map_err(|e| e.to_string())?;
			let db = Db::open(&cfg.db_path())?;
			memory::set_reserve(&db.conn(), reserve)
				.map_err(|e| e.to_string())?;
			apply_cmd(cfg).await
		}
		Cmd::Attention(cmd) => {
			edit(cfg, |_, conn| conversations::attention(conn, cmd))
		}
		Cmd::Activity { limit } => {
			let db = Db::open(&cfg.db_path())?;
			let entries = activity::list(&db.conn(), None, limit)
				.map_err(|e| e.to_string())?;
			for e in entries.iter().rev() {
				let at = util::format_time(e.at).await;
				let ok = match e.ok {
					Some(false) => " FAILED",
					_ => "",
				};
				println!(
					"{at}  {}  {}{ok}  {}  {}",
					e.actor, e.action, e.target, e.detail
				);
			}
			Ok(())
		}
		Cmd::Update {
			check,
			yes,
			version,
			file,
			mode,
		} => {
			let args = update::Args {
				check,
				yes,
				version,
				file,
				mode,
			};
			update::main(&cfg, args).await
		}
		Cmd::Client(cmd) => edit(cfg, |_, conn| cli::client(conn, cmd)),
		// as the group's user, like the panel: the key could be a link
		Cmd::Group(cli::GroupCmd::DeployKey { name }) => {
			let db = Db::open(&cfg.db_path())?;
			let group = hosting::group_by_name(&db.conn(), &name)
				.map_err(|e| e.to_string())?
				.ok_or(format!("no group {name}"))?;
			let key = deploy::deploy_key(&group).await.ok_or(
				"no deploy key yet: set a repository and run genco apply",
			)?;
			println!("{key}");
			Ok(())
		}
		Cmd::Group(cli::GroupCmd::AdoptLimits { name }) => {
			let db = Db::open(&cfg.db_path())?;
			let msg = cli::adopt_limits(&db, &name)
				.await
				.map_err(|e| e.to_string())?;
			println!("{msg}");
			apply_cmd(cfg).await
		}
		Cmd::Group(cli::GroupCmd::Quarantine { name, reason }) => {
			quarantine_cmd(cfg, &name, Some(&reason)).await
		}
		Cmd::Group(cli::GroupCmd::Unquarantine { name }) => {
			quarantine_cmd(cfg, &name, None).await
		}
		Cmd::Group(cmd) => edit(cfg, |_, conn| cli::group(conn, cmd)),
		Cmd::Domain(cmd) => edit(cfg, |cfg, conn| cli::domain(cfg, conn, cmd)),
		Cmd::Database(cli::DatabaseCmd::Import { name, file }) => {
			import_cmd(cfg, name, file).await
		}
		Cmd::Database(cmd) => {
			if let cli::DatabaseCmd::Add { name, postgres, .. } = &cmd {
				let db = Db::open(&cfg.db_path())?;
				let engine = cli::engine(*postgres);
				engines::check_database_free(&db, name, engine)
					.await
					.map_err(|e| e.to_string())?;
			}
			edit(cfg, |_, conn| cli::database(conn, cmd))
		}
		Cmd::Worker(cmd) => edit(cfg, |_, conn| cli::worker(conn, cmd)),
		Cmd::Schedule(cli::ScheduleCmd::Run { name, group }) => {
			let db = Db::open(&cfg.db_path())?;
			let g = hosting::group_by_name(&db.conn(), &group)
				.map_err(|e| e.to_string())?
				.ok_or(format!("no group {group}"))?;
			quarantine::refuse(&g)?;
			let unit = render::schedule_unit(&group, &name);
			procs::start_now(&unit).await.map_err(|e| e.to_string())?;
			println!("started {unit}, see journalctl -u {unit}");
			Ok(())
		}
		Cmd::Schedule(cmd) => {
			let every = match &cmd {
				cli::ScheduleCmd::Add { every, .. } => Some(every.clone()),
				cli::ScheduleCmd::Set { every, .. } => every.clone(),
				_ => None,
			};
			if let Some(every) = every {
				procs::check_every(&every)
					.await
					.map_err(|e| e.to_string())?;
			}
			edit(cfg, |_, conn| cli::schedule(conn, cmd))
		}
		Cmd::Secret(cmd) => {
			edit(cfg, |_, conn| secrets::cli(conn, cmd, read_password))
		}
		Cmd::Firewall(cmd) => {
			let db = Db::open(&cfg.db_path())?;
			let msg = firewall::cli(&cfg, &db, cmd)
				.await
				.map_err(|e| e.to_string())?;
			println!("{msg}");
			Ok(())
		}
		Cmd::Mail(cmd) => {
			let db = Db::open(&cfg.db_path())?;
			let msg = mailbox::cli(&cfg, &db, cmd, read_password)
				.await
				.map_err(|e| e.to_string())?;
			println!("{msg}");
			Ok(())
		}
	}
}

/// Puts a group in quarantine with the reason, or lifts it without; then
/// applies.
async fn quarantine_cmd(
	cfg: Config,
	name: &str,
	reason: Option<&str>,
) -> Result<(), String> {
	let db = Db::open(&cfg.db_path())?;
	let group = hosting::group_by_name(&db.conn(), name)
		.map_err(|e| e.to_string())?
		.ok_or(format!("no group {name}"))?;
	quarantine::change(&db.conn(), group.id, reason, activity::cli_actor())
		.map_err(|e| e.to_string())?;
	match reason {
		Some(_) => println!("{name} is in quarantine"),
		None => println!("lifted the quarantine of {name}"),
	}
	apply_cmd(cfg).await
}

/// Every command that changes something goes into the activity log, as
/// it was typed.
fn log_cli(cfg: &Config) {
	let line = masked_args(std::env::args().skip(1).collect());
	if let Ok(db) = Db::open(&cfg.db_path()) {
		let detail = format!("genco {}", line.join(" "));
		activity::log(
			&db.conn(),
			activity::cli_actor(),
			"Command",
			"",
			&detail,
			None,
		);
	}
}

/// The command line for the activity log, values of `--secret` hidden.
fn masked_args(args: Vec<String>) -> Vec<String> {
	let hide = |a: &str| match a.split_once('=') {
		Some((name, _)) => format!("{name}=***"),
		None => "***".into(),
	};
	let mut out = Vec::with_capacity(args.len());
	let mut next_secret = false;
	for a in args {
		if next_secret {
			out.push(hide(&a));
			next_secret = false;
		} else if let Some(v) = a.strip_prefix("--secret=") {
			out.push(format!("--secret={}", hide(v)));
		} else {
			next_secret = a == "--secret";
			out.push(a);
		}
	}
	out
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn secrets_stay_out_of_the_activity_log() {
		let args = [
			"domain",
			"env",
			"a.ch",
			"--secret",
			"KEY=s3",
			"--secret=B=x",
			"--set",
			"M=1",
		];
		let line = masked_args(args.iter().map(|a| a.to_string()).collect());
		assert_eq!(
			line.join(" "),
			"domain env a.ch --secret KEY=*** --secret=B=*** --set M=1"
		);
	}
}
