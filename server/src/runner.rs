//! `genco runner`: the build side of runners.rs, on a build box. It asks
//! its genco servers for build jobs (HTTPS out, no open port), builds
//! each in a container with podman or docker, sends the log while it
//! runs and the artifacts at the end. Each server and step folder keeps
//! its source and caches (cargo, npm, `target/`) in its own directory, so
//! the next build only does what changed.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::{
	AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, BufReader,
};
use tokio::process::Command;
use tokio::sync::Semaphore;

use crate::runners::{Job, RunnerInfo};

const CONFIG: &str = "/etc/genco/runner.toml";
const UNIT: &str = "/etc/systemd/system/genco-runner.service";
const UPDATE_UNIT: &str = "/etc/systemd/system/genco-runner-update.service";
const UPDATE_TIMER: &str = "/etc/systemd/system/genco-runner-update.timer";

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct RunnerConfig {
	/// builds at the same time, over all servers
	#[serde(default = "one")]
	pub jobs: usize,
	/// sources and caches, `~/genco-builds` by default
	#[serde(default)]
	pub dir: Option<PathBuf>,
	#[serde(default, rename = "server")]
	pub servers: Vec<Server>,
}

fn one() -> usize {
	1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Server {
	/// `https://genco.example.ch`
	pub url: String,
	pub token: String,
}

#[derive(Debug, clap::Subcommand)]
pub enum RunnerCmd {
	/// Add a genco server: its URL and the runner token from its panel
	/// (Settings → Runners) or `genco runners add`
	Register {
		url: String,
		token: String,
		#[clap(long, default_value = CONFIG)]
		config: PathBuf,
	},
	/// Take build jobs until stopped (what genco-runner.service runs)
	Run {
		#[clap(long, default_value = CONFIG)]
		config: PathBuf,
	},
	/// Set up this box as a runner: podman, the user it builds as, the
	/// systemd service and a daily update
	Install {
		#[clap(long, default_value = "genco-runner")]
		user: String,
	},
	/// Install the newest signed release (or --version) and restart the
	/// runner; the daily timer of `install` runs it
	Update {
		#[clap(long)]
		version: Option<String>,
	},
}

pub async fn main(cmd: RunnerCmd) -> Result<(), String> {
	match cmd {
		RunnerCmd::Register { url, token, config } => {
			register(&config, url, token).await
		}
		RunnerCmd::Run { config } => run(&config).await,
		RunnerCmd::Install { user } => install(&user).await,
		RunnerCmd::Update { version } => update(version.as_deref()).await,
	}
}

fn load(path: &Path) -> Result<RunnerConfig, String> {
	match std::fs::read_to_string(path) {
		Ok(s) => {
			toml::from_str(&s).map_err(|e| format!("{}: {e}", path.display()))
		}
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
			Ok(RunnerConfig {
				jobs: 1,
				..Default::default()
			})
		}
		Err(e) => Err(format!("{}: {e}", path.display())),
	}
}

fn client() -> reqwest::Client {
	reqwest::Client::builder()
		.user_agent(format!("genco-runner/{}", crate::update::VERSION))
		.connect_timeout(Duration::from_secs(20))
		.build()
		.expect("http client")
}

fn api(server: &Server, path: &str) -> String {
	format!("{}/api/runner{path}", server.url.trim_end_matches('/'))
}

async fn register(
	path: &Path,
	url: String,
	token: String,
) -> Result<(), String> {
	let url = url.trim().trim_end_matches('/').to_string();
	if !url.starts_with("https://") {
		return Err("the server's URL starts with https://".into());
	}
	let server = Server {
		url,
		token: token.trim().into(),
	};
	let res = client()
		.get(api(&server, "/hello"))
		.bearer_auth(&server.token)
		.send()
		.await
		.map_err(|e| e.to_string())?;
	if !res.status().is_success() {
		return Err(format!(
			"{} refused the token ({})",
			server.url,
			res.status()
		));
	}
	let hello: serde_json::Value =
		res.json().await.map_err(|e| e.to_string())?;
	let mut cfg = load(path)?;
	cfg.servers.retain(|s| s.url != server.url);
	cfg.servers.push(server);
	write_config(path, &cfg)?;
	println!(
		"registered as {} on {}, it builds for {}",
		hello["runner"].as_str().unwrap_or("?"),
		hello["server"].as_str().unwrap_or("?"),
		hello["target"]
	);
	println!("restart the runner to use it: systemctl restart genco-runner");
	Ok(())
}

