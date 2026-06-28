//! The UCI engine process manager.
//!
//! [`Engine`] owns a spawned child process and speaks UCI over its stdin/stdout:
//! it performs the `uci`/`isready` handshake on spawn, applies `setoption`
//! configuration, sets a `position`, drives `go`/`stop`, and yields parsed
//! analysis as [`AnalysisEvent`]s via [`Engine::next_event`].
//!
//! The struct is intentionally a thin, single-search-at-a-time adapter: callers
//! (the WebSocket route, the integration test) own the read loop so they can
//! interleave it with their own control flow (`select!` on a client `stop`).

use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdout, Command};
use tokio::time::timeout;
use vampirc_uci::UciMessage;

use super::analysis::{event_from_message, AnalysisEvent};
use super::command::{go_command, position_command, set_option_command, Limits};
use super::{parse_uci_line, EngineConfig};
use crate::position::{position_from_fen, CastlingMode};

/// How long to wait for the engine to answer `uci`/`isready` before giving up.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

/// A running UCI engine: a child process plus buffered access to its I/O.
pub struct Engine {
    config: EngineConfig,
    child: Child,
    stdin: tokio::process::ChildStdin,
    lines: Lines<BufReader<ChildStdout>>,
}

impl Engine {
    /// Spawn the configured engine and complete the UCI handshake (`uci` →
    /// `uciok`, optional `WeightsFile`, then `isready` → `readyok`). The child is
    /// killed on drop, so a dropped `Engine` never leaks a process.
    pub async fn spawn(config: EngineConfig) -> Result<Self> {
        // A `runner` (script / `wine` / `docker exec` shim) wraps the binary:
        // the program becomes the runner and the engine path its first argument.
        let mut command = match &config.runner {
            Some(runner) => {
                let mut c = Command::new(runner);
                c.arg(&config.path);
                c
            }
            None => Command::new(&config.path),
        };
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to spawn engine '{}'", config.path.display()))?;

        let stdin = child.stdin.take().context("engine stdin unavailable")?;
        let stdout = child.stdout.take().context("engine stdout unavailable")?;
        let lines = BufReader::new(stdout).lines();

        let mut engine = Self {
            config,
            child,
            stdin,
            lines,
        };
        engine.handshake().await?;
        Ok(engine)
    }

    /// The configuration this engine was spawned from.
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    async fn handshake(&mut self) -> Result<()> {
        self.send("uci").await?;
        self.read_until(HANDSHAKE_TIMEOUT, |m| matches!(m, UciMessage::UciOk))
            .await
            .context("engine did not acknowledge 'uci' (no uciok)")?;

        // Lc0 / Maia load their neural net from the WeightsFile option.
        if let Some(weights) = self.config.weights.clone() {
            let value = weights.to_string_lossy().into_owned();
            self.set_option("WeightsFile", &value).await?;
        }
        self.wait_ready().await
    }

    /// Apply a single `setoption name … value …`. Reconfiguration between
    /// searches (e.g. `MultiPV`, `Threads`, `Hash`) goes through here.
    pub async fn set_option(&mut self, name: &str, value: &str) -> Result<()> {
        self.send(&set_option_command(name, value)).await
    }

    /// `isready` round-trip: block until the engine reports `readyok`. Called
    /// after a batch of `setoption`s so configuration is in effect before `go`.
    pub async fn wait_ready(&mut self) -> Result<()> {
        self.send("isready").await?;
        self.read_until(HANDSHAKE_TIMEOUT, |m| matches!(m, UciMessage::ReadyOk))
            .await
            .context("engine did not answer 'isready' (no readyok)")
    }

    /// Set the position to analyse from a FEN. The FEN is validated through the
    /// pure `position` module first, so a malformed FEN errors here instead of
    /// confusing the engine.
    pub async fn set_position(&mut self, fen: &str) -> Result<()> {
        position_from_fen(fen, CastlingMode::Standard)
            .map_err(|e| anyhow!("invalid FEN for analysis: {e}"))?;
        self.send(&position_command(fen)).await
    }

    /// Start a search under the given limits (`go …`). Stream the resulting
    /// events with [`Engine::next_event`] until a `bestmove` arrives.
    pub async fn go(&mut self, limits: &Limits) -> Result<()> {
        self.send(&go_command(limits)).await
    }

    /// Ask the engine to stop searching; it answers with a final `bestmove`.
    pub async fn stop(&mut self) -> Result<()> {
        self.send("stop").await
    }

    /// Await the next analysis event (`info` / `bestmove`), transparently
    /// skipping handshake replies and non-analysis chatter. Returns `None` when
    /// the engine closes its output stream (it exited).
    pub async fn next_event(&mut self) -> Result<Option<AnalysisEvent>> {
        loop {
            match self.next_message().await? {
                Some(msg) => {
                    if let Some(event) = event_from_message(msg) {
                        return Ok(Some(event));
                    }
                }
                None => return Ok(None),
            }
        }
    }

    /// Send `quit` and wait briefly for a clean exit; `kill_on_drop` is the
    /// backstop if the engine ignores it.
    pub async fn quit(mut self) -> Result<()> {
        let _ = self.send("quit").await;
        let _ = timeout(Duration::from_secs(2), self.child.wait()).await;
        Ok(())
    }

    async fn send(&mut self, line: &str) -> Result<()> {
        self.stdin
            .write_all(line.as_bytes())
            .await
            .context("writing to engine stdin")?;
        self.stdin.write_all(b"\n").await?;
        self.stdin.flush().await?;
        Ok(())
    }

    async fn next_message(&mut self) -> Result<Option<UciMessage>> {
        match self
            .lines
            .next_line()
            .await
            .context("reading engine stdout")?
        {
            Some(line) => Ok(Some(parse_uci_line(line.trim()))),
            None => Ok(None),
        }
    }

    /// Read messages until `pred` matches, bounded by `dur`. Used for the
    /// fixed-response handshake exchanges (`uciok`, `readyok`).
    async fn read_until<F>(&mut self, dur: Duration, pred: F) -> Result<()>
    where
        F: Fn(&UciMessage) -> bool,
    {
        let wait = async {
            loop {
                match self.next_message().await? {
                    Some(msg) if pred(&msg) => return Ok(()),
                    Some(_) => continue,
                    None => bail!("engine closed its output stream unexpectedly"),
                }
            }
        };
        timeout(dur, wait)
            .await
            .context("timed out waiting for engine response")?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Spawning a path that isn't an executable must fail cleanly rather than
    // hang or panic. This needs no real engine, so it always runs.
    #[tokio::test]
    async fn spawn_missing_binary_errors() {
        let cfg = EngineConfig::new("nope", "/nonexistent/chess-base/engine-xyz");
        assert!(Engine::spawn(cfg).await.is_err());
    }
}
