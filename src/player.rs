use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread;

/// Candidate raw-PCM sinks, tried in order. Each reads s16le mono 48 kHz on stdin.
fn candidates() -> Vec<(&'static str, Vec<&'static str>)> {
    vec![
        ("pw-cat", vec!["--playback", "--raw", "--latency", "20ms", "--rate", "48000", "--channels", "1", "--format", "s16", "-"]),
        ("paplay", vec!["--raw", "--rate=48000", "--channels=1", "--format=s16le"]),
        ("aplay", vec!["-q", "-t", "raw", "-r", "48000", "-c", "1", "-f", "S16_LE"]),
    ]
}

fn spawn_sink() -> Option<Child> {
    for (bin, args) in candidates() {
        if let Ok(child) = Command::new(bin)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            return Some(child);
        }
    }
    None
}

#[derive(Clone)]
pub struct Player(Sender<Arc<Vec<u8>>>);

impl Player {
    /// Keeps one audio stream open so each sound has low latency instead of
    /// paying process-spawn cost per play.
    pub fn start() -> Self {
        let (tx, rx) = mpsc::channel::<Arc<Vec<u8>>>();
        thread::spawn(move || {
            // Open the stream now and prime it with silence, so the first sound
            // is not clipped by stream start-up.
            let mut sink = spawn_sink();
            if let Some(w) = sink.as_mut().and_then(|c| c.stdin.as_mut()) {
                let _ = w.write_all(&vec![0u8; crate::sound::RATE as usize / 10 * 2]);
            }
            for pcm in rx {
                for _ in 0..2 {
                    if sink.is_none() {
                        sink = spawn_sink();
                        if sink.is_none() {
                            eprintln!("no audio player found (tried pw-cat, paplay, aplay)");
                            break;
                        }
                    }
                    let ok = sink
                        .as_mut()
                        .and_then(|c| c.stdin.as_mut())
                        .is_some_and(|w| w.write_all(&pcm).and_then(|_| w.flush()).is_ok());
                    if ok {
                        break;
                    }
                    sink = None; // died; respawn once
                }
            }
        });
        Player(tx)
    }

    pub fn play(&self, pcm: &Arc<Vec<u8>>) {
        let _ = self.0.send(Arc::clone(pcm));
    }
}