/// The tokens are in there: root and the runner's group only.
fn write_config(path: &Path, cfg: &RunnerConfig) -> Result<(), String> {
	use std::os::unix::fs::OpenOptionsExt as _;
	if let Some(dir) = path.parent() {
		std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
	}
	let text = format!(
		"# written by genco runner register\n{}",
		toml::to_string(cfg).map_err(|e| e.to_string())?
	);
	let tmp = path.with_extension("new");
	let mut f = std::fs::OpenOptions::new()
		.write(true)
		.create(true)
		.truncate(true)
		.mode(0o640)
		.open(&tmp)
		.map_err(|e| format!("{}: {e}", tmp.display()))?;
	std::io::Write::write_all(&mut f, text.as_bytes())
		.map_err(|e| e.to_string())?;
	// keeps the owner the runner reads it as
	if let Ok(meta) = std::fs::metadata(path) {
		use std::os::unix::fs::MetadataExt as _;
		let _ =
			std::os::unix::fs::chown(&tmp, Some(meta.uid()), Some(meta.gid()));
	}
	std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

async fn info() -> RunnerInfo {
	let host = crate::util::sh("hostname").await.unwrap_or_default();
	let engine = crate::util::sh(
		"podman --version 2>/dev/null || docker --version 2>/dev/null",
	)
	.await
	.unwrap_or_default();
	RunnerInfo {
		host,
		engine,
		arch: std::env::consts::ARCH.into(),
		cpus: std::thread::available_parallelism()
			.map_or(1, |n| n.get() as u32),
		version: crate::update::VERSION.into(),
	}
}

async fn run(path: &Path) -> Result<(), String> {
	let cfg = load(path)?;
	if cfg.servers.is_empty() {
		return Err(format!(
			"no server in {}, add one with genco runner register",
			path.display()
		));
	}
	let dir = match &cfg.dir {
		Some(d) => d.clone(),
		None => PathBuf::from(std::env::var("HOME").map_err(|_| "no HOME")?)
			.join("genco-builds"),
	};
	let slots = Arc::new(Semaphore::new(cfg.jobs.max(1)));
	let info = Arc::new(info().await);
	println!(
		"genco runner {} on {} ({}), {} server(s), builds in {}",
		info.version,
		info.host,
		info.engine,
		cfg.servers.len(),
		dir.display()
	);

	let mut tasks = vec![];
	for server in cfg.servers {
		let (slots, info, dir) = (slots.clone(), info.clone(), dir.clone());
		tasks.push(tokio::spawn(async move {
			serve(server, slots, info, dir).await;
		}));
	}
	for t in tasks {
		let _ = t.await;
	}
	Ok(())
}

/// Asks one server for jobs, forever.
async fn serve(
	server: Server,
	slots: Arc<Semaphore>,
	info: Arc<RunnerInfo>,
	dir: PathBuf,
) {
	let http = client();
	// the server sees the runner online while it builds for another one
	// (one slot, several servers): a hello every 30 s
	{
		let (http, server) = (http.clone(), server.clone());
		tokio::spawn(async move {
			let mut warned = false;
			loop {
				let hello = http
					.get(api(&server, "/hello"))
					.bearer_auth(&server.token)
					.timeout(Duration::from_secs(20))
					.send()
					.await;
				// a different genco may build differently
				let version = match hello {
					Ok(r) => r.json::<serde_json::Value>().await.ok(),
					Err(_) => None,
				}
				.and_then(|v| Some(v.get("version")?.as_str()?.to_string()));
				if let Some(v) = version
					&& v != crate::update::VERSION
					&& !warned
				{
					warned = true;
					eprintln!(
						"{}: the server runs genco {v}, this runner {}; \
						update it with genco runner update",
						server.url,
						crate::update::VERSION
					);
				}
				tokio::time::sleep(Duration::from_secs(30)).await;
			}
		});
	}
	loop {
		let slot = slots.clone().acquire_owned().await.unwrap();
		let next = http
			.post(api(&server, "/jobs/next"))
			.bearer_auth(&server.token)
			.timeout(Duration::from_secs(60))
			.json(&*info)
			.send()
			.await;
		let job = match next {
			Ok(r) if r.status() == reqwest::StatusCode::NO_CONTENT => None,
			Ok(r) if r.status().is_success() => {
				r.json::<Option<Job>>().await.ok().flatten()
			}
			Ok(r) => {
				eprintln!("{}: {}", server.url, r.status());
				drop(slot);
				tokio::time::sleep(Duration::from_secs(30)).await;
				continue;
			}
			Err(e) => {
				eprintln!("{}: {e}", server.url);
				drop(slot);
				tokio::time::sleep(Duration::from_secs(15)).await;
				continue;
			}
		};
		if let Some(job) = job {
			println!("{}: job {} in {}", server.url, job.id, job.image);
			let build = Build {
				http: &http,
				server: &server,
				job: &job,
			};
			let res = build.run(&dir).await;
			let (ok, message) = match &res {
				Ok(()) => (true, String::new()),
				Err(e) => (false, e.clone()),
			};
			println!(
				"{}: job {} {}",
				server.url,
				job.id,
				if ok { "done" } else { &message }
			);
			let _ = http
				.post(api(&server, &format!("/jobs/{}/finish", job.id)))
				.bearer_auth(&server.token)
				.json(&serde_json::json!({ "ok": ok, "message": message }))
				.send()
				.await;
		}
		drop(slot);
	}
}

/// A cache key or artifact from the server: a plain relative path.
fn plain(p: &str) -> bool {
	!p.is_empty()
		&& !p.starts_with('/')
		&& p.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
		&& p.chars()
			.all(|c| c.is_ascii_alphanumeric() || "._-/+".contains(c))
}

/// The server's host as a folder name.
fn host_dir(url: &str) -> String {
	// with its port: two servers on one host keep their caches apart
	url.trim_start_matches("https://")
		.split('/')
		.next()
		.unwrap_or("server")
		.chars()
		.map(|c| {
			if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
				c
			} else {
				'_'
			}
		})
		.collect()
}

