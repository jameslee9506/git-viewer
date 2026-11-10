//! Single entry point for spawning the git CLI.
//!
//! Every invocation is read-only friendly: optional locks are disabled so that
//! `git status` never rewrites the index (which would also retrigger the file watcher),
//! output is never colored or paged, and user config that would break parsing is overridden.
//! Every git runs with the user's login-shell `PATH` (see [`path_env`]), so the programs git itself
//! spawns by bare name are found: `git-lfs` for the filters and hooks of a Git LFS repo, credential
//! helpers, hooks. This module is the only one that spawns processes: git, and the login shell once.

use crate::error::{AppError, AppResult};
use rustix::process::{kill_process, Pid, Signal};
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

static GIT_PATH: OnceLock<PathBuf> = OnceLock::new();
static PATH_ENV: OnceLock<OsString> = OnceLock::new();

/// How long the login shell may take to print its PATH.
const SHELL_TIMEOUT: Duration = Duration::from_secs(10);

/// How often a running git is checked against its deadline and cancel flag.
const POLL: Duration = Duration::from_millis(4);

/// How long a git that was asked to end may take over it before it is killed (see [`end`]).
const TERM_GRACE: Duration = Duration::from_secs(2);

/// Read size for streamed stdout (`Git::output_streaming`).
const STREAM_CHUNK: usize = 64 * 1024;

/// Max git processes running at once (bursts of requests would otherwise spawn dozens).
const MAX_CONCURRENT: usize = 8;
static SLOTS: (std::sync::Mutex<usize>, std::sync::Condvar) = (std::sync::Mutex::new(0), std::sync::Condvar::new());

/// Holds one of the MAX_CONCURRENT process slots until dropped.
struct Slot;

impl Slot {
    fn acquire() -> Slot {
        let (lock, cvar) = &SLOTS;
        let mut n = lock.lock().unwrap();
        while *n >= MAX_CONCURRENT {
            n = cvar.wait(n).unwrap();
        }
        *n += 1;
        Slot
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        let (lock, cvar) = &SLOTS;
        *lock.lock().unwrap() -= 1;
        cvar.notify_one();
    }
}

/// Resolve the git executable. GUI apps on macOS get a minimal PATH, so probe well-known locations.
pub fn git_binary() -> &'static Path {
    GIT_PATH.get_or_init(|| {
        if let Ok(p) = std::env::var("GIT_VIEWER_GIT") {
            if !p.is_empty() {
                return PathBuf::from(p);
            }
        }
        // Prefer real binaries over the /usr/bin/git xcrun shim (2x slower per spawn, may prompt for CLT).
        for cand in [
            "/opt/homebrew/bin/git",
            "/usr/local/bin/git",
            "/Library/Developer/CommandLineTools/usr/bin/git",
            "/Applications/Xcode.app/Contents/Developer/usr/bin/git",
            "/usr/bin/git",
        ] {
            let p = PathBuf::from(cand);
            if p.is_file() {
                return p;
            }
        }
        PathBuf::from("git")
    })
}

/// `PATH` for every git process: the login shell's PATH, then the current one, then the directory of
/// the git binary and the Homebrew / `/usr/local` bin directories when still missing. Resolved once
/// (about 0.2 s; `lib.rs` starts that at launch so the first repo opens without the wait).
///
/// A GUI app launched from the Finder runs with `/usr/bin:/bin:/usr/sbin:/sbin`, where git cannot
/// find what it spawns by bare name. In a Git LFS repo that is `git-lfs`: `status`, `diff-files`,
/// `add` and `stash` run the clean / smudge filters (`filter.lfs.required` makes a missing
/// `git-lfs` fatal: "git-lfs: command not found", "the remote end hung up unexpectedly") and
/// `switch -c` / `commit` / `pull` / `push` run the LFS hooks; elsewhere commit hooks and credential
/// helpers configured by bare name (`!gh …`, `node`).
pub fn path_env() -> &'static OsStr {
    PATH_ENV.get_or_init(|| {
        let current = std::env::var("PATH").unwrap_or_default();
        let git_dir = git_binary().parent().map(|p| p.to_string_lossy().into_owned());
        OsString::from(compose_path(login_path().as_deref(), &current, git_dir.as_deref()))
    })
}

