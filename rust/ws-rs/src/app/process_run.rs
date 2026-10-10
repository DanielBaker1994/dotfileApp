//! Port of `ProcessRun.swift` — run a program to completion, stdin fed,
//! stdout + stderr drained concurrently (Foundation/std only).

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::thread;

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

    let status = child.wait()?;
    let mut out = out_h.join().unwrap_or_default();
    let err = err_h.join().unwrap_or_default();
    let err_str = String::from_utf8_lossy(&err).into_owned();
    if merge_stderr {
        out.extend_from_slice(&err);
    }
    Ok(ProcessOutput {
        code: status.code().unwrap_or(-1),
        out: String::from_utf8_lossy(&out).into_owned(),
        err: if merge_stderr { String::new() } else { err_str },
    })
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