struct Build<'a> {
	http: &'a reqwest::Client,
	server: &'a Server,
	job: &'a Job,
}

/// `$U` and `$H` of the scripts.
fn with_user(c: &mut Command, u: &ImageUser) {
	c.env("U", &u.user).env("H", &u.home);
}

/// The step's container, after ENGINE; `$U` and `$H` from `ImageUser`.
/// Arguments: image, name, source, cache, command, seconds.
const CONTAINER: &str = r#"set -o pipefail; exec 2>&1
timeout "$6" $E run --rm --name "$2" $U \
	--cap-drop ALL --security-opt no-new-privileges --pids-limit 4096 \
	-v "$3:/src" -v "$4:/cache" -w /src \
	$H -e CARGO_HOME=/cache/cargo -e npm_config_cache=/cache/npm \
	"$1" bash -c "$5"
rc=$?
if [ $rc -eq 124 ]; then
	echo "stopped after $(($6 / 60)) minutes"
	$E rm -f "$2" >/dev/null 2>&1
fi
exit $rc"#;

/// The engine, for the scripts below.
const ENGINE: &str = r#"if command -v podman >/dev/null; then E=podman
elif command -v docker >/dev/null; then E=docker
else echo "neither podman nor docker on the runner"; exit 3; fi"#;