/// `login`'s then `current`'s entries (duplicates dropped), then `git_dir`, `/opt/homebrew/bin` and
/// `/usr/local/bin` unless already present.
fn compose_path(login: Option<&str>, current: &str, git_dir: Option<&str>) -> String {
    let mut parts: Vec<&str> = Vec::new();
    let listed = login.into_iter().chain([current]).flat_map(|s| s.split(':'));
    for p in listed.chain(git_dir).chain(["/opt/homebrew/bin", "/usr/local/bin"]) {
        if !p.is_empty() && !parts.contains(&p) {
            parts.push(p);
        }
    }
    parts.join(":")
}

/// PATH of the user's login shell: `$SHELL` (else `/bin/zsh`) with `-ilc` printing `printenv PATH`
/// between markers (shell-agnostic: fish would join `"$PATH"` with spaces), 10 s deadline, stdin
/// closed and `TERM=dumb` so nothing waits for a terminal. None when that fails.
fn login_path() -> Option<String> {
    let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/bin/zsh".into());
    let mut child = Command::new(shell)
        .args(["-ilc", "printf __GV_PATH__; printenv PATH; printf __GV_END__"])
        .env("TERM", "dumb")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = stdout.read_to_end(&mut v);
        v
    });
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
        if started.elapsed() >= SHELL_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let text = lossy(&reader.join().ok()?);
    let start = text.find("__GV_PATH__")? + "__GV_PATH__".len();
    let end = text[start..].find("__GV_END__")? + start;
    let path = text[start..end].trim();
    (!path.is_empty()).then(|| path.to_string())
}

/// Config overrides applied to every invocation (before the subcommand).
const BASE_CONFIG: &[&str] = &[
    "core.quotepath=false",
    "color.ui=never",
    "log.showSignature=false",
    "i18n.logOutputEncoding=UTF-8",
    "diff.noprefix=false",
    "diff.mnemonicPrefix=false",
    "diff.relative=false",
    "diff.suppressBlankEmpty=false",
    "core.pager=cat",
];

/// Builder for one git invocation.
///
/// ```ignore
/// let out = Git::new(&repo).args(["log", "-z", "--format=%H"]).run()?;
/// ```
pub struct Git {
    cwd: PathBuf,
    args: Vec<OsString>,
    stdin: Option<Vec<u8>>,
    ok_codes: Vec<i32>,
    literal_paths: bool,
    /// Run outside the MAX_CONCURRENT slots (network operations that may run for minutes).
    unslotted: bool,
    timeout: Option<Duration>,
    /// Longest silence allowed (see `idle_timeout`).
    idle: Option<Duration>,
    /// Git is stopped as soon as one of these is set.
    cancel: Vec<Arc<AtomicBool>>,
    envs: Vec<(OsString, OsString)>,
}

/// Raw result of an invocation that may legitimately exit non-zero.
pub struct GitOutput {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Git {
    pub fn new(cwd: impl AsRef<Path>) -> Self {
        Git {
            cwd: cwd.as_ref().to_path_buf(),
            args: Vec::new(),
            stdin: None,
            ok_codes: vec![0],
            literal_paths: false,
            unslotted: false,
            timeout: None,
            idle: None,
            cancel: Vec::new(),
            envs: Vec::new(),
        }
    }

    pub fn arg(mut self, a: impl AsRef<OsStr>) -> Self {
        self.args.push(a.as_ref().to_os_string());
        self
    }

    pub fn args<I, S>(mut self, it: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        for a in it {
            self.args.push(a.as_ref().to_os_string());
        }
        self
    }

