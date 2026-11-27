//! Port of `ProcessRun.swift` — run a program to completion, stdin fed,
//! stdout + stderr drained concurrently (Foundation/std only).

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Instant;

pub struct ProcessOutput {
    pub code: i32,
    pub out: String,
    pub err: String,
}

/// Run `exe args` to completion. `env`, when given, replaces the environment.
/// `merge_stderr` folds stderr into `out` (and leaves `err` empty).
pub fn run_process(
    exe: &str,
    args: &[String],
    stdin: Option<&[u8]>,
    env: Option<&[(String, String)]>,
    merge_stderr: bool,
) -> std::io::Result<ProcessOutput> {
    run_process_deadline(exe, args, stdin, env, merge_stderr, None)
}

/// `run_process` with a hard deadline. On timeout the child is killed and a
/// note is appended to `err` with code `-1`: a wedged `jira_poll.py` (or any
/// helper child) must never hold a UI "busy" slot forever — that state used
/// to survive daemon tasks one by one until a restart.
pub fn run_process_deadline(
    exe: &str,
    args: &[String],
    stdin: Option<&[u8]>,
    env: Option<&[(String, String)]>,
    merge_stderr: bool,
    timeout: Option<std::time::Duration>,
) -> std::io::Result<ProcessOutput> {
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    if let Some(env) = env {
        cmd.env_clear();
        for (k, v) in env {
            cmd.env(k, v);
        }
    }
    let mut child = cmd.spawn()?;

    if let Some(data) = stdin {
        if let Some(mut si) = child.stdin.take() {
            let _ = si.write_all(data);
            // dropping `si` closes the pipe, signalling EOF
        }
    }

    // Drain both pipes concurrently so a full pipe buffer can't deadlock.
    let out_pipe = child.stdout.take();
    let err_pipe = child.stderr.take();
    let out_h = thread::spawn(move || {
        let mut b = Vec::new();
        if let Some(mut p) = out_pipe {
            let _ = p.read_to_end(&mut b);
        }
        b
    });
    let err_h = thread::spawn(move || {
        let mut b = Vec::new();
        if let Some(mut p) = err_pipe {
            let _ = p.read_to_end(&mut b);
        }
        b
    });

    let mut timed_out = false;
    let status = match timeout {
        None => child.wait()?,
        Some(limit) => wait_with_timeout(&mut child, limit, &mut timed_out)?,
    };
    let mut out = out_h.join().unwrap_or_default();
    let err = err_h.join().unwrap_or_default();
    let mut err_str = String::from_utf8_lossy(&err).into_owned();
    if timed_out {
        let secs = timeout.map(|d| d.as_secs()).unwrap_or(0);
        err_str.push_str(&format!("\n{exe} timed out after {secs}s and was killed"));
    }
    if merge_stderr {
        out.extend_from_slice(&err);
    }
    Ok(ProcessOutput {
        code: if timed_out { -1 } else { status.code().unwrap_or(-1) },
        out: String::from_utf8_lossy(&out).into_owned(),
        err: if merge_stderr { String::new() } else { err_str },
    })
}

/// Wait for `child` up to `limit` without polling: uses `kqueue` `EVFILT_PROC`
/// (`NOTE_EXIT`) on macOS for 0 wakeups and 0 ms exit-detection latency.
fn wait_with_timeout(
    child: &mut std::process::Child,
    limit: std::time::Duration,
    timed_out: &mut bool,
) -> std::io::Result<std::process::ExitStatus> {
    if let Some(st) = child.try_wait()? {
        return Ok(st);
    }
    #[cfg(target_os = "macos")]
    {
        let kq = unsafe { libc::kqueue() };
        if kq >= 0 {
            let pid = child.id() as usize;
            let kev = libc::kevent {
                ident: pid,
                filter: libc::EVFILT_PROC,
                flags: libc::EV_ADD | libc::EV_ONESHOT,
                fflags: libc::NOTE_EXIT,
                data: 0,
                udata: std::ptr::null_mut(),
            };
            let ts = libc::timespec {
                tv_sec: limit.as_secs() as libc::time_t,
                tv_nsec: limit.subsec_nanos() as libc::c_long,
            };
            let mut out: libc::kevent = unsafe { std::mem::zeroed() };
            let n = unsafe { libc::kevent(kq, &kev, 1, &mut out, 1, &ts) };
            unsafe { libc::close(kq) };
            if n == 0 {
                // Deadline expired before child exited.
                if let Some(st) = child.try_wait()? {
                    return Ok(st);
                }
                let _ = child.kill();
                *timed_out = true;
                return Ok(child.wait().unwrap_or_else(|_| std::process::ExitStatus::default()));
            }
            // Either NOTE_EXIT fired or ESRCH (already exited): reap immediately.
            if let Some(st) = child.try_wait()? {
                return Ok(st);
            }
            // Rare EINTR / race: fall through to bounded wait below.
        }
    }
    let start = Instant::now();
    loop {
        match child.try_wait()? {
            Some(st) => return Ok(st),
            None => {
                if start.elapsed() >= limit {
                    let _ = child.kill();
                    *timed_out = true;
                    return Ok(child.wait().unwrap_or_else(|_| std::process::ExitStatus::default()));
                }
                thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_stdout() {
        let r = run_process("/bin/echo", &["hi".into()], None, None, false).unwrap();
        assert_eq!(r.code, 0);
        assert_eq!(r.out, "hi\n");
        assert_eq!(r.err, "");
    }

    #[test]
    fn feeds_stdin() {
        let r = run_process("/bin/cat", &[], Some(b"abc"), None, false).unwrap();
        assert_eq!(r.out, "abc");
    }

    #[test]
    fn merges_stderr() {
        let r = run_process(
            "/bin/sh",
            &["-c".into(), "echo out; echo err 1>&2".into()],
            None,
            None,
            true,
        )
        .unwrap();
        assert!(r.out.contains("out"));
        assert!(r.out.contains("err"));
        assert!(r.err.is_empty());
    }

    #[test]
    fn reports_exit_code() {
        let r = run_process("/bin/sh", &["-c".into(), "exit 3".into()], None, None, false).unwrap();
        assert_eq!(r.code, 3);
    }
}