/// The USER of image `$1` (empty for root), after ENGINE; a name, not a
/// number, gets resolved in one container that prints uid and gid.
const IMAGE_USER: &str = r#"u=$($E image inspect --format '{{.Config.User}}' "$1") || exit 1
case "$u" in
	''|root|0|0:0|root:root) echo "0 0" ;;
	[0-9]*:[0-9]*) echo "${u%%:*} ${u##*:}" ;;
	# docker can't map a named user anyway, the image needn't run
	*) [ $E = docker ] && { echo named; exit 0; }
		timeout 60 $E run --rm --network none --cap-drop ALL \
			--security-opt no-new-privileges --pids-limit 64 \
			--entrypoint sh "$1" -c 'echo $(id -u) $(id -g)' ;;
esac"#;

/// Who runs in the job's containers. Rootless podman maps the runner's
/// user onto the image's own: a build image with a USER keeps its HOME
/// and tools (rustup in a private home) and what the build writes belongs
/// to the runner; root images get HOME in the cache. docker can't map
/// users: root images run as the runner's user, others are refused.
struct ImageUser {
	/// `--user` and the user namespace
	user: String,
	/// `-e HOME=...`, empty to keep the image's
	home: String,
	/// the image's uid, for errors
	uid: u32,
}

/// The engine's name and the job image's user, once per job.
async fn image_user(image: &str) -> Result<ImageUser, String> {
	let out = Command::new("bash")
		.args([
			"-c",
			&format!("{ENGINE}\necho $E\n{IMAGE_USER}"),
			"bash",
			image,
		])
		.output()
		.await
		.map_err(|e| e.to_string())?;
	let text = String::from_utf8_lossy(&out.stdout);
	let mut lines = text.lines();
	let engine = lines.next().unwrap_or_default().to_string();
	let ids: Vec<u32> = lines
		.next()
		.unwrap_or_default()
		.split_whitespace()
		.filter_map(|n| n.parse().ok())
		.collect();
	let ids = if ids.is_empty() && text.contains("named") {
		// any user, docker refuses it in user_flags
		vec![u32::MAX, u32::MAX]
	} else {
		ids
	};
	let (Some(&uid), Some(&gid), true) =
		(ids.first(), ids.get(1), out.status.success())
	else {
		return Err(format!(
			"the user of {image}: {}",
			String::from_utf8_lossy(&out.stderr).trim()
		));
	};
	let runner = format!(
		"{}:{}",
		crate::util::sh("id -u").await?,
		crate::util::sh("id -g").await?
	);
	user_flags(&engine, uid, gid, &runner, image)
}

fn user_flags(
	engine: &str,
	uid: u32,
	gid: u32,
	runner: &str,
	image: &str,
) -> Result<ImageUser, String> {
	// root images get their HOME in the cache, the others keep their own
	let home = if uid == 0 {
		"-e HOME=/cache/home".to_string()
	} else {
		String::new()
	};
	// what the build writes belongs to the runner's user: podman maps it
	// onto the image's (root too), docker runs as it
	if engine == "podman" {
		return Ok(ImageUser {
			user: format!(
				"--userns=keep-id:uid={uid},gid={gid} --user {uid}:{gid}"
			),
			home,
			uid,
		});
	}
	if uid != 0 {
		return Err(format!(
			"{image} runs as its own user, which only podman maps onto the \
			runner's; install podman on the runner (genco runner install \
			does) or use an image that runs as root"
		));
	}
	Ok(ImageUser {
		user: format!("--user {runner}"),
		home,
		uid,
	})
}

/// `tar` in a container of the job's image on the source folder, no
/// network: the source goes in and the artifacts come out there, so a
/// link the build planted resolves inside the container, never on the
/// runner's own disk. After ENGINE, `$U` from `ImageUser`. Arguments:
/// image, source, tar's arguments.
const TAR: &str = r#"exec $E run --rm -i --network none --cap-drop ALL \
	--security-opt no-new-privileges $U -v "$2:/src" -w /src "$1" tar "${@:3}""#;