    /// Bytes written to git's stdin (e.g. revisions for `git log --stdin`).
    pub fn stdin(mut self, data: Vec<u8>) -> Self {
        self.stdin = Some(data);
        self
    }

    /// Additional exit codes treated as success (e.g. `diff --no-index` exits 1 when files differ).
    pub fn allow_codes(mut self, codes: &[i32]) -> Self {
        self.ok_codes.extend_from_slice(codes);
        self
    }

    /// Treat pathspecs literally (`GIT_LITERAL_PATHSPECS=1`): use whenever passing real file paths,
    /// so names like `[ab].txt` don't act as globs. Never for glob searches.
    pub fn literal_paths(mut self) -> Self {
        self.literal_paths = true;
        self
    }

    /// Run without taking one of the MAX_CONCURRENT process slots: fetch / pull / push can run for
    /// minutes and must not starve the read-only commands of their slots.
    pub fn unslotted(mut self) -> Self {
        self.unslotted = true;
        self
    }

    /// Kill git and fail if it runs longer than `d` (searches, blame on huge files).
    pub fn timeout(mut self, d: Duration) -> Self {
        self.timeout = Some(d);
        self
    }

    /// Stop git and fail once it has written nothing, to stdout or stderr, for `d`: for a command that
    /// reports as it goes (a fetch with `--progress`), where silence means a connection that hangs
    /// and no total time is too long. Not with `output_streaming`.
    pub fn idle_timeout(mut self, d: Duration) -> Self {
        self.idle = Some(d);
        self
    }

    /// Extra environment variable for this invocation (applied after the defaults).
    pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.envs.push((key.as_ref().to_os_string(), value.as_ref().to_os_string()));
        self
    }

