// Shared command runner. Every external program the app runs goes through a
// `CommandRunner`, always as a program path plus an argv array: never through
// a shell, never as a string the system has to split. Tests swap in
// `FakeRunner`, which records each call and returns canned output, so no test
// ever runs a real system tool.

use std::process::Command;

/// What a finished command produced. `code` is `None` if the process was
/// killed by a signal. Output is decoded lossily so odd bytes never panic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdOutput {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

pub trait CommandRunner: Send + Sync {
    fn run(&self, program: &str, args: &[&str]) -> std::io::Result<CmdOutput>;
}

/// Runs the real program with `std::process::Command` (argv only, stdin
/// closed, stdout and stderr both captured).
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, program: &str, args: &[&str]) -> std::io::Result<CmdOutput> {
        let output = Command::new(program).args(args).output()?;
        Ok(CmdOutput {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

#[cfg(test)]
pub use fake::FakeRunner;

#[cfg(test)]
mod fake {
    use super::{CmdOutput, CommandRunner};
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// Test double: records every call (program + args) and answers with the
    /// queued outputs in order. Once the queue is empty it returns an error,
    /// so an unexpected extra call fails loudly instead of passing silently.
    #[derive(Default)]
    pub struct FakeRunner {
        calls: Mutex<Vec<(String, Vec<String>)>>,
        outputs: Mutex<VecDeque<CmdOutput>>,
    }

    impl FakeRunner {
        pub fn new() -> Self {
            Self::default()
        }

        /// Queue an output for the next call.
        pub fn push(&self, code: Option<i32>, stdout: &str, stderr: &str) -> &Self {
            self.outputs.lock().unwrap().push_back(CmdOutput {
                code,
                stdout: stdout.to_string(),
                stderr: stderr.to_string(),
            });
            self
        }

        /// Queue a successful (exit 0, empty stderr) output.
        pub fn push_ok(&self, stdout: &str) -> &Self {
            self.push(Some(0), stdout, "")
        }

        pub fn calls(&self) -> Vec<(String, Vec<String>)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, program: &str, args: &[&str]) -> std::io::Result<CmdOutput> {
            self.calls.lock().unwrap().push((
                program.to_string(),
                args.iter().map(|a| a.to_string()).collect(),
            ));
            self.outputs
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| std::io::Error::other("FakeRunner: no canned output left"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_runner_records_calls_and_replays_outputs_in_order() {
        let fake = FakeRunner::new();
        fake.push_ok("first").push(Some(4), "second", "err");

        let a = fake.run("/bin/prog", &["-a", "x y"]).unwrap();
        let b = fake.run("/bin/prog", &["-b"]).unwrap();
        let c = fake.run("/bin/prog", &[]);

        assert_eq!(a.stdout, "first");
        assert_eq!(a.code, Some(0));
        assert_eq!(b.code, Some(4));
        assert_eq!(b.stderr, "err");
        assert!(c.is_err(), "exhausted queue must error");
        assert_eq!(
            fake.calls(),
            vec![
                (
                    "/bin/prog".to_string(),
                    vec!["-a".to_string(), "x y".to_string()]
                ),
                ("/bin/prog".to_string(), vec!["-b".to_string()]),
                ("/bin/prog".to_string(), vec![]),
            ]
        );
    }
}