impl Build<'_> {
	fn url(&self, what: &str) -> String {
		api(self.server, &format!("/jobs/{}/{what}", self.job.id))
	}

	async fn run(&self, dir: &Path) -> Result<(), String> {
		let job = self.job;
		if !plain(&job.cache_key) || !job.artifacts.iter().all(|a| plain(a)) {
			return Err("odd paths in the job".into());
		}
		crate::runners::check_image(&job.image)?;
		let base = dir.join(host_dir(&self.server.url)).join(&job.cache_key);
		let src = base.join("src");
		let cache = base.join("cache");
		for d in [&src, &cache.join("home")] {
			std::fs::create_dir_all(d)
				.map_err(|e| format!("{}: {e}", d.display()))?;
		}
		// the server hears from the runner during downloads and uploads
		// too, not only while the step prints
		let alive = {
			let (http, url, token) = (
				self.http.clone(),
				self.url("log"),
				self.server.token.clone(),
			);
			tokio::spawn(async move {
				loop {
					tokio::time::sleep(Duration::from_secs(15)).await;
					let _ = http
						.post(&url)
						.bearer_auth(&token)
						.timeout(Duration::from_secs(30))
						.body("")
						.send()
						.await;
				}
			})
		};
		let res = async {
			self.pull().await?;
			let user = image_user(&job.image).await?;
			clean(&src)?;
			self.source(&src, &user).await?;
			self.build(&src, &cache, &user).await?;
			self.artifacts(&src, &user).await
		}
		.await;
		alive.abort();
		res
	}

	async fn pull(&self) -> Result<(), String> {
		self.send_line(&format!("pulling {}", self.job.image)).await;
		let out = Command::new("bash")
			.args(["-c", &format!("{ENGINE}\n$E pull -q \"$1\""), "bash"])
			.arg(&self.job.image)
			.output()
			.await
			.map_err(|e| e.to_string())?;
		if !out.status.success() {
			return Err(format!(
				"pulling {}: {}",
				self.job.image,
				String::from_utf8_lossy(&out.stderr).trim()
			));
		}
		Ok(())
	}

	/// One line into the deploy's log.
	async fn send_line(&self, line: &str) {
		let mut buf = format!("{line}\n");
		self.send_log(&mut buf).await;
	}

	/// `tar` in a container, see TAR.
	fn tar(&self, src: &Path, user: &ImageUser, args: &[&str]) -> Command {
		let mut c = Command::new("bash");
		c.args(["-c", &format!("{ENGINE}\n{TAR}"), "bash", &self.job.image])
			.arg(src)
			.args(args)
			.kill_on_drop(true);
		with_user(&mut c, user);
		c
	}

	/// The step's folder from the server into `src`.
	async fn source(&self, src: &Path, user: &ImageUser) -> Result<(), String> {
		let mut res = self
			.http
			.get(self.url("source"))
			.bearer_auth(&self.server.token)
			.send()
			.await
			.and_then(|r| r.error_for_status())
			.map_err(|e| format!("the source: {e}"))?;
		let mut tar = self
			.tar(src, user, &["-xf", "-", "--no-same-owner"])
			.stdin(Stdio::piped())
			.stdout(Stdio::null())
			.stderr(Stdio::piped())
			.spawn()
			.map_err(|e| e.to_string())?;
		let mut stdin = tar.stdin.take().unwrap();
		while let Some(chunk) = res.chunk().await.map_err(|e| e.to_string())? {
			stdin.write_all(&chunk).await.map_err(|e| e.to_string())?;
		}
		drop(stdin);
		let out = tar.wait_with_output().await.map_err(|e| e.to_string())?;
		if !out.status.success() {
			return Err(unpack_error(
				&String::from_utf8_lossy(&out.stderr),
				user.uid,
			));
		}
		Ok(())
	}

	/// Runs the step, its output goes to the server every second; a 410
	/// there means the deploy gave up, the container stops.
	async fn build(
		&self,
		src: &Path,
		cache: &Path,
		user: &ImageUser,
	) -> Result<(), String> {
		let job = self.job;
		let name =
			format!("genco-job-{}-{}", host_dir(&self.server.url), job.id);
		let secs = u64::from(job.minutes.max(1)) * 60;
		let mut cmd = Command::new("bash");
		with_user(&mut cmd, user);
		let mut child = cmd
			.args([
				"-c",
				&format!("{ENGINE}\n{CONTAINER}"),
				"bash",
				&job.image,
				&name,
			])
			.arg(src)
			.arg(cache)
			.args([&job.run, &secs.to_string()])
			.stdin(Stdio::null())
			.stdout(Stdio::piped())
			.stderr(Stdio::null())
			.kill_on_drop(true)
			.spawn()
			.map_err(|e| e.to_string())?;
		// bytes, not lines of UTF-8: a build may print anything, the
		// reading must go on (an error would leave the pipe full)
		let mut out = BufReader::new(child.stdout.take().unwrap());
		let mut raw = Vec::new();
		let mut buf = String::new();
		let mut tick = tokio::time::interval(Duration::from_secs(1));
		let mut quiet = 0;
		loop {
			tokio::select! {
				read = read_line(&mut out, &mut raw) => match read {
					Ok(0) | Err(_) => break,
					Ok(_) => {
						// a huge line or output stays bounded per send
						if buf.len() < 256 * 1024 {
							let line = String::from_utf8_lossy(&raw);
							buf.push_str(line.trim_end_matches(['\n', '\r']));
							buf.push('\n');
						}
						raw.clear();
					}
				},
				_ = tick.tick() => {
					quiet += 1;
					// at least every 15 s, the server knows the runner lives
					if !buf.is_empty() || quiet >= 15 {
						quiet = 0;
						if self.send_log(&mut buf).await == Some(false) {
							let _ = Command::new("bash")
								.args(["-c", &format!("{ENGINE}\n$E rm -f \"$1\""), "bash", &name])
								.output()
								.await;
							return Err("the server gave up on the job".into());
						}
					}
				}
			}
		}
		self.send_log(&mut buf).await;
		let status = child.wait().await.map_err(|e| e.to_string())?;
		if !status.success() {
			return Err(format!("the step failed ({status})"));
		}
		Ok(())
	}

	/// Sends what is in `buf`; Some(false) when the server doesn't want the
	/// job anymore, None when it didn't answer (it tries again).
	async fn send_log(&self, buf: &mut String) -> Option<bool> {
		let res = self
			.http
			.post(self.url("log"))
			.bearer_auth(&self.server.token)
			.timeout(Duration::from_secs(30))
			.body(buf.clone())
			.send()
			.await
			.ok()?;
		if res.status() == reqwest::StatusCode::GONE {
			return Some(false);
		}
		if res.status().is_success() {
			buf.clear();
		}
		Some(true)
	}

	/// The artifacts as a tarball to the server, packed in a container
	/// (see TAR): a link in `src` can't point the runner at its own files.
	async fn artifacts(
		&self,
		src: &Path,
		user: &ImageUser,
	) -> Result<(), String> {
		let list = &self.job.artifacts;
		if list.is_empty() {
			return Ok(());
		}
		let mut args = vec!["-cf", "-", "--"];
		args.extend(list.iter().map(String::as_str));
		let mut tar = self
			.tar(src, user, &args)
			.stdin(Stdio::null())
			.stdout(Stdio::piped())
			.stderr(Stdio::piped())
			.spawn()
			.map_err(|e| e.to_string())?;
		let out = tar.stdout.take().unwrap();
		let body =
			reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(out));
		let res = self
			.http
			.put(self.url("artifacts"))
			.bearer_auth(&self.server.token)
			.body(body)
			.send()
			.await
			.and_then(|r| r.error_for_status());
		let packed = tar.wait_with_output().await.map_err(|e| e.to_string())?;
		if !packed.status.success() {
			return Err(format!(
				"the build made no {}: {}",
				list.join(", "),
				String::from_utf8_lossy(&packed.stderr).trim()
			));
		}
		res.map_err(|e| format!("sending the artifacts: {e}"))?;
		Ok(())
	}
}