    /// Kill git and fail with "cancelled" as soon as `flag` becomes true (superseded searches). Several
    /// flags may be given: any of them cancels (a background fetch: its own flag and the closed repo's).
    pub fn cancel_on(mut self, flag: Arc<AtomicBool>) -> Self {
        self.cancel.push(flag);
        self
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(git_binary());
        cmd.current_dir(&self.cwd);
        cmd.arg("--no-pager").arg("--no-optional-locks");
        for c in BASE_CONFIG {
            cmd.arg("-c").arg(c);
        }
        cmd.args(&self.args);
        for v in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_NAMESPACE", "GIT_OBJECT_DIRECTORY", "GIT_COMMON_DIR"] {
            cmd.env_remove(v);
        }
        cmd.env("PATH", path_env())
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("GIT_PAGER", "cat")
            .env("PAGER", "cat")
            // UTF-8 aware C locale: English messages, correct case-insensitive matching of non-ASCII.
            .env("LC_ALL", "C.UTF-8")
            .env("LANG", "C.UTF-8");
        if self.literal_paths {
            cmd.env("GIT_LITERAL_PATHSPECS", "1");
        }
        for (k, v) in &self.envs {
            cmd.env(k, v);
        }
        cmd.stdin(if self.stdin.is_some() { Stdio::piped() } else { Stdio::null() })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd
    }

    fn describe(&self) -> String {
        self.args.iter().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ")
    }

    fn cancelled(&self) -> bool {
        self.cancel.iter().any(|c| c.load(Ordering::Relaxed))
    }

    /// Start git in one of the process slots (held until the returned `Slot` drops; none when
    /// `unslotted`); stdin, if any, is written on a thread.
    fn spawn(&self) -> AppResult<(Child, Option<Slot>)> {
        // A missing cwd (deleted / prunable worktree) would otherwise surface as "git not found".
        if !self.cwd.is_dir() {
            return Err(AppError::msg(format!("not a directory: {}", self.cwd.display())));
        }
        let slot = (!self.unslotted).then(Slot::acquire);
        // Cancelled while waiting for a slot: don't start git at all.
        if self.cancelled() {
            return Err(AppError::msg("cancelled"));
        }
        let mut child = self.command().spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AppError::msg("git executable not found (install Xcode Command Line Tools or Homebrew git)")
            } else {
                AppError::Io(e)
            }
        })?;
        if let Some(data) = self.stdin.as_ref() {
            if let Some(mut sin) = child.stdin.take() {
                let data = data.clone();
                // Write on a separate thread so a large stdin cannot deadlock against a full stdout pipe.
                std::thread::spawn(move || {
                    let _ = sin.write_all(&data);
                });
            }
        }
        Ok((child, slot))
    }

    /// Run and return the raw output regardless of exit code. A git that is stopped here (cancel flag,
    /// deadline, silence) is asked to end before it is killed (see [`end`]): every write operation runs
    /// through this.
    pub fn output(self) -> AppResult<GitOutput> {
        let (mut child, _slot) = self.spawn()?;
        if self.timeout.is_none() && self.idle.is_none() && self.cancel.is_empty() {
            let out = child.wait_with_output()?;
            return Ok(GitOutput { code: out.status.code().unwrap_or(-1), stdout: out.stdout, stderr: out.stderr });
        }
        // With a deadline, an idle limit or a cancel flag: drain both pipes on threads and poll for exit.
        let started = Instant::now();
        // When git last wrote to a pipe, in ms after `started` (kept only for an idle limit).
        let wrote = Arc::new(AtomicU64::new(0));
        let clock = self.idle.map(|_| (started, wrote.clone()));
        let out_t = drain(child.stdout.take(), clock.clone());
        let err_t = drain(child.stderr.take(), clock);
        let status = loop {
            if let Some(st) = child.try_wait()? {
                break st;
            }
            let silent = || started.elapsed().saturating_sub(Duration::from_millis(wrote.load(Ordering::Relaxed)));
            let why = if self.cancelled() {
                "cancelled"
            } else if self.timeout.is_some_and(|limit| started.elapsed() >= limit) {
                "git took too long and was stopped"
            } else if self.idle.is_some_and(|limit| silent() >= limit) {
                "git stopped responding and was stopped"
            } else {
                std::thread::sleep(POLL);
                continue;
            };
            end(&mut child);
            return Err(AppError::msg(why));
        };
        let stdout = out_t.join().unwrap_or_default();
        let stderr = err_t.join().unwrap_or_default();
        Ok(GitOutput { code: status.code().unwrap_or(-1), stdout, stderr })
    }

    /// Like `output`, but stdout goes to `sink` chunk by chunk as git writes it instead of being
    /// collected (the returned `stdout` is empty), so a huge output never sits in memory whole. `sink`
    /// returns false once it needs no more output: git is then killed and `code` is -1. stderr is
    /// drained on a thread; a deadline or cancel flag is watched by another one, which kills git
    /// (ending the read with EOF) and makes the call fail as `output` does. Killed outright, unlike
    /// in `output`: only for commands that read (they hold no lock and write no file).
    pub fn output_streaming(self, mut sink: impl FnMut(&[u8]) -> bool) -> AppResult<GitOutput> {
        let (mut child, _slot) = self.spawn()?;
        let stdout = child.stdout.take();
        let mut err_pipe = child.stderr.take();
        let err_t = std::thread::spawn(move || {
            let mut v = Vec::new();
            if let Some(p) = err_pipe.as_mut() {
                let _ = p.read_to_end(&mut v);
            }
            v
        });
        let child = Mutex::new(child);
        let kill = || {
            let _ = child.lock().unwrap_or_else(|e| e.into_inner()).kill();
        };
        let (stopped, read_err, killed_for) = std::thread::scope(|s| {
            // Dropped once reading is over: wakes the watchdog, which then ends.
            let (reading, done) = mpsc::channel::<()>();
            let (this, kill) = (&self, &kill);
            let watchdog = (self.timeout.is_some() || !self.cancel.is_empty()).then(|| {
                s.spawn(move || {
                    let started = Instant::now();
                    while let Err(RecvTimeoutError::Timeout) = done.recv_timeout(POLL) {
                        let why = if this.cancelled() {
                            "cancelled"
                        } else if this.timeout.is_some_and(|limit| started.elapsed() >= limit) {
                            "git took too long and was stopped"
                        } else {
                            continue;
                        };
                        kill();
                        return Some(why);
                    }
                    None
                })
            });
            let mut stopped = false;
            let mut read_err = None;
            if let Some(mut out) = stdout {
                let mut buf = vec![0u8; STREAM_CHUNK];
                loop {
                    match out.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if !sink(&buf[..n]) {
                                stopped = true;
                                break;
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(e) => {
                            read_err = Some(e);
                            break;
                        }
                    }
                }
            }
            if stopped || read_err.is_some() {
                kill();
            }
            drop(reading);
            let killed_for = watchdog.and_then(|w| w.join().ok().flatten());
            (stopped, read_err, killed_for)
        });
        let status = child.into_inner().unwrap_or_else(|e| e.into_inner()).wait()?;
        let stderr = err_t.join().unwrap_or_default();
        if let Some(why) = killed_for {
            return Err(AppError::msg(why));
        }
        if let Some(e) = read_err {
            return Err(AppError::Io(e));
        }
        let code = if stopped { -1 } else { status.code().unwrap_or(-1) };
        Ok(GitOutput { code, stdout: Vec::new(), stderr })
    }

    /// `output_streaming` that fails unless the exit code is allowed or `sink` stopped git early.
    pub fn run_streaming(self, mut sink: impl FnMut(&[u8]) -> bool) -> AppResult<()> {
        let desc = self.describe();
        let ok = self.ok_codes.clone();
        let mut stopped = false;
        let out = self.output_streaming(|chunk| {
            let more = sink(chunk);
            stopped = !more;
            more
        })?;
        if stopped || ok.contains(&out.code) {
            Ok(())
        } else {
            Err(AppError::Git { args: desc, code: Some(out.code), stderr: lossy(&out.stderr) })
        }
    }

    /// Run and return stdout; fails unless the exit code is allowed.
    pub fn run(self) -> AppResult<Vec<u8>> {
        let desc = self.describe();
        let ok = self.ok_codes.clone();
        let out = self.output()?;
        if ok.contains(&out.code) {
            Ok(out.stdout)
        } else {
            Err(AppError::Git {
                args: desc,
                code: Some(out.code),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            })
        }
    }

    /// Run and return stdout decoded as (lossy) UTF-8.
    pub fn run_string(self) -> AppResult<String> {
        self.run().map(|b| String::from_utf8_lossy(&b).into_owned())
    }

    /// Run and return the whole output; fails like `run` unless the exit code is allowed (write
    /// operations report their stderr too, e.g. the summary `git push` prints there).
    pub fn output_checked(self) -> AppResult<GitOutput> {
        let desc = self.describe();
        let ok = self.ok_codes.clone();
        let out = self.output()?;
        if ok.contains(&out.code) {
            Ok(out)
        } else {
            Err(AppError::Git { args: desc, code: Some(out.code), stderr: lossy(&out.stderr) })
        }
    }
}