/// tar's errors in one line: "Permission denied" for every file means
/// the image's user can't write the source folder.
fn unpack_error(stderr: &str, uid: u32) -> String {
	let denied = stderr
		.lines()
		.filter(|l| l.contains("Permission denied"))
		.count();
	if denied > 0 {
		return format!(
			"the build image's user (uid {uid}) can't write the source \
			folder of the runner's user ({denied} files: Permission \
			denied); the image needs podman's user mapping or to run as \
			root"
		);
	}
	let first = stderr.lines().next().unwrap_or("").trim();
	format!("unpacking the source failed: {first}")
}

/// The next line, at most 8 KiB of it; what was read stays in `raw` when
/// the future is dropped (select!), the next call goes on from there.
async fn read_line<R: tokio::io::AsyncBufRead + Unpin>(
	out: &mut R,
	raw: &mut Vec<u8>,
) -> std::io::Result<usize> {
	out.take(8192).read_until(b'\n', raw).await
}

/// Empties `src` but the build output of earlier builds.
fn clean(src: &Path) -> Result<(), String> {
	for entry in std::fs::read_dir(src).map_err(|e| e.to_string())?.flatten() {
		let name = entry.file_name();
		if name == "target" || name == "node_modules" {
			continue;
		}
		let path = entry.path();
		let is_dir = std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir());
		let res = if is_dir {
			std::fs::remove_dir_all(&path)
		} else {
			std::fs::remove_file(&path)
		};
		res.map_err(|e| format!("{}: {e}", path.display()))?;
	}
	Ok(())
}

async fn install(user: &str) -> Result<(), String> {
	use crate::util::sh;
	if sh("id -u").await? != "0" {
		return Err("genco runner install needs root".into());
	}
	if !user
		.chars()
		.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
	{
		return Err("odd user name".into());
	}
	println!("==> podman");
	sh(
		"command -v podman >/dev/null || command -v docker >/dev/null || \
		DEBIAN_FRONTEND=noninteractive apt-get install -y -qq podman uidmap passt",
	)
	.await?;
	println!("==> the user {user}");
	sh(&format!(
		"id {user} >/dev/null 2>&1 || useradd --system --create-home \
		--home-dir /var/lib/{user} --shell /usr/sbin/nologin --add-subids-for-system {user}; \
		loginctl enable-linger {user}; \
		if ! command -v podman >/dev/null && getent group docker >/dev/null; then \
			usermod -aG docker {user}; fi"
	))
	.await?;
	let uid = sh(&format!("id -u {user}")).await?;
	// an existing user keeps its home, e.g. on a big disk
	let home = sh(&format!("getent passwd {user} | cut -d: -f6")).await?;
	if !Path::new(CONFIG).exists() {
		write_config(
			Path::new(CONFIG),
			&RunnerConfig {
				jobs: 1,
				..Default::default()
			},
		)?;
	}
	sh(&format!("chown root:{user} {CONFIG} && chmod 640 {CONFIG}")).await?;

	println!("==> genco-runner.service");
	let exe = std::env::current_exe().map_err(|e| e.to_string())?;
	let unit = format!(
		"# written by genco runner install
[Unit]
Description=genco build runner
After=network-online.target
Wants=network-online.target

[Service]
User={user}
Group={user}
Environment=XDG_RUNTIME_DIR=/run/user/{uid}
WorkingDirectory={home}
ExecStart={} runner run
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
",
		exe.display()
	);
	std::fs::write(UNIT, unit).map_err(|e| format!("{UNIT}: {e}"))?;

	println!("==> genco-runner-update.timer");
	let update = format!(
		"# written by genco runner install
[Unit]
Description=genco runner update (signed releases)

[Service]
Type=oneshot
PrivateTmp=yes
ExecStart={} runner update
",
		exe.display()
	);
	std::fs::write(UPDATE_UNIT, update)
		.map_err(|e| format!("{UPDATE_UNIT}: {e}"))?;
	std::fs::write(
		UPDATE_TIMER,
		"# written by genco runner install
[Unit]
Description=genco runner update, daily

[Timer]
OnCalendar=*-*-* 04:30
RandomizedDelaySec=30m
Persistent=true

[Install]
WantedBy=timers.target
",
	)
	.map_err(|e| format!("{UPDATE_TIMER}: {e}"))?;
	sh(
		"systemctl daemon-reload && systemctl enable -q genco-runner \
		&& systemctl enable -q --now genco-runner-update.timer",
	)
	.await?;
	println!(
		"done; add servers with genco runner register <url> <token>, then \
		systemctl restart genco-runner"
	);
	Ok(())
}