/// Read `pipe` to its end on a thread. With a `clock` (a start and a cell), the cell keeps when
/// data last came, in ms after that start.
fn drain(pipe: Option<impl Read + Send + 'static>, clock: Option<(Instant, Arc<AtomicU64>)>) -> JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut all = Vec::new();
        let Some(mut pipe) = pipe else { return all };
        let Some((started, wrote)) = clock else {
            let _ = pipe.read_to_end(&mut all);
            return all;
        };
        let mut buf = vec![0u8; STREAM_CHUNK];
        loop {
            match pipe.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    all.extend_from_slice(&buf[..n]);
                    wrote.fetch_max(started.elapsed().as_millis() as u64, Ordering::Relaxed);
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        all
    })
}

/// End a running git and reap it. It is asked first (SIGTERM), so that it removes what it holds: a
/// fetch the locks of the refs it updates and the `.keep` file of the pack it stored, a commit or a
/// merge its `index.lock`. A kill leaves those behind, and a stale ref lock fails every later fetch
/// and pull ("Unable to create '….lock': File exists") until someone deletes it. Killed after
/// [`TERM_GRACE`] if it is still there (a command that only reads is gone at once either way).
fn end(child: &mut Child) {
    if kill_process(Pid::from_child(child), Signal::TERM).is_ok() {
        let deadline = Instant::now() + TERM_GRACE;
        while Instant::now() < deadline {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(POLL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Split a byte buffer on `sep`, dropping a single trailing empty record.
pub fn split_records(buf: &[u8], sep: u8) -> Vec<&[u8]> {
    let mut v: Vec<&[u8]> = buf.split(|b| *b == sep).collect();
    if v.last().is_some_and(|l| l.is_empty()) {
        v.pop();
    }
    v
}

/// Lossy UTF-8 conversion of a byte slice.
pub fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// Undo git's C-style path quoting (`"tab\tname.txt"`, `"\303\244"`), used where `-z` cannot prevent it
/// (e.g. blame `filename`). Unquoted input is returned unchanged.
pub fn unquote_c(b: &[u8]) -> Vec<u8> {
    if b.len() < 2 || b[0] != b'"' || b[b.len() - 1] != b'"' {
        return b.to_vec();
    }
    let inner = &b[1..b.len() - 1];
    let mut out = Vec::with_capacity(inner.len());
    let mut i = 0;
    while i < inner.len() {
        let c = inner[i];
        if c != b'\\' || i + 1 >= inner.len() {
            out.push(c);
            i += 1;
            continue;
        }
        let e = inner[i + 1];
        i += 2;
        match e {
            b'a' => out.push(0x07),
            b'b' => out.push(0x08),
            b't' => out.push(b'\t'),
            b'n' => out.push(b'\n'),
            b'v' => out.push(0x0b),
            b'f' => out.push(0x0c),
            b'r' => out.push(b'\r'),
            b'0'..=b'7' => {
                let mut v: u32 = (e - b'0') as u32;
                let mut n = 1;
                while n < 3 && i < inner.len() && (b'0'..=b'7').contains(&inner[i]) {
                    v = v * 8 + (inner[i] - b'0') as u32;
                    i += 1;
                    n += 1;
                }
                out.push((v & 0xff) as u8);
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unquote_plain_and_escapes() {
        assert_eq!(unquote_c(b"a.txt"), b"a.txt");
        assert_eq!(unquote_c(b"\"tab\\tname.txt\""), b"tab\tname.txt");
        assert_eq!(unquote_c(b"\"q\\\"x\\\\y\""), b"q\"x\\y");
        assert_eq!(String::from_utf8(unquote_c(b"\"\\355\\225\\234\"")).unwrap(), "\u{d55c}");
    }

    #[test]
    fn split_records_drops_trailing_empty() {
        assert_eq!(split_records(b"a\0b\0", 0), vec![&b"a"[..], &b"b"[..]]);
        assert_eq!(split_records(b"", 0), Vec::<&[u8]>::new());
    }

    #[test]
    fn path_composition() {
        assert_eq!(
            compose_path(Some("/opt/homebrew/bin:/usr/bin"), "/usr/bin:/bin", Some("/opt/homebrew/bin")),
            "/opt/homebrew/bin:/usr/bin:/bin:/usr/local/bin"
        );
        // Login shell unavailable: the git binary's directory still gets git-lfs found next to it.
        assert_eq!(compose_path(None, "/usr/bin:/bin", Some("/usr/local/bin")), "/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin");
        assert_eq!(compose_path(None, "", None), "/opt/homebrew/bin:/usr/local/bin");
    }
}