/// `genco runner update`: the signed release replaces this binary, the
/// runner restarts. Not on a genco server, `genco update` keeps that one.
async fn update(version: Option<&str>) -> Result<(), String> {
	if crate::util::sh("id -u").await? != "0" {
		return Err("genco runner update needs root".into());
	}
	if Path::new("/etc/genco/config.toml").exists() {
		return Err("this box is a genco server, use genco update".into());
	}
	let Some(file) = crate::update::fetch_signed(version).await? else {
		println!("genco {} is the newest", crate::update::VERSION);
		return Ok(());
	};
	let exe = std::env::current_exe().map_err(|e| e.to_string())?;
	let tmp = exe.with_extension("new");
	std::fs::copy(&file, &tmp)
		.and_then(|_| crate::util::set_mode(&tmp, 0o755))
		.and_then(|()| std::fs::rename(&tmp, &exe))
		.map_err(|e| format!("{}: {e}", exe.display()))?;
	crate::util::sh("systemctl try-restart genco-runner").await?;
	println!("installed {}, the runner restarted", file.display());
	Ok(())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn only_plain_paths_from_the_server() {
		assert!(plain("shop/cms"));
		assert!(plain("shop-cms"));
		assert!(!plain("../x"));
		assert!(!plain("/etc"));
		assert!(!plain("a//b"));
		assert!(!plain("a/./b"));
		assert!(!plain("a b"));
	}

	#[test]
	fn image_users() {
		let root = user_flags("docker", 0, 0, "1001:1001", "debian").unwrap();
		assert_eq!(root.user, "--user 1001:1001");
		assert_eq!(root.home, "-e HOME=/cache/home");
		let root = user_flags("podman", 0, 0, "1001:1001", "debian").unwrap();
		assert_eq!(root.user, "--userns=keep-id:uid=0,gid=0 --user 0:0");
		assert_eq!(root.home, "-e HOME=/cache/home");

		let own = user_flags("podman", 1000, 1000, "1001:1001", "b").unwrap();
		assert_eq!(
			own.user,
			"--userns=keep-id:uid=1000,gid=1000 --user 1000:1000"
		);
		assert!(own.home.is_empty());
		assert!(user_flags("docker", 1000, 1000, "1001:1001", "b").is_err());
	}

	#[test]
	fn permission_errors_in_one_line() {
		let e = unpack_error(
			"tar: ./a: Cannot open: Permission denied\n\
			tar: ./b: Cannot open: Permission denied\n",
			1000,
		);
		assert!(e.contains("uid 1000"));
		assert!(e.contains("2 files"));
		assert!(!e.contains('\n'));
	}

	#[test]
	fn hosts_as_folders() {
		assert_eq!(host_dir("https://genco.s4.ch"), "genco.s4.ch");
		assert_eq!(host_dir("https://genco.s4.ch:8443/x"), "genco.s4.ch_8443");
	}

	#[test]
	fn the_config_round_trips() {
		let cfg: RunnerConfig =
			toml::from_str("[[server]]\nurl = \"https://a\"\ntoken = \"t\"\n")
				.unwrap();
		assert_eq!(cfg.jobs, 1);
		assert_eq!(cfg.servers[0].url, "https://a");
		let back = toml::to_string(&cfg).unwrap();
		assert!(back.contains("[[server]]"));
	}
}
